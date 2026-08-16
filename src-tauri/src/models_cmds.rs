//! Thin Tauri commands over model provisioning. The manifest, the readiness
//! rules and the download itself live in `kodabi_core::models`; this module owns
//! the serde IPC DTOs, the managed state, and the worker thread that turns core
//! progress callbacks into `models:state` events.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use kodabi_core::models::{self, DownloadOptions, HttpFetcher, ModelsStatus, Progress};
use tauri::{AppHandle, Emitter};

use crate::events::MODELS_STATE_EVENT;
use crate::sandbox;

/// Progress of a first-run model download, as a tagged-status payload. Mirrors
/// the `index:state` / `transcription:state` shape the frontend already reads.
#[derive(Clone, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum ModelsStateEvent {
    Downloading {
        file: String,
        file_index: usize,
        file_count: usize,
        file_received: u64,
        file_total: u64,
        overall_received: u64,
        overall_total: u64,
    },
    /// Hashing a finished file. Its own state because verifying 631 MB takes
    /// seconds during which byte progress is legitimately frozen, and a bar that
    /// simply stopped would read as a hang.
    Verifying {
        file: String,
    },
    Retrying {
        file: String,
        attempt: u32,
        max_attempts: u32,
        message: String,
    },
    Ready,
    Cancelled,
    Error {
        message: String,
    },
}

/// What `model_status` returns. The core status plus the two things only the
/// shell knows.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatusDto {
    #[serde(flatten)]
    status: ModelsStatus,
    /// Whether a download is running right now, so a view that mounts mid-run
    /// shows progress instead of offering to start a second one.
    downloading: bool,
    /// Shown in Settings so a user can find the files. Display only.
    models_dir: String,
}

/// Guards the single download worker.
#[derive(Default)]
pub struct ModelsState {
    /// Set for the lifetime of a worker thread. A second `download_models` while
    /// this is set is a no-op rather than an error: the events already describe
    /// what is happening, and a double-click is not a failure.
    running: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}

/// Reports what is on disk. Cheap enough to call on every mount: it stats files,
/// it does not hash them.
#[tauri::command]
pub fn model_status(
    app: AppHandle,
    state: tauri::State<'_, ModelsState>,
) -> Result<ModelStatusDto, String> {
    let manifest = models::manifest::embedded().map_err(|err| {
        crate::user_errors::reported(
            "model_manifest",
            err,
            "Kodabi's model list couldn't be read. Reinstall Kodabi to repair it.",
        )
    })?;
    let models_dir = sandbox::models_dir(&app)?;
    let status = models::status(
        &manifest,
        &models_dir,
        &crate::models::enabled_features(),
        &crate::models::env_overridden,
    );
    Ok(ModelStatusDto {
        status,
        downloading: state.running.load(Ordering::Relaxed),
        models_dir: models_dir.display().to_string(),
    })
}

/// Starts fetching whatever is missing. Returns as soon as the worker is
/// spawned; progress arrives on the `models:state` event.
#[tauri::command]
pub fn download_models(app: AppHandle, state: tauri::State<'_, ModelsState>) -> Result<(), String> {
    let manifest = models::manifest::embedded().map_err(|err| {
        crate::user_errors::reported(
            "model_manifest",
            err,
            "Kodabi's model list couldn't be read. Reinstall Kodabi to repair it.",
        )
    })?;
    let models_dir = sandbox::models_dir(&app)?;

    if state
        .running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        // Already downloading. The running worker's events are the truth.
        return Ok(());
    }
    // Only after winning the guard: clearing it first would let a second call
    // arriving between a cancel and the worker noticing it wipe the request,
    // leaving the user watching a download they had already stopped.
    state.cancel.store(false, Ordering::Relaxed);

    let running = Arc::clone(&state.running);
    let cancel = Arc::clone(&state.cancel);
    std::thread::spawn(move || {
        let features = crate::models::enabled_features();
        let plan = models::plan(
            &manifest,
            &models_dir,
            &features,
            &crate::models::env_overridden,
        );
        let options = DownloadOptions::default();
        let fetcher = HttpFetcher::new();
        let result = models::download(&plan, &fetcher, &options, &cancel, &mut |progress| {
            let _ = app.emit(MODELS_STATE_EVENT, event_for(progress));
        });

        let terminal = match result {
            Ok(()) => {
                // Rewritten on every success so it always describes what is
                // actually installed. Best-effort: a missing NOTICE is not worth
                // failing a download that otherwise landed.
                let sets = manifest.required_sets(&features);
                if let Err(err) = models::write_notice(&models_dir, &sets) {
                    eprintln!("kodabi: could not write the model NOTICE file: {err}");
                }
                ModelsStateEvent::Ready
            }
            Err(models::ModelsError::Cancelled) => ModelsStateEvent::Cancelled,
            Err(err) => {
                eprintln!("kodabi: model download failed: {err}");
                // A checksum or size mismatch discarded the file, so retrying is
                // a fresh attempt; everything else resumes from what landed.
                let message = match err {
                    models::ModelsError::ShaMismatch { .. }
                    | models::ModelsError::SizeMismatch { .. } => {
                        "A downloaded file didn't verify, so it was discarded. Try the download \
                         again."
                    }
                    _ => {
                        "The download didn't finish. Nothing else was affected; trying again picks \
                         up where it stopped."
                    }
                };
                ModelsStateEvent::Error {
                    message: message.to_string(),
                }
            }
        };
        // Cleared before the terminal event, so a UI that immediately re-reads
        // `model_status` never sees a finished run still marked as running.
        running.store(false, Ordering::SeqCst);
        let _ = app.emit(MODELS_STATE_EVENT, terminal);
    });
    Ok(())
}

/// Asks the worker to stop. Partial files are kept, so starting again resumes.
#[tauri::command]
pub fn cancel_model_download(state: tauri::State<'_, ModelsState>) -> Result<(), String> {
    state.cancel.store(true, Ordering::Relaxed);
    Ok(())
}

/// Translates a core progress callback into the wire payload. `FileStarted` has
/// no event of its own: it is a `Downloading` at the file's resumed offset,
/// which is what the UI needs to render anyway.
fn event_for(progress: Progress<'_>) -> ModelsStateEvent {
    match progress {
        Progress::FileStarted {
            file,
            index,
            count,
            resumed_from,
            total,
            overall_received,
            overall_total,
        } => ModelsStateEvent::Downloading {
            file: file.to_string(),
            file_index: index,
            file_count: count,
            file_received: resumed_from,
            file_total: total,
            overall_received,
            overall_total,
        },
        Progress::Bytes {
            file,
            index,
            count,
            file_received,
            file_total,
            overall_received,
            overall_total,
        } => ModelsStateEvent::Downloading {
            file: file.to_string(),
            file_index: index,
            file_count: count,
            file_received,
            file_total,
            overall_received,
            overall_total,
        },
        Progress::Verifying { file } => ModelsStateEvent::Verifying {
            file: file.to_string(),
        },
        Progress::Retrying {
            file,
            attempt,
            max_attempts,
            message,
        } => ModelsStateEvent::Retrying {
            file: file.to_string(),
            attempt,
            max_attempts,
            message,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact JSON the frontend destructures.
    ///
    /// Worth pinning here rather than trusting either side's types: `src/models.ts`
    /// mirrors this shape by hand, the flatten-plus-`camelCase` combination is
    /// easy to get subtly wrong, and neither the Rust tests nor the vitest suite
    /// would notice a renamed key — the vitest ones build their fixtures from the
    /// TypeScript type, so they would agree with each other and both be wrong.
    #[test]
    fn the_status_dto_serializes_to_the_keys_the_frontend_reads() {
        let dto = ModelStatusDto {
            status: ModelsStatus {
                ready: false,
                bytes_required: 762,
                bytes_present: 131,
                sets: vec![models::SetStatus {
                    id: "parakeet-tdt-0.6b-v2-int8".to_string(),
                    state: models::SetState::Partial,
                    bytes_total: 631,
                    bytes_present: 131,
                    license: "CC-BY-4.0".to_string(),
                }],
            },
            downloading: true,
            models_dir: r"C:\app\.models".to_string(),
        };

        let json: serde_json::Value = serde_json::to_value(&dto).expect("serializes");
        assert_eq!(json["ready"], false);
        assert_eq!(json["bytesRequired"], 762);
        assert_eq!(json["bytesPresent"], 131);
        assert_eq!(json["downloading"], true);
        assert_eq!(json["modelsDir"], r"C:\app\.models");

        let set = &json["sets"][0];
        assert_eq!(set["id"], "parakeet-tdt-0.6b-v2-int8");
        assert_eq!(set["state"], "partial");
        assert_eq!(set["bytesTotal"], 631);
        assert_eq!(set["bytesPresent"], 131);
        assert_eq!(set["license"], "CC-BY-4.0");
    }

    /// The event union is tagged by `status`, and its fields stay snake_case —
    /// `src/models.ts` reads `file_received`, not `fileReceived`.
    #[test]
    fn the_progress_event_serializes_to_the_tagged_shape_the_frontend_matches_on() {
        let json: serde_json::Value = serde_json::to_value(ModelsStateEvent::Downloading {
            file: "parakeet-tdt-0.6b-v2-int8/encoder.int8.onnx".to_string(),
            file_index: 2,
            file_count: 5,
            file_received: 10,
            file_total: 20,
            overall_received: 300,
            overall_total: 762,
        })
        .expect("serializes");
        assert_eq!(json["status"], "downloading");
        assert_eq!(json["file_index"], 2);
        assert_eq!(json["file_count"], 5);
        assert_eq!(json["overall_received"], 300);
        assert_eq!(json["overall_total"], 762);

        for (event, tag) in [
            (ModelsStateEvent::Ready, "ready"),
            (ModelsStateEvent::Cancelled, "cancelled"),
        ] {
            let json: serde_json::Value = serde_json::to_value(event).expect("serializes");
            assert_eq!(json["status"], tag);
        }
    }
}
