//! Feature-selected [`TranscriptionEngine`] construction and the
//! transcribe → clean → persist pipeline that runs after capture stops.
//!
//! Exactly one `build_engine` variant below compiles, chosen by the
//! mutually-exclusive `parakeet`/`whisper` Cargo features (forwarded from
//! `kodabi-transcribe`'s own — see that crate's Cargo.toml for the
//! sherpa-onnx static/shared link-mode conflict that keeps them from ever
//! coexisting in one binary). Neither is on by default, so a *debug* build
//! transcribes with `kodabi_core::transcription::MockEngine` and stays
//! native-dependency-free (keeping the workspace gates fast and CI-green).
//!
//! Release builds are different: the `compile_error!` guard below refuses to
//! compile a release binary that picked no engine, so a mock build can never
//! ship. Release builds pass `--features parakeet,embed` (`pnpm tauri:build`) —
//! the engine locked in by `docs/benchmarks/stt-engine-benchmark.md`, plus the
//! embedder, which has no guard of its own and is pinned only by that script.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use kodabi_aec::EchoCanceller;
use kodabi_audio::{AlignedSession, CombinedSession, MonoResampler, SessionChannel, SpillReader};
use kodabi_core::device::DeviceId;
use kodabi_core::glossary::Glossary;
use kodabi_core::inflight::{self, InflightSession, OrphanKind, RecoverableOrphan};
use kodabi_core::metrics::PipelineTimings;
use kodabi_core::pipeline::{transcribe_and_persist, PipelineProgress};
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

/// Staging filename for the retained recording while it is being written
/// inside the in-flight spill directory, before its atomic rename into
/// `sessions/`. The directory is exclusive to one session (and removed
/// wholesale), so no uniquifier is needed.
const AUDIO_STAGING_NAME: &str = ".audio.tmp";

/// Event the frontend subscribes to for post-capture transcription progress.
pub const TRANSCRIPTION_STATE_EVENT: &str = "transcription:state";

/// Payload for [`TRANSCRIPTION_STATE_EVENT`]. Tagged on `status` so the
/// frontend can switch on that alone; the fields only accompany their
/// matching variant. Mirrored by `TranscriptionState` in
/// `src/useTranscriptionState.ts` (`.claude/rules/tauri-command-parity.md`):
/// `rename_all` renames the *variants*, so the fields ride the wire in
/// snake_case and the TS type spells them that way too.
#[derive(Clone, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum TranscriptionStateEvent {
    /// This run is parked behind a predecessor holding [`TRANSCRIBE_LOCK`].
    /// Genuinely waiting, which is the one case the UI may call "queued".
    Queued,
    /// Progress through the ASR pass, recording-normalized: `seconds_processed`
    /// is the audio fed to the engines divided by the channel count and
    /// clamped to `total_seconds`, which is the recording's own duration. The
    /// channels are transcribed one after the other, so the raw fed total runs
    /// to twice the meeting's length — a figure the user has no way to read
    /// against the meeting they remember.
    Transcribing {
        seconds_processed: f64,
        total_seconds: f64,
    },
    Saved {
        path: String,
    },
    Error {
        message: String,
    },
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

    let total_seconds = duration.as_secs_f64();

    std::thread::spawn(move || {
        // Hold the lock across the whole pipeline so only one engine is ever
        // resident; a queued stop waits here rather than piling a second model
        // load on top of the first. The `try_lock` probe exists to name that
        // wait: a run parked behind a predecessor says `Queued` and nothing
        // else until its turn comes, which is the only state the UI is allowed
        // to call queued. `Transcribing` still waits for the lock — emitting it
        // up front would leave a stale `Saved`/`Error` from the previous run
        // showing while this one merely waits.
        let _guard = acquire_transcribe_lock(&app);
        let _ = app.emit(
            TRANSCRIPTION_STATE_EVENT,
            TranscriptionStateEvent::Transcribing {
                seconds_processed: 0.0,
                total_seconds,
            },
        );
        match run(&app, combined, captured_at, total_seconds) {
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

/// Take [`TRANSCRIBE_LOCK`], announcing the wait if there is one.
///
/// The probe is the whole point: an uncontended run acquires silently and goes
/// straight to `Transcribing`, while one parked behind a predecessor emits
/// `Queued` first, so "queued" on screen means a run that genuinely is. A
/// poisoned lock is recovered rather than propagated — the mutex guards
/// serialization, not data, so a panicking predecessor must not wedge every
/// later meeting.
fn acquire_transcribe_lock(app: &AppHandle) -> std::sync::MutexGuard<'static, ()> {
    match TRANSCRIBE_LOCK.try_lock() {
        Ok(guard) => guard,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => {
            let _ = app.emit(TRANSCRIPTION_STATE_EVENT, TranscriptionStateEvent::Queued);
            TRANSCRIBE_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        }
    }
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
    let _guard = acquire_transcribe_lock(app);
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

    // The in-flight metadata records when capture started but not how much of
    // it reached the disk, so the denominator comes from the spill files
    // themselves. The longer channel is the session's length, matching what
    // `AlignedSession::duration` would have reported for a normal stop.
    let total_seconds = spill_seconds(&orphan.mic_pcm, orphan.meta.sample_rate)
        .max(spill_seconds(&orphan.system_pcm, orphan.meta.sample_rate));

    let _ = app.emit(
        TRANSCRIPTION_STATE_EVENT,
        TranscriptionStateEvent::Transcribing {
            seconds_processed: 0.0,
            total_seconds,
        },
    );
    match run_spilled(
        app,
        &orphan.mic_pcm,
        &orphan.system_pcm,
        orphan.meta.sample_rate,
        captured_at,
        total_seconds,
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
            // Same real-engines-only gate as the normal capture path above.
            #[cfg(any(feature = "parakeet", feature = "whisper"))]
            if !crate::distill_cmds::spawn_distill(app, path.clone()) {
                // Recovery runs at startup, before anything else can queue a
                // distill, so this would mean a duplicate recovered path.
                eprintln!(
                    "distill: {} is already queued; not queuing it twice",
                    path.display()
                );
            }
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

/// Length of a spilled channel, from its size alone.
///
/// Spill files are raw headerless `f32` little-endian at the capture rate
/// (`kodabi_audio::spill`), so this is one `metadata` call rather than a read.
/// The division floors, which is what a crash-truncated trailing partial
/// sample deserves — the reader tolerates it the same way. An unreadable file
/// (or a zero sample rate) reports zero, leaving the caller with no
/// denominator rather than a wrong one.
fn spill_seconds(path: &Path, sample_rate: u32) -> f64 {
    if sample_rate == 0 {
        return 0.0;
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return 0.0;
    };
    let samples = meta.len() / std::mem::size_of::<f32>() as u64;
    samples as f64 / f64::from(sample_rate)
}

/// Environment override for the knowledge-base root.
///
/// Deliberately the same name `kodabi-mcp` already reads at startup
/// (`crates/kodabi-mcp/src/config.rs`) and that the generated `.mcp.json`
/// already writes (`kodabi_core::terminal`), so one name means one thing across
/// the tree: the app that *generates* the sidecar's config while ignoring the
/// same variable itself is exactly the drift `.claude/rules/tauri-command-parity.md`
/// exists to prevent, one layer down.
///
/// Always-on rather than `#[cfg(debug_assertions)]`, so the end-to-end harness
/// (`e2e/`) exercises the path that actually ships. Setting it also requires
/// setting `KODABI_INDEX_DB` — see `index_state::open_index`.
///
/// Setting the pair by hand is the low-level form. `KODABI_SANDBOX` is the
/// high-level one: it derives both (plus the config dir and the WebView2
/// profile) from a single base and writes them into this process's environment
/// before anything reads them, so this resolver needs no sandbox branch of its
/// own. Setting this variable *alongside* `KODABI_SANDBOX` is refused at
/// startup rather than silently half-honoured — see `crate::sandbox`.
const KB_ROOT_ENV: &str = "KODABI_KB_ROOT";

/// Resolves the knowledge-base root — the plain, user-syncable folder that
/// holds sessions, glossaries, and (later) routed notes (FOUNDING_DOC's
/// "plain folder" model). Unset, this is the app-data dir; it is the single
/// seam a future vault-path setting replaces, so every KB path derives from
/// here rather than calling `app_data_dir()` inline, and `KODABI_KB_ROOT` is
/// the first, smallest form of that setting. Per-project subfolders — each with
/// their own `_glossary.yml` — hang off this. Shared with `note_cmds` so the
/// note writer resolves the same KB root as the transcribe pipeline (both must
/// route through this single seam).
pub(crate) fn knowledge_base_dir(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some(root) = std::env::var_os(KB_ROOT_ENV).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(root));
    }
    app.path()
        .app_data_dir()
        .map_err(|err| format!("failed to resolve knowledge base directory: {err}"))
}

/// Runs the pure `kodabi-core` pipeline against a finalized session, streaming
/// from its spill files (a long meeting never materialises in memory) or from
/// the in-memory buffer when spilling was unavailable. Errors collapse to a
/// message string — the same convention `audio_cmds` uses for IPC results —
/// since the only consumer here is the `transcription:state` event.
///
/// `total_seconds` is the recording's own length, carried down solely as the
/// denominator for the progress this pipeline emits.
fn run(
    app: &AppHandle,
    combined: CombinedSession,
    captured_at: DateTime<Utc>,
    total_seconds: f64,
) -> Result<PathBuf, String> {
    match combined {
        CombinedSession::Spilled(spilled) => run_spilled(
            app,
            &spilled.mic_path,
            &spilled.system_path,
            spilled.sample_rate,
            captured_at,
            total_seconds,
        ),
        CombinedSession::InMemory(session) => {
            run_in_memory(app, &session, captured_at, total_seconds)
        }
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
    total_seconds: f64,
) -> Result<PathBuf, String> {
    let mut mic =
        Resampling16kSource::open(mic_path, source_rate).map_err(|err| err.to_string())?;
    let mut system =
        Resampling16kSource::open(system_path, source_rate).map_err(|err| err.to_string())?;
    // A second, independent reader over the same spill file: the AEC
    // reference stream. It must be a distinct `AudioSource` instance from
    // `system` above (not a shared handle) because the You and Them channels
    // are transcribed one after the other, each fully draining its source —
    // sharing one reader between the reference and the Them channel would
    // have the second consumer see an already-drained stream.
    let mut system_reference =
        Resampling16kSource::open(system_path, source_rate).map_err(|err| err.to_string())?;
    let mut cleaned_mic =
        EchoCancelledSource::new(&mut mic, &mut system_reference).map_err(|err| err.to_string())?;
    let mut channels: [(Channel, &mut dyn AudioSource); 2] = [
        (Channel::You, &mut cleaned_mic),
        (Channel::Them, &mut system),
    ];
    let path = persist(app, &mut channels, captured_at, total_seconds)?;
    // Retain the recording now, while the spill files still exist — the
    // callers remove the in-flight directory only after this returns. The
    // staging temp lives inside that directory, so a crash mid-write leaves
    // it where the directory's own cleanup (or recovery's idempotency check)
    // sweeps it. The WAV keeps the 48 kHz capture rate and the raw (pre-AEC)
    // channels: it exists to check the summary against the source, not to
    // feed an engine.
    retain_audio(
        kodabi_audio::write_stereo_wav_from_spills(
            mic_path,
            system_path,
            source_rate,
            &mic_path.with_file_name(AUDIO_STAGING_NAME),
            &kodabi_core::naming::audio_sibling(&path),
        ),
        &path,
    );
    Ok(path)
}

/// The in-memory fallback (spill files couldn't be created): resample both
/// channels up front and feed them in fixed-size chunks.
fn run_in_memory(
    app: &AppHandle,
    session: &AlignedSession,
    captured_at: DateTime<Utc>,
    total_seconds: f64,
) -> Result<PathBuf, String> {
    let mic_pcm = session
        .channel_resampled(SessionChannel::Mic, ENGINE_SAMPLE_RATE_HZ)
        .map_err(|err| err.to_string())?;
    let system_pcm = session
        .channel_resampled(SessionChannel::System, ENGINE_SAMPLE_RATE_HZ)
        .map_err(|err| err.to_string())?;
    let mut mic = SliceSource::new(&mic_pcm, IN_MEMORY_CHUNK_SAMPLES);
    let mut system = SliceSource::new(&system_pcm, IN_MEMORY_CHUNK_SAMPLES);
    // See `run_spilled`'s matching comment: the AEC reference must be its own
    // reader, independent of the one the Them channel drains.
    let mut system_reference = SliceSource::new(&system_pcm, IN_MEMORY_CHUNK_SAMPLES);
    let mut cleaned_mic =
        EchoCancelledSource::new(&mut mic, &mut system_reference).map_err(|err| err.to_string())?;
    let mut channels: [(Channel, &mut dyn AudioSource); 2] = [
        (Channel::You, &mut cleaned_mic),
        (Channel::Them, &mut system),
    ];
    let path = persist(app, &mut channels, captured_at, total_seconds)?;
    // Retain the recording from the resident 48 kHz channels (`channel`, not
    // `channel_resampled` — the WAV keeps capture fidelity). The staging temp
    // is a `.tmp` sibling in the sessions directory, invisible to both the
    // retention sweep and the needs-attention list until the atomic rename.
    retain_audio(
        kodabi_audio::write_stereo_wav_from_channels(
            session.channel(SessionChannel::Mic),
            session.channel(SessionChannel::System),
            session.sample_rate(),
            &path.with_file_name(format!(".audio.{}.tmp", std::process::id())),
            &kodabi_core::naming::audio_sibling(&path),
        ),
        &path,
    );
    Ok(path)
}

/// Logs a failed recording write and moves on: the transcript is the primary
/// artifact and is already safely on disk, so audio retention is best-effort —
/// a failure here must never fail the pipeline (the note simply shows no
/// recording). One accepted gap, documented rather than closed: a crash after
/// the `.jsonl` lands but before the WAV rename loses only the audio, because
/// recovery's idempotency check sees the transcript and removes the spill
/// without converting.
fn retain_audio(result: io::Result<()>, session_path: &Path) {
    if let Err(err) = result {
        eprintln!(
            "audio retention failed for {}: {err}",
            session_path.display()
        );
    }
}

/// Load the glossary and device identity and run the core transcribe → clean →
/// persist pipeline over `channels` (each already yielding 16 kHz mono `f32`),
/// republishing the pipeline's progress as `transcription:state` events against
/// `total_seconds` as the denominator.
fn persist(
    app: &AppHandle,
    channels: &mut [(Channel, &mut dyn AudioSource)],
    captured_at: DateTime<Utc>,
    total_seconds: f64,
) -> Result<PathBuf, String> {
    let kb_dir = knowledge_base_dir(app)?;
    let glossary = Glossary::load(&kb_dir).map_err(|err| err.to_string())?;
    let device = app.state::<DeviceId>().inner().clone();

    let cleaner = ClaudeRunner::new(ClaudeConfig::cleanup_from_env());
    // Resolved once per session rather than per channel: the pipeline builds an
    // engine for each of You and Them, and both must load the same files.
    let stt_paths = crate::models::resolve_stt_paths(app);
    let mut make_engine = || build_engine(&stt_paths);

    // The core counts every second it feeds an engine; the channels are
    // transcribed one after the other, so dividing by their count turns that
    // back into a position within the recording. The clamp covers the rounding
    // either side of the resampler, and `Cleaning` pins the bar full for the
    // stages that have no fraction at all (the headless Claude call, the
    // write) rather than leaving it frozen just short of the end.
    let channel_count = (channels.len() as f64).max(1.0);
    let mut on_progress = |progress: PipelineProgress| {
        let seconds_processed = match progress {
            PipelineProgress::Transcribing { seconds } => {
                (seconds / channel_count).min(total_seconds)
            }
            PipelineProgress::Cleaning => total_seconds,
        };
        let _ = app.emit(
            TRANSCRIPTION_STATE_EVENT,
            TranscriptionStateEvent::Transcribing {
                seconds_processed,
                total_seconds,
            },
        );
    };

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
        &mut on_progress,
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

/// Echo canceller frame size (20 ms at [`ENGINE_SAMPLE_RATE_HZ`]) —
/// speexdsp's documented sweet spot is 10-20 ms.
const AEC_FRAME_SIZE: usize = (ENGINE_SAMPLE_RATE_HZ as usize / 1000) * 20;

/// Echo canceller tail length (256 ms at [`ENGINE_SAMPLE_RATE_HZ`]) — well
/// past the acoustic delay between a speaker and the microphone that hears
/// it, which is typically under 250 ms even for a wireless setup.
const AEC_FILTER_LENGTH: usize = (ENGINE_SAMPLE_RATE_HZ as usize / 1000) * 256;

/// cpal/dasp's f32-PCM convention (also used by `kodabi_audio::convert`):
/// full-scale is ±1.0, scaled against `i16::MIN`'s magnitude so the two
/// conversions are exact inverses at zero and never overflow on the clamp.
const PCM_I16_SCALE: f32 = 32768.0;

/// Converts `src` to `i16` into `dst`, zero-padding any remainder of `dst`
/// beyond `src`'s length — the shared tail-frame and silent-reference padding
/// [`EchoCancelledSource`] needs, in one pass.
fn f32_to_i16_into(src: &[f32], dst: &mut [i16]) {
    for (out, &sample) in dst.iter_mut().zip(src.iter()) {
        *out = (sample * PCM_I16_SCALE).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
    }
    for out in dst.iter_mut().skip(src.len()) {
        *out = 0;
    }
}

/// Wraps a mic [`AudioSource`] and cancels the echo of a reference (the
/// system/loopback channel) [`AudioSource`] out of it — the fix for the
/// acoustic crosstalk a speaker-and-microphone setup produces (the mic
/// picking up what the speakers play, duplicating the Them channel's speech
/// onto You). Both inputs must already be [`ENGINE_SAMPLE_RATE_HZ`] mono
/// `f32`.
///
/// Yields exactly as many samples, in the same order, as `mic` would have on
/// its own: the canceller only ever cleans a frame, it never resizes the
/// timeline the You channel's segment timestamps are measured against. A
/// reference that runs short (or is silent throughout, e.g. a mic-only
/// capture) is padded with silence, which is speexdsp's natural no-op — the
/// output degrades to a passthrough of `mic` rather than failing.
struct EchoCancelledSource<'a> {
    mic: &'a mut dyn AudioSource,
    reference: &'a mut dyn AudioSource,
    canceller: EchoCanceller,
    /// This call's mic samples, copied out of `mic`'s borrowed buffer so
    /// `mic` can be read again (`fill_reference`, the next call) while this
    /// buffer is still being processed.
    mic_scratch: Vec<f32>,
    /// Reference samples pulled ahead of what's been consumed so far —
    /// `mic`'s and `reference`'s upstream chunk boundaries rarely line up.
    reference_buffer: Vec<f32>,
    reference_finished: bool,
    mic_frame: Vec<i16>,
    reference_frame: Vec<i16>,
    out_frame: Vec<i16>,
    out: Vec<f32>,
}

impl<'a> EchoCancelledSource<'a> {
    fn new(mic: &'a mut dyn AudioSource, reference: &'a mut dyn AudioSource) -> io::Result<Self> {
        let canceller =
            EchoCanceller::new(ENGINE_SAMPLE_RATE_HZ, AEC_FRAME_SIZE, AEC_FILTER_LENGTH)
                .map_err(io::Error::other)?;
        Ok(EchoCancelledSource {
            mic,
            reference,
            canceller,
            mic_scratch: Vec::new(),
            reference_buffer: Vec::new(),
            reference_finished: false,
            mic_frame: vec![0i16; AEC_FRAME_SIZE],
            reference_frame: vec![0i16; AEC_FRAME_SIZE],
            out_frame: vec![0i16; AEC_FRAME_SIZE],
            out: Vec::new(),
        })
    }

    /// Tops up `reference_buffer` to at least `needed` samples, or until the
    /// reference source is exhausted.
    fn fill_reference(&mut self, needed: usize) -> io::Result<()> {
        while self.reference_buffer.len() < needed && !self.reference_finished {
            match self.reference.next_chunk()? {
                Some(chunk) => self.reference_buffer.extend_from_slice(chunk),
                None => self.reference_finished = true,
            }
        }
        Ok(())
    }
}

impl AudioSource for EchoCancelledSource<'_> {
    fn next_chunk(&mut self) -> io::Result<Option<&[f32]>> {
        {
            // Scoped so `mic_chunk`'s borrow of `self.mic` ends here — the
            // rest of this method needs to borrow other fields of `self`.
            let mic_chunk = match self.mic.next_chunk()? {
                Some(chunk) => chunk,
                None => return Ok(None),
            };
            self.mic_scratch.clear();
            self.mic_scratch.extend_from_slice(mic_chunk);
        }

        self.fill_reference(self.mic_scratch.len())?;

        self.out.clear();
        let mut offset = 0;
        while offset < self.mic_scratch.len() {
            let frame_len = AEC_FRAME_SIZE.min(self.mic_scratch.len() - offset);
            let ref_len = frame_len.min(self.reference_buffer.len());

            f32_to_i16_into(
                &self.mic_scratch[offset..offset + frame_len],
                &mut self.mic_frame,
            );
            f32_to_i16_into(&self.reference_buffer[..ref_len], &mut self.reference_frame);

            self.canceller
                .cancel(&self.mic_frame, &self.reference_frame, &mut self.out_frame)
                .map_err(io::Error::other)?;
            self.out.extend(
                self.out_frame[..frame_len]
                    .iter()
                    .map(|&s| s as f32 / PCM_I16_SCALE),
            );

            if ref_len > 0 {
                self.reference_buffer.drain(..ref_len);
            }
            offset += frame_len;
        }

        Ok(Some(&self.out))
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
fn build_engine(
    paths: &crate::models::ResolvedSttPaths,
) -> transcription::Result<Box<dyn TranscriptionEngine>> {
    use kodabi_transcribe::{ParakeetConfig, ParakeetEngine};

    let config = ParakeetConfig {
        encoder: paths.encoder.clone(),
        decoder: paths.decoder.clone(),
        joiner: paths.joiner.clone(),
        tokens: paths.tokens.clone(),
        vad_model: paths.vad_model.clone(),
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

// Whisper stays environment-only. It is a local benchmarking feature that no
// release ships (`pnpm tauri:build` passes `--features parakeet`), so its models
// are not mirrored on the models release and there is nothing to download.
#[cfg(all(feature = "whisper", not(feature = "parakeet")))]
fn build_engine(
    _paths: &crate::models::ResolvedSttPaths,
) -> transcription::Result<Box<dyn TranscriptionEngine>> {
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

// Release builds must ship a real engine. A `MockEngine` release would
// silently "transcribe" placeholder text and never run the distill pass (its
// call sites above are cfg-gated to the real engines), so the flagship
// capture -> note flow would not exist in the shipped binary. Debug builds
// keep the mock, which is what every workspace gate and `pnpm tauri dev` uses.
//
// NOTE: CI's `app` job asserts this guard fires by grepping for the message
// below. Reword it and `.github/workflows/ci.yml` must change with it.
#[cfg(all(
    not(debug_assertions),
    not(any(feature = "parakeet", feature = "whisper"))
))]
compile_error!(
    "release builds must ship a real STT engine: build with `--features parakeet` \
     (canonical: `pnpm tauri:build`, which passes `--features parakeet,embed`); \
     the MockEngine fallback is debug-only"
);

/// Milliseconds the mock engine sleeps per accepted chunk, so a dev build can
/// show a transcription that takes long enough to watch.
///
/// The mock is instantaneous, which means a debug capture finishes its whole
/// pipeline between frames and the Inbox card's progress bar never renders a
/// state worth looking at. Chunks are ten audio-seconds, so a one-minute
/// capture is twelve of them across the two channels: `400` stretches that to
/// roughly five seconds of moving bar. Unset (or unparsable) is zero, which is
/// what the gates and every test run with.
#[cfg(not(any(feature = "parakeet", feature = "whisper")))]
const MOCK_STT_DELAY_ENV: &str = "KODABI_MOCK_STT_DELAY_MS";

#[cfg(not(any(feature = "parakeet", feature = "whisper")))]
fn build_engine(
    _paths: &crate::models::ResolvedSttPaths,
) -> transcription::Result<Box<dyn TranscriptionEngine>> {
    let delay = std::env::var(MOCK_STT_DELAY_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or_default();
    Ok(Box::new(
        transcription::MockEngine::new().with_accept_delay(delay),
    ))
}

/// Reads a model file path from an environment variable. An unset var resolves
/// to an empty path, which fails the engine constructor's own `require_file`
/// check with a clear `TranscriptionError::ModelLoad` rather than panicking
/// here.
///
/// Only the whisper leg still reads paths this way. Parakeet resolves through
/// [`crate::models`], which prefers the downloaded models directory and treats
/// the `PARAKEET_*` variables as an all-or-nothing developer override.
#[cfg(all(feature = "whisper", not(feature = "parakeet")))]
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

    /// The wire shape `src/useTranscriptionState.ts` mirrors. Nothing else
    /// checks it: the invoke-parity test only covers command names, so a
    /// renamed field would otherwise reach the frontend as a silent `undefined`
    /// (`.claude/rules/tauri-command-parity.md`).
    #[test]
    fn transcription_state_event_serializes_the_documented_wire_shape() {
        assert_eq!(
            serde_json::to_value(TranscriptionStateEvent::Queued).unwrap(),
            serde_json::json!({ "status": "queued" })
        );
        assert_eq!(
            serde_json::to_value(TranscriptionStateEvent::Transcribing {
                seconds_processed: 12.5,
                total_seconds: 3_484.0,
            })
            .unwrap(),
            serde_json::json!({
                "status": "transcribing",
                "seconds_processed": 12.5,
                "total_seconds": 3_484.0,
            })
        );
        assert_eq!(
            serde_json::to_value(TranscriptionStateEvent::Saved {
                path: "C:\\kb\\sessions\\s.jsonl".to_owned(),
            })
            .unwrap(),
            serde_json::json!({ "status": "saved", "path": "C:\\kb\\sessions\\s.jsonl" })
        );
        assert_eq!(
            serde_json::to_value(TranscriptionStateEvent::Error {
                message: "model missing".to_owned(),
            })
            .unwrap(),
            serde_json::json!({ "status": "error", "message": "model missing" })
        );
    }

    #[test]
    fn spill_seconds_floors_a_truncated_trailing_sample() {
        let path = scratch_path("spill");
        let _ = std::fs::remove_file(&path);
        // 13 bytes: three whole f32 samples plus a crash-truncated fourth.
        std::fs::write(&path, [0u8; 13]).unwrap();

        let seconds = spill_seconds(&path, 3);
        std::fs::remove_file(&path).ok();

        assert_eq!(seconds, 1.0);
    }

    #[test]
    fn spill_seconds_reports_zero_rather_than_a_wrong_denominator() {
        assert_eq!(
            spill_seconds(Path::new("Z:\\no\\such\\spill.pcm"), 48_000),
            0.0
        );

        let path = scratch_path("spill-rate");
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, [0u8; 16]).unwrap();
        let seconds = spill_seconds(&path, 0);
        std::fs::remove_file(&path).ok();
        assert_eq!(seconds, 0.0);
    }

    /// A cheap deterministic pseudo-random generator, scaled into the f32 PCM
    /// range — avoids adding a `rand` dependency for synthetic test audio.
    fn lcg_noise_f32(len: usize, seed: u32) -> Vec<f32> {
        let mut state = seed;
        (0..len)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((state >> 16) as i16 as f32) / (4.0 * PCM_I16_SCALE)
            })
            .collect()
    }

    fn rms(samples: &[f32]) -> f64 {
        let sum_sq: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
        (sum_sq / samples.len() as f64).sqrt()
    }

    fn drain_all(source: &mut dyn AudioSource) -> Vec<f32> {
        let mut out = Vec::new();
        while let Some(chunk) = source.next_chunk().unwrap() {
            out.extend_from_slice(chunk);
        }
        out
    }

    /// The property the You channel's segment timing depends on: cancelling
    /// echo must never grow or shrink the timeline, only clean it.
    #[test]
    fn echo_cancelled_source_preserves_sample_count() {
        // Deliberately not a whole multiple of `AEC_FRAME_SIZE` or of the
        // sources' own chunk size, to exercise the tail-frame padding path.
        let total = AEC_FRAME_SIZE * 50 + 37;
        let mic = lcg_noise_f32(total, 1);
        let reference = lcg_noise_f32(total, 2);
        let mut mic_source = SliceSource::new(&mic, 4000);
        let mut reference_source = SliceSource::new(&reference, 4000);
        let mut cleaned = EchoCancelledSource::new(&mut mic_source, &mut reference_source).unwrap();

        assert_eq!(drain_all(&mut cleaned).len(), total);
    }

    /// The bug this crate exists to fix: a mic channel that is mostly a
    /// delayed, attenuated copy of the reference (acoustic speaker bleed)
    /// must come out the other side with most of that echo removed.
    #[test]
    fn echo_cancelled_source_adapts_out_a_pure_echo() {
        let total_frames = 200; // several seconds — enough for MDF to converge
        let total = total_frames * AEC_FRAME_SIZE;
        let reference = lcg_noise_f32(total, 42);
        let delay = 240; // 15ms at 16kHz, comfortably inside the 256ms tail
        let mut mic = vec![0.0f32; total];
        for i in delay..total {
            mic[i] = reference[i - delay] * 0.5;
        }
        let mut mic_source = SliceSource::new(&mic, 4000);
        let mut reference_source = SliceSource::new(&reference, 4000);
        let mut cleaned = EchoCancelledSource::new(&mut mic_source, &mut reference_source).unwrap();

        let out = drain_all(&mut cleaned);
        let tail = total - ENGINE_SAMPLE_RATE_HZ as usize..total;
        let echo_rms = rms(&mic[tail.clone()]);
        let out_rms = rms(&out[tail]);
        assert!(
            out_rms < echo_rms * 0.5,
            "expected the echo adapted well below the original (echo_rms={echo_rms}, out_rms={out_rms})"
        );
    }

    /// A mic-only capture (no loopback, or a silent one) must degrade to a
    /// passthrough rather than mangling or dropping the near-end signal.
    #[test]
    fn echo_cancelled_source_passes_through_with_silent_reference() {
        let total = AEC_FRAME_SIZE * 20;
        let mic = lcg_noise_f32(total, 7);
        let silence = vec![0.0f32; total];
        let mut mic_source = SliceSource::new(&mic, 4000);
        let mut reference_source = SliceSource::new(&silence, 4000);
        let mut cleaned = EchoCancelledSource::new(&mut mic_source, &mut reference_source).unwrap();

        let out = drain_all(&mut cleaned);
        let mic_rms = rms(&mic);
        let out_rms = rms(&out);
        assert!(
            out_rms > mic_rms * 0.5,
            "expected the near-end signal to mostly survive a silent reference (mic_rms={mic_rms}, out_rms={out_rms})"
        );
    }
}
