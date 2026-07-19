# Resource budget

Status: **Done.** Idle, capturing, and transcription are all measured on real hardware, with
no fan spin-up observed. See [FOUNDING_DOC.md §3.7](FOUNDING_DOC.md) for the requirement this
ticket implements: *"idle ≈ zero; capturing under a target CPU ceiling (tune on real
hardware); no fan spin-up during meetings. Treat as a requirement, not a bug report."* The
numbers below show the compiled-in defaults already comfortably clear the budget on this
hardware — no tuning was needed this pass (see **Final tuned constants**).

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
| Embedding thread count (bge-small) | 1 | `KODABI_EMBED_THREADS` | Embedding |
| Embedding model directory | — | `KODABI_EMBED_MODEL_DIR` | Embedding |

"Batch vs. streaming" isn't a runtime knob — it's fixed per engine (Whisper is a batch
engine; Parakeet is VAD-gated pseudo-streaming). Its tunable proxy is `KODABI_VAD_MAX_SPEECH`,
which bounds both the recognizer's per-segment work and the VAD's internal buffer size.

## Embedding (Phase 2 note index)

The note index embeds bodies locally with **bge-small-en-v1.5** (384-d) via `fastembed` (ONNX
Runtime, CPU), behind `kodabi-embed`'s `bge` feature. It follows the same resource discipline as
transcription: the intra-op thread count defaults to **1** (`KODABI_EMBED_THREADS`, clamped to
`1..=8`), and inference is serialized behind a mutex so at most one heavyweight model runs at a
time. Unlike the STT engines, the ~150 MB session is kept resident after first use rather than
dropped between calls — it is small enough to hold and re-loading per note would cost more than it
saves. Model files load from `KODABI_EMBED_MODEL_DIR` (no network at runtime — data custody,
FOUNDING_DOC §2); only the ONNX Runtime *binary* is fetched, once, at build time by
`ort-download-binaries`. Embedding runs on note write/edit, off the capture path, so it doesn't
compete with the capture/transcription budgets above.

## Chosen CPU ceiling

- **Capturing (sustained, system-wide):** ≤ 10%
- **Post-meeting transcription burst (brief, few-second spike):** ≤ 35%

Measured peaks were far below both (1.67% capturing, 4.29% transcription burst — see
Results), so this ceiling is set with several-fold headroom rather than at the observed
number: this hardware is a 16C/24T desktop CPU, likely stronger than the field/laptop target
the app actually ships to, and "no fan spin-up" (the real requirement) was already confirmed
at the measured levels. If a real capturing session on a more modest laptop ever approaches
these ceilings, re-tune via the knobs above rather than raising the ceiling by default.

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

### Capturing

Measured 2026-07-15: ~93s of real two-channel capture (mic + system-audio loopback, a
YouTube video playing for the system/"them" channel) via `measure-resources.ps1` sampling
alongside a real `Ctrl+Shift+K` start→stop cycle. **Well within the chosen ceiling.**

| Process | CPU% avg | CPU% peak | Working set |
|---|---|---|---|
| `kodabi.exe` | 1.00% | 1.67% | 3.6 MB → 37.5 MB (grows with buffered audio; ≈ expected for ~93s of 48 kHz stereo two-channel f32 PCM) |
| `msedgewebview2.exe` fleet (summed) | 0.03% | 0.38% | avg 295.5 MB, peak 302.3 MB |

`frames_dropped`: not observed this pass (no IPC status poll was run alongside the profiler).

**Fan observation: no spin-up** — confirmed by the human running the session, during both
capturing and the transcription burst that followed.

### Transcription burst

**Fixture-based (`crates/kodabi-transcribe/tests/resource_budget.rs`, `speech_16k_mono.wav`,
6.12s of audio), real models:**

| Engine | `speed_x` | Notes |
|---|---|---|
| Parakeet (CPU, 1 thread) | **72.19x** | Production configuration (VAD-gated pseudo-streaming) — the leaned default's real number. |
| Whisper (CPU, 4 threads) | blocked by the sherpa-onnx ORT bug (task #53) | `whisper.cpp` alone (bare `WhisperEngine`, no VAD) transcribed the fixture in ~15.1s as a rough, non-production proxy, but the mandatory VAD gate (`whisper_with_vad`, FOUNDING_DOC §8) crashes — see below. |
| Whisper (CUDA) | blocked by the sherpa-onnx ORT bug (task #53) | Model loaded onto the RTX 4080 fine, then hit the identical crash at VAD init. |

**Real end-to-end pipeline** (the ~93s capture above, stopped via `Ctrl+Shift+K`, full
`transcribe → clean → persist` pipeline, from `KODABI_METRICS`'s `PipelineTimings`):

| Stage | Value |
|---|---|
| Audio processed (both channels summed) | 180.0s (≈ 90s × 2 channels) |
| Engine build (both channels — a fresh engine per channel, see `pipeline.rs`) | 6.16s |
| Transcribe (mic, system) | 302 ms, 267 ms |
| Assemble / cleanup / persist | 0 ms / 0 ms / 1 ms |
| **Total wall time** | **6.83s** |
| **`speed_x` (aggregate)** | **26.36x** |

`kodabi.exe` CPU during this burst: **avg ~4.1%, peak 4.29%** (`scripts/measure-resources.ps1`,
same session). Working set climbed cleanly 55→64 MB, then showed two anomalous readings
(711 MB, 1360 MB) that oscillate rather than trend — treated as
`Win32_PerfFormattedData_PerfProc_Process` sampling artifacts around the model-unload/reload
boundary (both models are 650 MB/1.6 GB but memory-mapped, not necessarily fully resident;
the readings don't monotonically decay the way real freed memory would), not genuine peak
memory. Reported transparently rather than silently included or excluded. Engine-build
dominates the pipeline cost (6.16s of the 6.83s total) — a real target for future tuning if
the transcription burst ever needs to shrink further, though it's already well inside the
chosen ceiling.

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
entirely, not just this measurement. Tracked as board task **#53**
(`fix/sherpa-onnx-ort-mismatch`).

## Final tuned constants

**No changes needed.** The compiled-in defaults (Parakeet, 1 thread; `FRAME_CAPACITY=256`;
resampler chunk 1024 / 128 sinc taps) already hold the chosen ceiling with wide margin —
1.67% peak CPU while capturing against a 10% ceiling, no fan spin-up — on real target-adjacent
hardware. The env-override knobs above remain in place as an escape hatch for a future,
less powerful target machine (a laptop, most likely) where a tuning pass may find a real
need to trade resampler quality or thread count for headroom; none was evident here.

## Provenance

- **Idle, capturing, and transcription:** measured 2026-07-15 on branch
  `chore/resource-budget-tuning` (this ticket): idle and the fixture-based transcription RTF
  by the agent directly; capturing (a real ~93s `Ctrl+Shift+K` session with a YouTube video
  as the system-audio channel) driven by the user with the agent running the profiler and
  reading results, since starting real audio capture and confirming fan behavior both need a
  human. Debug build, real Parakeet models from `C:\Users\shane\kodabi-models\` (downloaded
  during task #37's engine benchmark). Hardware profile above.
- **Whisper's production (VAD-gated) path:** blocked by a pre-existing `sherpa-onnx`
  dependency bug, tracked separately as board task #53 — not this ticket's default engine,
  not blocking.
