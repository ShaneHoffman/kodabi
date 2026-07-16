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

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use kodabi_audio::{AlignedSession, SessionChannel};
use kodabi_core::device::DeviceId;
use kodabi_core::glossary::Glossary;
use kodabi_core::metrics::PipelineTimings;
use kodabi_core::pipeline::transcribe_and_persist;
use kodabi_core::transcription::{self, Channel, TranscriptionEngine};
use kodabi_llm::{ClaudeCleaner, ClaudeConfig};
use tauri::{AppHandle, Emitter, Manager};

/// All engines expect mono `f32` PCM at this rate.
const ENGINE_SAMPLE_RATE_HZ: u32 = 16_000;

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
const MIN_TRANSCRIBE_DURATION: Duration = Duration::from_secs(1);

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
pub fn spawn_transcription(app: &AppHandle, session: AlignedSession) {
    if session.duration() < MIN_TRANSCRIBE_DURATION {
        return;
    }

    let app = app.clone();
    // Stamp the meeting's start now — right after stop — rather than when this
    // thread finally acquires the lock and finishes model load + cleanup, any
    // of which can lag `stop` by minutes.
    let elapsed_ms = session.duration().as_millis() as i64;
    let captured_at = Utc::now() - chrono::Duration::milliseconds(elapsed_ms);

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
        let event = match run(&app, &session, captured_at) {
            Ok(path) => TranscriptionStateEvent::Saved {
                path: path.display().to_string(),
            },
            Err(message) => {
                eprintln!("transcription pipeline failed: {message}");
                TranscriptionStateEvent::Error { message }
            }
        };
        let _ = app.emit(TRANSCRIPTION_STATE_EVENT, event);
    });
}

/// Resolves the knowledge-base root — the plain, user-syncable folder that
/// holds sessions, glossaries, and (later) routed notes (FOUNDING_DOC's
/// "plain folder" model). This is a placeholder location: today it is the
/// app-data dir, but it is the single seam a future vault-path setting
/// replaces, so every KB path derives from here rather than calling
/// `app_data_dir()` inline. Per-project subfolders — each with their own
/// `_glossary.yml` — will hang off this once routing lands.
fn knowledge_base_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|err| format!("failed to resolve knowledge base directory: {err}"))
}

/// Resamples both channels to 16 kHz, loads the project glossary and device
/// identity, and runs the pure `kodabi-core` pipeline against `captured_at`
/// (stamped by the caller at stop, not here). Errors collapse to a message
/// string — the same convention `audio_cmds` uses for IPC results — since the
/// only consumer here is the `transcription:state` event.
fn run(
    app: &AppHandle,
    session: &AlignedSession,
    captured_at: DateTime<Utc>,
) -> Result<PathBuf, String> {
    let kb_dir = knowledge_base_dir(app)?;
    let glossary = Glossary::load(&kb_dir).map_err(|err| err.to_string())?;
    let device = app.state::<DeviceId>().inner().clone();

    let mic = session
        .channel_resampled(SessionChannel::Mic, ENGINE_SAMPLE_RATE_HZ)
        .map_err(|err| err.to_string())?;
    let system = session
        .channel_resampled(SessionChannel::System, ENGINE_SAMPLE_RATE_HZ)
        .map_err(|err| err.to_string())?;
    let channels = [(Channel::You, mic), (Channel::Them, system)];

    let cleaner = ClaudeCleaner::new(ClaudeConfig::from_env());
    let mut make_engine = build_engine;

    let outcome = transcribe_and_persist(
        &mut make_engine,
        &cleaner,
        &glossary,
        &channels,
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
