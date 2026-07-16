# Resource budget

Status: **idle + transcription measured on real hardware; capturing + fan observation still
pending a real meeting.** See [FOUNDING_DOC.md §3.7](FOUNDING_DOC.md) for the requirement
this ticket implements: *"idle ≈ zero; capturing under a target CPU ceiling (tune on real
hardware); no fan spin-up during meetings. Treat as a requirement, not a bug report."*
There is no pre-set numeric ceiling — it's deliberately left to be tuned on the real target
machine, and this doc is where the measured numbers get recorded. Idle and transcription
speed are automatable and were measured by the agent; "capturing" (needs a real, consented
meeting) and fan spin-up (needs a human ear) are not — see **How to finish this** below.

Two costs are measured separately, because they happen at different times:

- **Capture** — the all-day steady-state cost. While a meeting is live, only the capture
  path runs: two `cpal` capture threads (mic + loopback) plus one combiner thread running
  two concurrent live resamplers. This is the number `§3.7`'s "capturing under a target CPU
  ceiling" and "no fan spin-up" are really about — it's what's running continuously next to
  real work.
- **Transcription** — a post-meeting burst, not a during-meeting cost. `crates/kodabi-core`'s
  pipeline only runs *after* capture stops (serialized so at most one heavyweight engine is
  ever resident), so its CPU/GPU spike is bounded in time even if it's high.

## `speed_x` convention

Transcription timing is reported as `speed_x = audio_seconds ÷ wall_seconds` — **greater
than 1.0 means faster than realtime.** This is the *inverse* of the conventional real-time
factor (wall ÷ audio); read the ratio, don't assume which direction is "faster" without
checking which convention a given number uses. Computed by
`kodabi_core::metrics::real_time_factor` / `PipelineTimings::speed_x`.

## How to reproduce

1. Build the engine under test with its models available via env vars:
   ```
   cargo build -p kodabi --features parakeet
   # PARAKEET_ENCODER / PARAKEET_DECODER / PARAKEET_JOINER / PARAKEET_TOKENS / PARAKEET_VAD_MODEL

   cargo build -p kodabi --features whisper
   # WHISPER_MODEL / VAD_MODEL
   ```
2. Set `KODABI_METRICS` to a file path (or the literal `stderr`) so each transcription
   run appends one JSON line of `PipelineTimings` + `speed_x` (see
   `src-tauri/src/transcribe.rs`). Set any candidate tuning knobs (below).
3. Launch the app and run `scripts/measure-resources.ps1` alongside it — it samples
   `kodabi.exe` **and** the `msedgewebview2.exe` helper processes (which an in-process
   sampler can't see) via `Get-Counter`, at 1 Hz, and prints avg/peak CPU%/working-set at
   the end.
4. **Idle:** do nothing for ~60s — this is the "idle ≈ zero" baseline.
5. **Capturing:** join a real, consented meeting (§3.7 — two-party consent) for a realistic
   length. Watch the live CPU% and **listen for the fan** — fan spin-up is the felt failure
   mode `§3.7` calls out, not a number on a dashboard.
6. Stop capture — the transcription burst runs and appends a `speed_x` line to
   `KODABI_METRICS`.
7. Iterate the tuning knobs below until capture stays under the chosen ceiling with no fan
   spin-up, then fill in the tables below and bake the winning values in as the new
   defaults.

A repeatable proxy for transcription speed that doesn't need a live meeting:
`crates/kodabi-transcribe/tests/resource_budget.rs` (`#[ignore]`d, model-gated) runs an
engine over the committed `speech_16k_mono.wav` fixture and prints `speed_x`:

```
cargo test -p kodabi-transcribe --features parakeet -- --ignored --nocapture rtf
cargo test -p kodabi-transcribe --features whisper -- --ignored --nocapture rtf
```

## How to finish this

Idle and transcription-speed numbers below were measured by the agent directly (build +
launch + read the profiler output — no privacy/consent concern, nothing physical to sense).
**Capturing** and **fan spin-up** need a human: starting real capture records your actual
microphone and system audio, and "no fan spin-up" is a physically-heard signal an agent has
no way to perceive. To finish:

1. Run `C:\Users\shane\kodabi-models\measure-capture.ps1` (personal, not in the repo — sets
   the Parakeet model env vars + `KODABI_METRICS` and launches the already-built
   `target\debug\kodabi.exe`).
2. Run `scripts\measure-resources.ps1` alongside it and watch the CPU% column live.
3. Press `Ctrl+Shift+K` to start capture during a real, consented meeting (or just talk),
   listen for the fan, press it again to stop.
4. Hand over `metrics.jsonl` + the profiler CSV (or just the avg/peak numbers) to fill in
   the **Capturing** and **Fan observation** rows below.

## Tuning knobs

All env-overridable, applied on top of the compiled-in default — no recompile needed to
iterate. A blank/unparsable/out-of-range value falls back to the default rather than
breaking the pipeline.

| Knob | Default | Env var | Applies to |
|---|---|---|---|
| Per-source capture channel depth | 256 | `KODABI_FRAME_CAPACITY` | Capture |
| Live resampler input chunk | 1024 | `KODABI_RESAMPLE_CHUNK` | Capture |
| Live resampler sinc taps | 128 | `KODABI_RESAMPLE_TAPS` | Capture (main CPU lever) |
| Parakeet thread count (VAD + recognizer) | 1 | `KODABI_PARAKEET_THREADS` | Transcription |
| Whisper thread count | 4 | `KODABI_WHISPER_THREADS` | Transcription |
| Whisper GPU (CUDA) | true | `KODABI_WHISPER_GPU` | Transcription |
| Standalone VAD thread count (fronts Whisper) | 1 | `KODABI_VAD_THREADS` | Transcription |
| VAD speech-probability threshold | 0.5 | `KODABI_VAD_THRESHOLD` | Transcription |
| VAD min silence duration (s) | 0.25 | `KODABI_VAD_MIN_SILENCE` | Transcription |
| VAD min speech duration (s) | 0.25 | `KODABI_VAD_MIN_SPEECH` | Transcription |
| VAD max speech duration (s) | 20.0 | `KODABI_VAD_MAX_SPEECH` | Transcription |

"Batch vs. streaming" isn't a runtime knob — it's fixed per engine (Whisper is a batch
engine; Parakeet is VAD-gated pseudo-streaming). Its tunable proxy is `KODABI_VAD_MAX_SPEECH`,
which bounds both the recognizer's per-segment work and the VAD's internal buffer size.

## Chosen CPU ceiling

_TBD — idle is confirmed negligible (below); the ceiling itself is set once a real-meeting
capturing measurement exists to tune against. Rationale (why this number, what fan behavior
drove it) goes here._

## Hardware profile

- CPU: Intel Core i7-13700K — 16 physical / 24 logical cores
- RAM: 31.8 GB
- GPU: NVIDIA GeForce RTX 4080 (CUDA 13.3 toolkit installed)
- OS: Windows 11 Home, build 26200
- Power plan: Balanced

## Results

### Idle

Measured 2026-07-15: launched the default (no engine feature) build, no capture started,
sampled ~65s via `scripts/measure-resources.ps1`. **Confirms "idle ≈ zero."**

| Process | CPU% avg | CPU% peak | Working set |
|---|---|---|---|
| `kodabi.exe` | 0.00% | 0.00% | 3.1 MB |
| `msedgewebview2.exe` fleet (19 helper processes, summed) | 0.00% | 0.17% | 223.4 MB (peak total) |

The WebView2 fleet's memory is Chromium/WebView2 infrastructure (GPU process, renderer,
network service, crashpad, etc.), not app logic — `kodabi.exe` itself is a thin native host
with the visible work happening in these child processes.

### Capturing (real meeting)

**TBD — needs a human.** See **How to finish this** above.

Meeting length: _TBD_

| Process | CPU% avg | CPU% peak | Working set | `frames_dropped` |
|---|---|---|---|---|
| `kodabi.exe` | _TBD_ | _TBD_ | _TBD_ | _TBD_ |
| `msedgewebview2.exe` (summed) | _TBD_ | _TBD_ | _TBD_ | — |

Fan observation: _TBD_

### Transcription burst

Measured 2026-07-15 against the committed `speech_16k_mono.wav` fixture (6.12s of audio) via
`crates/kodabi-transcribe/tests/resource_budget.rs`, real models
(`C:\Users\shane\kodabi-models\`).

| Engine | `speed_x` | Notes |
|---|---|---|
| Parakeet (CPU, 1 thread) | **72.19x** | Production configuration (VAD-gated pseudo-streaming) — the leaned default's real number. |
| Whisper (CPU, 4 threads) | ~0.41x *(caveated)* | **Not the production configuration** — see the known issue below. `whisper.cpp` alone (bare `WhisperEngine`, no VAD) transcribed the fixture in ~15.1s; the mandatory VAD gate (`whisper_with_vad`, FOUNDING_DOC §8) crashes on this machine, so a true production-path number couldn't be measured. |
| Whisper (CUDA) | — | Blocked by the same issue (see below) — model loaded onto the RTX 4080 fine, then hit the identical crash. |

CPU%/GPU%/working-set peaks during the transcription burst were not separately profiled this
pass (the RTF harness measures wall time only); `scripts/measure-resources.ps1` run alongside
a real capture-then-stop cycle will capture these too.

#### Known issue: sherpa-onnx VAD crashes with an ONNX Runtime version mismatch

Both `whisper_with_vad` (the mandatory production Whisper path) and, separately, the
`whisper-cuda` feature crash on this machine with:

```
The requested API version [27] is not available, only API versions [1, 17] are supported
in this build. Current ORT Version is: 1.17.1
```
`(exit code: 0xc0000005, STATUS_ACCESS_VIOLATION)`

Isolated by running the repo's own pre-existing, unmodified tests: bare `WhisperEngine`
(`whisper_real.rs`, no VAD) **passes** and transcribes real speech correctly; the VAD-gated
`whisper_with_vad` (`vad_whisper.rs`) **crashes** with the error above, on both CPU-only
`whisper` and `whisper-cuda` features. This reproduces with a freshly-cleared build cache
(stale artifacts ruled out) and against the unmodified `main`-equivalent test files (this
ticket's changes ruled out) — it's an internal inconsistency in the `sherpa-onnx` 1.13.4
crate's Windows shared-link artifact: its native library appears to request a newer ONNX
Runtime C API version than the `onnxruntime.dll` it bundles actually implements. Parakeet
(`sherpa-onnx/static` link mode) is unaffected.

This is a real, pre-existing bug independent of this ticket (the `sherpa-onnx` version is
unchanged by this branch) that blocks the production Whisper fallback path on Windows
entirely, not just this measurement — worth its own ticket to reproduce elsewhere and
pin/patch the dependency.

## Final tuned constants

_TBD — filled in once a configuration is confirmed to hold the budget. `old` is the
compiled-in default before this pass; `new` is what got baked in as the new default (the
env var remains available as an override either way)._

| Knob | Old default | New default | Env var |
|---|---|---|---|
| _TBD_ | | | |

## Provenance

- **Idle + transcription (Parakeet + bare Whisper):** measured 2026-07-15 by the agent, on
  branch `chore/resource-budget-tuning` (this ticket), debug build, models from
  `C:\Users\shane\kodabi-models\` (downloaded during task #37's engine benchmark).
- **Capturing + fan observation:** _TBD — pending a human-run real meeting._
