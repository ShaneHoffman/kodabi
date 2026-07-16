# Resource budget

Status: **scaffold — numbers not yet measured.** See [FOUNDING_DOC.md §3.7](FOUNDING_DOC.md)
for the requirement this ticket implements: *"idle ≈ zero; capturing under a target CPU
ceiling (tune on real hardware); no fan spin-up during meetings. Treat as a requirement,
not a bug report."* There is no pre-set numeric ceiling — it's deliberately left to be
tuned on the real target machine, and this doc is where the measured numbers get recorded.

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

"Batch vs. streaming" isn't a runtime knob — it's fixed per engine (Whisper is a batch
engine; Parakeet is VAD-gated pseudo-streaming). Its tunable proxy is `KODABI_VAD_MAX_SPEECH`,
which bounds both the recognizer's per-segment work and the VAD's internal buffer size.

## Chosen CPU ceiling

_TBD — set after measuring on the target machine. Rationale (why this number, what fan
behavior drove it) goes here._

## Hardware profile

_TBD — CPU model, physical/logical core count, RAM, OS build, power plan, AC vs. battery._

## Results

### Idle

| Process | CPU% avg | CPU% peak | Working set |
|---|---|---|---|
| `kodabi.exe` | _TBD_ | _TBD_ | _TBD_ |
| `msedgewebview2.exe` (summed) | _TBD_ | _TBD_ | _TBD_ |

### Capturing (real meeting)

Meeting length: _TBD_

| Process | CPU% avg | CPU% peak | Working set | `frames_dropped` |
|---|---|---|---|---|
| `kodabi.exe` | _TBD_ | _TBD_ | _TBD_ | _TBD_ |
| `msedgewebview2.exe` (summed) | _TBD_ | _TBD_ | _TBD_ | — |

Fan observation: _TBD_

### Transcription burst

| Engine | `speed_x` | CPU%/GPU% peak | Working set peak |
|---|---|---|---|
| Parakeet (CPU) | _TBD_ | _TBD_ | _TBD_ |
| Whisper (CUDA) | _TBD_ | _TBD_ | _TBD_ |

## Final tuned constants

_TBD — filled in once a configuration is confirmed to hold the budget. `old` is the
compiled-in default before this pass; `new` is what got baked in as the new default (the
env var remains available as an override either way)._

| Knob | Old default | New default | Env var |
|---|---|---|---|
| _TBD_ | | | |

## Provenance

_TBD — commit/build measured, date, who ran it._
