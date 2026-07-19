//! Feature-selected [`TranscriptionEngine`] construction and the
//! transcribe → clean → persist pipeline that runs after capture stops.
//!
//! Exactly one `build_engine` variant below compiles, chosen by the
//! mutually-exclusive `parakeet`/`whisper` Cargo features (forwarded from
//! `kodabi-transcribe`'s own — see that crate's Cargo.toml for the
//! sherpa-onnx static/shared link-mode conflict that keeps them from ever
//! coexisting in one binary). Neither is on by default, so the default build
//! transcribes with `kodabi_core::transcription::MockEngine` and stays
//! native-dependency-free (CI-green ahead of #37's benchmark-and-lock).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use kodabi_audio::{AlignedSession, CombinedSession, MonoResampler, SessionChannel, SpillReader};
use kodabi_core::device::DeviceId;
use kodabi_core::glossary::Glossary;
use kodabi_core::inflight::{self, InflightSession, OrphanKind, RecoverableOrphan};
use kodabi_core::metrics::PipelineTimings;
use kodabi_core::pipeline::transcribe_and_persist;
use kodabi_core::transcription::{self, AudioSource, Channel, SliceSource, TranscriptionEngine};
use kodabi_llm::{ClaudeConfig, ClaudeRunner};
use tauri::{AppHandle, Emitter, Manager};

/// All engines expect mono `f32` PCM at this rate.
const ENGINE_SAMPLE_RATE_HZ: u32 = 16_000;

/// Samples per chunk fed to the engine on the in-memory fallback path. Ten
/// seconds at 16 kHz — matches the streamed-from-disk cadence so both paths
/// exercise the engine the same way.
const IN_MEMORY_CHUNK_SAMPLES: usize = ENGINE_SAMPLE_RATE_HZ as usize * 10;

/// How long an un-recoverable in-flight directory (corrupt, lone-channel, a
/// mis-tap) is kept before [`kodabi_core::inflight::sweep_stale`] deletes it.
/// Generous, so a directory that is merely failing to transcribe (a missing
/// model) is retried across many launches rather than swept. `pub(crate)` so
/// the retention schedule can piggyback the sweep on its cadence.
pub(crate) const INFLIGHT_STALE_GRACE: Duration = Duration::from_secs(48 * 60 * 60);

/// Event the frontend subscribes to for post-capture transcription progress.
pub const TRANSCRIPTION_STATE_EVENT: &str = "transcription:state";

/// Payload for [`TRANSCRIPTION_STATE_EVENT`]. Tagged on `status` so the
/// frontend can switch on that alone; `path`/`message` only accompany their
/// matching variant.
#[derive(Clone, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum TranscriptionStateEvent {
    Transcribing,
    Saved { path: String },
    Error { message: String },
}

/// Minimum captured length worth transcribing. A shorter two-channel session
/// is treated as an accidental toggle (a mis-tap start→stop) rather than a
/// real meeting: running the pipeline on one only writes a near-empty session
/// file and flashes "Saved" for nothing, so we skip it and leave the UI idle.
/// `pub(crate)` so recovery's stale-sweep applies the same mis-tap bar.
pub(crate) const MIN_TRANSCRIBE_DURATION: Duration = Duration::from_secs(1);

/// Serializes pipeline runs so back-to-back meetings never hold two
/// heavyweight transcription engines (each a multi-hundred-MB model) resident
/// at once. A second stop's thread blocks on this until the first finishes
/// instead of running concurrently and doubling peak memory/CPU.
static TRANSCRIBE_LOCK: Mutex<()> = Mutex::new(());

/// Spawns the transcribe → clean → persist pipeline on a background thread
/// and returns immediately, so the capture-stop path (and the toggle lock it
/// runs under) never blocks on model load or the headless Claude cleanup
/// call. A session shorter than [`MIN_TRANSCRIBE_DURATION`] is dropped without
/// spawning anything; concurrent runs are serialized on [`TRANSCRIBE_LOCK`].
///
/// `inflight` is the on-disk session backing the audio (present whenever the
/// spill directory was created at start): its `started_at` gives the recovered
/// session an accurate `captured_at`, and it is removed once the transcript
/// lands or kept (for next-launch recovery) if the run fails. When absent
/// (spill setup failed, so capture stayed in memory), `captured_at` falls back
/// to back-dating from the duration.
pub fn spawn_transcription(
    app: &AppHandle,
    combined: CombinedSession,
    inflight: Option<InflightSession>,
) {
    let duration = combined_duration(&combined);
    if duration < MIN_TRANSCRIBE_DURATION {
        // A mis-tap: nothing worth transcribing. Drop the in-flight directory.
        if let Some(inflight) = inflight {
            if let Err(err) = inflight.remove() {
                eprintln!("capture: failed to remove short in-flight session: {err}");
            }
        }
        return;
    }

    let app = app.clone();
    // Prefer the in-flight start instant (stamped at capture start) over
    // back-dating from the duration, which only estimates it.
    let captured_at = inflight
        .as_ref()
        .map(InflightSession::started_at)
        .unwrap_or_else(|| {
            Utc::now() - chrono::Duration::milliseconds(duration.as_millis() as i64)
        });

    std::thread::spawn(move || {
        // Hold the lock across the whole pipeline so only one engine is ever
        // resident; a queued stop waits here rather than piling a second model
        // load on top of the first.
        let _guard = TRANSCRIBE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Announce `Transcribing` only once this run actually starts (i.e.
        // after the lock is held). Emitting before blocking on the lock would
        // let a queued back-to-back stop fire a second `Transcribing` up front
        // and leave a stale `Saved`/`Error` showing while it waits its turn.
        let _ = app.emit(
            TRANSCRIPTION_STATE_EVENT,
            TranscriptionStateEvent::Transcribing,
        );
        match run(&app, combined, captured_at) {
            Ok(path) => {
                let _ = app.emit(
                    TRANSCRIPTION_STATE_EVENT,
                    TranscriptionStateEvent::Saved {
                        path: path.display().to_string(),
                    },
                );
                // The transcript is safely on disk (atomic hard-link claim), so
                // the spill is no longer needed: remove the in-flight directory.
                // Order matters — remove only after the `.jsonl` lands, so a
                // crash before this leaves a re-recoverable directory.
                if let Some(inflight) = inflight {
                    if let Err(err) = inflight.remove() {
                        eprintln!("capture: failed to remove in-flight session: {err}");
                    }
                }
                // End-of-meeting brain-pass (FOUNDING_DOC §3.5): distill the
                // freshly saved session into a note. It has its own lock,
                // thread, and event channel, so a slow or failing distill
                // never disturbs the transcription flow reported above.
                // Real engines only: a default (MockEngine) build would spend
                // a real headless Claude call on placeholder text and write a
                // junk note into the KB on every dev capture. To exercise the
                // distill from a mock build, invoke the `distill_session`
                // command on the saved path instead.
                #[cfg(any(feature = "parakeet", feature = "whisper"))]
                if !crate::distill_cmds::spawn_distill(&app, path.clone()) {
                    // A freshly written session can't already be claimed, so
                    // this would mean a duplicate path escaped the writer.
                    eprintln!(
                        "distill: {} is already queued; not queuing it twice",
                        path.display()
                    );
                }
            }
            Err(message) => {
                eprintln!("transcription pipeline failed: {message}");
                let _ = app.emit(
                    TRANSCRIPTION_STATE_EVENT,
                    TranscriptionStateEvent::Error { message },
                );
                // Keep the in-flight directory (dropping `inflight` releases the
                // lock but leaves the spill on disk) so the next launch can
                // retry it — a transient failure like a missing model must not
                // discard a real recording.
            }
        }
    });
}

/// Duration of a finalized session, whichever form it took.
fn combined_duration(combined: &CombinedSession) -> Duration {
    match combined {
        CombinedSession::InMemory(session) => session.duration(),
        CombinedSession::Spilled(spilled) => spilled.duration(),
    }
}

/// On startup, recover any orphaned in-flight capture sessions (a crash or kill
/// mid-meeting) and sweep away un-recoverable leftovers. Runs on a detached
/// thread so it never blocks launch; a missing in-flight root is a no-op.
///
/// Each recoverable orphan flows through the *same* transcribe → persist →
/// distill chain a normal stop uses, emitting the same `transcription:state`
/// events, so the UI surfaces "Transcribing…" → "Saved" for it with no new
/// wiring. Runs are serialized against live transcription on [`TRANSCRIBE_LOCK`].
pub fn spawn_recovery(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let sessions_dir = match knowledge_base_dir(&app) {
            Ok(kb) => kb.join("sessions"),
            Err(err) => {
                eprintln!("capture recovery skipped: {err}");
                return;
            }
        };
        let root = inflight::inflight_root(&sessions_dir);

        let orphans = match inflight::scan(&root, MIN_TRANSCRIBE_DURATION) {
            Ok(orphans) => orphans,
            Err(err) => {
                eprintln!("capture recovery scan failed: {err}");
                return;
            }
        };
        for orphan in orphans {
            if let OrphanKind::Recoverable(orphan) = orphan {
                recover_orphan(&app, orphan);
            }
            // `Discard` entries are left for `sweep_stale` below, which only
            // deletes them once they are safely past the grace window.
        }

        if let Err(err) = inflight::sweep_stale(
            &root,
            Utc::now(),
            INFLIGHT_STALE_GRACE,
            MIN_TRANSCRIBE_DURATION,
        ) {
            eprintln!("capture recovery sweep failed: {err}");
        }
    });
}

/// Transcribe one recovered orphan through the shared pipeline and, on success,
/// delete its directory. A failure keeps the directory (dropping `orphan`
/// releases its lock but leaves the spill on disk) for the next launch to retry.
fn recover_orphan(app: &AppHandle, orphan: RecoverableOrphan) {
    let _guard = TRANSCRIBE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let captured_at = orphan.meta.started_at;

    // Idempotency: the spill directory is deleted only *after* its transcript
    // lands, so a crash (or a failed removal) in that window leaves a directory
    // whose meeting is already saved. If a transcript for this instant+device
    // is already on disk, don't re-transcribe it into a duplicate `.jsonl` +
    // note — just clear the leftover spill.
    if let Ok(kb_dir) = knowledge_base_dir(app) {
        let device = app.state::<DeviceId>().inner().clone();
        if kodabi_core::raw_session::session_exists(&kb_dir.join("sessions"), captured_at, &device)
        {
            if let Err(err) = orphan.remove() {
                eprintln!("capture recovery: failed to remove already-transcribed session: {err}");
            }
            return;
        }
    }

    let _ = app.emit(
        TRANSCRIPTION_STATE_EVENT,
        TranscriptionStateEvent::Transcribing,
    );
    match run_spilled(
        app,
        &orphan.mic_pcm,
        &orphan.system_pcm,
        orphan.meta.sample_rate,
        captured_at,
    ) {
        Ok(path) => {
            let _ = app.emit(
                TRANSCRIPTION_STATE_EVENT,
                TranscriptionStateEvent::Saved {
                    path: path.display().to_string(),
                },
            );
            if let Err(err) = orphan.remove() {
                eprintln!("capture recovery: failed to remove recovered session: {err}");
            }
            #[cfg(any(feature = "parakeet", feature = "whisper"))]
            crate::distill_cmds::spawn_distill(app, path);
        }
        Err(message) => {
            eprintln!("capture recovery pipeline failed: {message}");
            let _ = app.emit(
                TRANSCRIPTION_STATE_EVENT,
                TranscriptionStateEvent::Error { message },
            );
        }
    }
}

/// Resolves the knowledge-base root — the plain, user-syncable folder that
/// holds sessions, glossaries, and (later) routed notes (FOUNDING_DOC's
/// "plain folder" model). This is a placeholder location: today it is the
/// app-data dir, but it is the single seam a future vault-path setting
/// replaces, so every KB path derives from here rather than calling
/// `app_data_dir()` inline. Per-project subfolders — each with their own
/// `_glossary.yml` — will hang off this once routing lands. Shared with
/// `note_cmds` so the note writer resolves the same KB root as the transcribe
/// pipeline (both must route through this single seam).
pub(crate) fn knowledge_base_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|err| format!("failed to resolve knowledge base directory: {err}"))
}

/// Runs the pure `kodabi-core` pipeline against a finalized session, streaming
/// from its spill files (a long meeting never materialises in memory) or from
/// the in-memory buffer when spilling was unavailable. Errors collapse to a
/// message string — the same convention `audio_cmds` uses for IPC results —
/// since the only consumer here is the `transcription:state` event.
fn run(
    app: &AppHandle,
    combined: CombinedSession,
    captured_at: DateTime<Utc>,
) -> Result<PathBuf, String> {
    match combined {
        CombinedSession::Spilled(spilled) => run_spilled(
            app,
            &spilled.mic_path,
            &spilled.system_path,
            spilled.sample_rate,
            captured_at,
        ),
        CombinedSession::InMemory(session) => run_in_memory(app, &session, captured_at),
    }
}

/// Stream both channels off their spill files, resampling to 16 kHz on the fly,
/// so nothing but the current chunk is ever resident. Shared by a normal stop
/// (the live spill) and startup recovery (an orphaned spill).
fn run_spilled(
    app: &AppHandle,
    mic_path: &Path,
    system_path: &Path,
    source_rate: u32,
    captured_at: DateTime<Utc>,
) -> Result<PathBuf, String> {
    let mut mic =
        Resampling16kSource::open(mic_path, source_rate).map_err(|err| err.to_string())?;
    let mut system =
        Resampling16kSource::open(system_path, source_rate).map_err(|err| err.to_string())?;
    let mut channels: [(Channel, &mut dyn AudioSource); 2] =
        [(Channel::You, &mut mic), (Channel::Them, &mut system)];
    persist(app, &mut channels, captured_at)
}

/// The in-memory fallback (spill files couldn't be created): resample both
/// channels up front and feed them in fixed-size chunks.
fn run_in_memory(
    app: &AppHandle,
    session: &AlignedSession,
    captured_at: DateTime<Utc>,
) -> Result<PathBuf, String> {
    let mic_pcm = session
        .channel_resampled(SessionChannel::Mic, ENGINE_SAMPLE_RATE_HZ)
        .map_err(|err| err.to_string())?;
    let system_pcm = session
        .channel_resampled(SessionChannel::System, ENGINE_SAMPLE_RATE_HZ)
        .map_err(|err| err.to_string())?;
    let mut mic = SliceSource::new(&mic_pcm, IN_MEMORY_CHUNK_SAMPLES);
    let mut system = SliceSource::new(&system_pcm, IN_MEMORY_CHUNK_SAMPLES);
    let mut channels: [(Channel, &mut dyn AudioSource); 2] =
        [(Channel::You, &mut mic), (Channel::Them, &mut system)];
    persist(app, &mut channels, captured_at)
}

/// Load the glossary and device identity and run the core transcribe → clean →
/// persist pipeline over `channels` (each already yielding 16 kHz mono `f32`).
fn persist(
    app: &AppHandle,
    channels: &mut [(Channel, &mut dyn AudioSource)],
    captured_at: DateTime<Utc>,
) -> Result<PathBuf, String> {
    let kb_dir = knowledge_base_dir(app)?;
    let glossary = Glossary::load(&kb_dir).map_err(|err| err.to_string())?;
    let device = app.state::<DeviceId>().inner().clone();

    let cleaner = ClaudeRunner::new(ClaudeConfig::cleanup_from_env());
    let mut make_engine = build_engine;

    let outcome = transcribe_and_persist(
        &mut make_engine,
        &cleaner,
        &glossary,
        channels,
        ENGINE_SAMPLE_RATE_HZ,
        &kb_dir.join("sessions"),
        captured_at,
        &device,
        None,
    )
    .map_err(|err| err.to_string())?;

    emit_metrics(&outcome.timings);
    Ok(outcome.path)
}

/// Reads a spilled channel file and resamples it from the capture rate (48 kHz)
/// down to the 16 kHz the engines expect, a chunk at a time — the disk-backed
/// [`AudioSource`] the streamed transcription path pulls from.
struct Resampling16kSource {
    reader: SpillReader,
    resampler: MonoResampler,
    read_chunk: usize,
    /// Reused copy-out buffer for each read, so the reader borrow can end
    /// before the resampler borrow begins without allocating a fresh `Vec` per
    /// chunk over a long recording.
    input: Vec<f32>,
    buf: Vec<f32>,
    finished: bool,
}

impl Resampling16kSource {
    fn open(path: &Path, source_rate: u32) -> io::Result<Self> {
        let reader = SpillReader::open(path)?;
        let resampler = MonoResampler::new(source_rate, ENGINE_SAMPLE_RATE_HZ)
            .map_err(|err| io::Error::other(err.to_string()))?;
        Ok(Resampling16kSource {
            reader,
            resampler,
            // Read ~10s of source audio per pull; resampling shrinks it to the
            // ~10s of 16 kHz the engine then consumes.
            read_chunk: source_rate as usize * 10,
            input: Vec::new(),
            buf: Vec::new(),
            finished: false,
        })
    }
}

impl AudioSource for Resampling16kSource {
    fn next_chunk(&mut self) -> io::Result<Option<&[f32]>> {
        loop {
            if self.finished {
                return Ok(None);
            }
            match self.reader.next_chunk(self.read_chunk)? {
                Some(samples) => {
                    // Copy out into the reused buffer so the reader borrow ends
                    // before the resampler borrow begins (disjoint fields).
                    self.input.clear();
                    self.input.extend_from_slice(samples);
                    let resampled = self.resampler.push(&self.input);
                    if resampled.is_empty() {
                        continue; // not yet a full resample chunk — read more
                    }
                    self.buf = resampled;
                    return Ok(Some(&self.buf));
                }
                None => {
                    self.finished = true;
                    let tail = self.resampler.flush();
                    if tail.is_empty() {
                        return Ok(None);
                    }
                    self.buf = tail;
                    return Ok(Some(&self.buf));
                }
            }
        }
    }
}

/// Env var naming where to append one JSON line of this run's
/// [`PipelineTimings`] (plus the derived `speed_x`) after a transcription
/// completes: a file path to append to, or the literal `stderr`. Unset (the
/// default) emits nothing — a resource-budget measurement pass opts in, at
/// zero cost otherwise. See `docs/RESOURCE_BUDGET.md`.
const METRICS_ENV_VAR: &str = "KODABI_METRICS";

/// One [`PipelineTimings`] line, with `speed_x` folded in alongside the
/// flattened timing fields — `speed_x` is a derived method, not a field on
/// `PipelineTimings`, and a resource-budget pass reading the JSONL shouldn't
/// have to recompute it from the raw numbers.
#[derive(serde::Serialize)]
struct MetricsLine<'a> {
    #[serde(flatten)]
    timings: &'a PipelineTimings,
    speed_x: f64,
}

/// Reads [`METRICS_ENV_VAR`] and, if set, writes `timings` there via
/// [`write_metrics_line`]; a no-op otherwise. Thin on purpose — the real
/// logic lives in [`write_metrics_line`], which takes the target explicitly
/// so it can be unit-tested without mutating the real process environment (a
/// shared, racy resource under Rust's default parallel test runner).
fn emit_metrics(timings: &PipelineTimings) {
    if let Ok(target) = std::env::var(METRICS_ENV_VAR) {
        write_metrics_line(timings, &target);
    }
}

/// Serializes `timings` (plus the derived `speed_x`) to one JSON line and
/// appends it to `target` — a file path, or the literal `stderr` to print it
/// there instead. A bad target path or a write failure is logged and
/// swallowed rather than surfaced — a metrics sink must never take down a
/// real meeting's transcript.
fn write_metrics_line(timings: &PipelineTimings, target: &str) {
    let line = match serde_json::to_string(&MetricsLine {
        timings,
        speed_x: timings.speed_x(),
    }) {
        Ok(line) => line,
        Err(err) => {
            eprintln!("{METRICS_ENV_VAR}: failed to serialize timings: {err}");
            return;
        }
    };

    if target == "stderr" {
        eprintln!("{line}");
        return;
    }
    use std::io::Write;
    let result = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(target)
        .and_then(|mut file| writeln!(file, "{line}"));
    if let Err(err) = result {
        eprintln!("{METRICS_ENV_VAR}: failed to write {target}: {err}");
    }
}

#[cfg(all(feature = "parakeet", not(feature = "whisper")))]
fn build_engine() -> transcription::Result<Box<dyn TranscriptionEngine>> {
    use kodabi_transcribe::{ParakeetConfig, ParakeetEngine};

    let config = ParakeetConfig {
        encoder: env_model_path("PARAKEET_ENCODER"),
        decoder: env_model_path("PARAKEET_DECODER"),
        joiner: env_model_path("PARAKEET_JOINER"),
        tokens: env_model_path("PARAKEET_TOKENS"),
        vad_model: env_model_path("PARAKEET_VAD_MODEL"),
        num_threads: 1,
        provider: Some("cpu".to_owned()),
        vad_threshold: 0.5,
        min_silence_duration: 0.25,
        min_speech_duration: 0.25,
        max_speech_duration: 20.0,
        debug: false,
    }
    .apply_env_overrides();
    Ok(Box::new(ParakeetEngine::new(config)?))
}

#[cfg(all(feature = "whisper", not(feature = "parakeet")))]
fn build_engine() -> transcription::Result<Box<dyn TranscriptionEngine>> {
    use kodabi_transcribe::{whisper_with_vad, VadConfig, WhisperConfig};

    let whisper_config = WhisperConfig {
        model: env_model_path("WHISPER_MODEL"),
        use_gpu: true,
        num_threads: 4,
        language: Some("en".to_owned()),
    }
    .apply_env_overrides();
    let vad_config = VadConfig {
        vad_model: env_model_path("VAD_MODEL"),
        num_threads: 1,
        provider: Some("cpu".to_owned()),
        vad_threshold: 0.5,
        min_silence_duration: 0.25,
        min_speech_duration: 0.25,
        max_speech_duration: 20.0,
        debug: false,
    }
    .apply_env_overrides();
    Ok(Box::new(whisper_with_vad(whisper_config, vad_config)?))
}

#[cfg(not(any(feature = "parakeet", feature = "whisper")))]
fn build_engine() -> transcription::Result<Box<dyn TranscriptionEngine>> {
    Ok(Box::new(transcription::MockEngine::new()))
}

/// Reads a model file path from an environment variable. An unset var
/// resolves to an empty path, which fails the engine constructor's own
/// `require_file` check with a clear `TranscriptionError::ModelLoad` rather
/// than panicking here — model download/settings wiring is a later ticket
/// (see `ParakeetConfig`'s docs).
#[cfg(any(feature = "parakeet", feature = "whisper"))]
fn env_model_path(var: &str) -> PathBuf {
    std::env::var(var).unwrap_or_default().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_timings() -> PipelineTimings {
        PipelineTimings {
            audio_secs: 4.0,
            engine_build_ms: 1,
            transcribe_ms: vec![2],
            assemble_ms: 0,
            cleanup_ms: 0,
            persist_ms: 0,
            total_ms: 2,
        }
    }

    /// A unique path per test run, in the OS temp dir — avoids adding a
    /// `tempfile` dev-dependency for what's otherwise this crate's only test
    /// needing a scratch file.
    fn scratch_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "kodabi-metrics-test-{}-{name}.jsonl",
            std::process::id()
        ))
    }

    #[test]
    fn write_metrics_line_appends_a_json_line_with_speed_x() {
        let path = scratch_path("basic");
        let _ = std::fs::remove_file(&path);

        write_metrics_line(&sample_timings(), path.to_str().unwrap());

        let contents = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).ok();

        let value: serde_json::Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(value["audio_secs"], 4.0);
        assert_eq!(value["total_ms"], 2);
        // 4.0s audio / (2ms / 1000) wall = 2000x realtime.
        assert_eq!(value["speed_x"], 2000.0);
    }

    #[test]
    fn write_metrics_line_appends_rather_than_overwrites() {
        let path = scratch_path("append");
        let _ = std::fs::remove_file(&path);

        write_metrics_line(&sample_timings(), path.to_str().unwrap());
        write_metrics_line(&sample_timings(), path.to_str().unwrap());

        let contents = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(contents.lines().count(), 2);
    }

    #[test]
    fn write_metrics_line_to_a_bad_path_does_not_panic() {
        // A path with a directory component that doesn't exist — the write
        // fails, but the caller (a real meeting's transcription) must not.
        write_metrics_line(
            &sample_timings(),
            "Z:\\definitely\\not\\a\\real\\path.jsonl",
        );
    }
}
