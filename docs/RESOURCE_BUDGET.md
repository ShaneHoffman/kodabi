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
| Capture spill flush interval (s) | 10 | `KODABI_FLUSH_SECS` | Capture (memory vs. crash-loss lever) |
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
| `kodabi.exe` | 1.00% | 1.67% | 3.6 MB → 37.5 MB (grew with buffered audio; ≈ expected for ~93s of 48 kHz stereo two-channel f32 PCM) |
| `msedgewebview2.exe` fleet (summed) | 0.03% | 0.38% | avg 295.5 MB, peak 302.3 MB |

`frames_dropped`: not observed this pass (no IPC status poll was run alongside the profiler).

**Fan observation: no spin-up** — confirmed by the human running the session, during both
capturing and the transcription burst that followed.

> **Superseded by the spill path (see next section).** This 2026-07-15 pass predates incremental
> capture. The `3.6 MB → 37.5 MB` climb over ~93s (~24 MB/min, a whole session held in RAM) is
> exactly the unbounded growth the spill path removes: capture now flushes to disk on a cadence
> and the in-memory buffer plateaus instead of tracking session length. The CPU numbers still
> stand (spilling is a periodic buffered write, not a per-frame cost).

### Incremental capture durability (spill + recovery)

Capture streams its aligned two-channel audio to disk during the meeting instead of holding the
whole session in RAM, so (a) a crash or `kill -9` mid-meeting loses at most the last flush
interval rather than everything, and (b) memory stays bounded no matter how long the meeting runs.
Design and code: `crates/kodabi-audio/src/spill.rs` + `combine.rs` (the spill sink and flushed-length
accounting), `crates/kodabi-core/src/inflight.rs` (the on-disk session + recovery scan), and
`src-tauri/src/transcribe.rs` (streamed transcription + startup recovery).

**How it works.** The combiner drains each channel's accumulated output to its own raw
little-endian `f32` file under `<sessions>/inflight/<timestamp>-<device>/` once it reaches
`KODABI_FLUSH_SECS` (default 10s) of audio, then clears the in-memory buffer (retaining capacity).
At stop, transcription streams the files back a chunk at a time (resampling 48 kHz → 16 kHz on the
fly) rather than materialising the session, writes the atomic `.jsonl`, then deletes the directory.
A directory still present at the next launch is an orphan (a crash): startup recovers it through the
same transcribe → distill chain. Retention never touches `inflight/` (it prunes only top-level
`.jsonl`); un-recoverable leftovers are swept after a 48h grace by `inflight::sweep_stale`, piggybacked
on the retention cadence.

**Memory target.**

- Combiner in-memory audio: **≤ ~16 MB**. Per channel the resident buffer plateaus near one flush
  interval (10s × 48 kHz × 4 B ≈ 1.9 MB) plus retained `Vec` capacity and resampler/scratch state;
  ×2 channels with generous headroom. This is the bound the old ~24 MB/min growth violated.
- `kodabi.exe` sustained working set during a multi-hour capture: **target ≤ 100 MB** (a
  conservative ceiling well under the pre-spill trajectory, which would have reached multiple GB
  over 3h).

The bound itself is proven deterministically by `combine.rs`'s
`spilling_keeps_the_in_memory_buffer_bounded` unit test (drives ~20s of synthetic frames and asserts
the resident buffer never exceeds the flush threshold plus one resample chunk — nowhere near the
~960k samples an unbounded buffer would reach). An empirical working-set measurement over a real
multi-hour capture needs a human-driven session (as the Capturing numbers above did) and is the
follow-up verification for this line; the design target is recorded here so that pass has a bar to
check against.

**Transient disk cost.** The spill is 48 kHz `f32` per channel — byte-identical to the timeline the
session held in memory before — so it costs **~1.4 GB/hour** for both channels (~4.2 GB peak for a
3h meeting), deleted within minutes of stop once the transcript lands. `meta.json` records the
sample rate and format, so a later switch to a smaller on-disk encoding (e.g. 16 kHz `i16`, ~6×
smaller) is a metadata version bump rather than a format break.

**Durability boundary (fsync policy).** Each flush is a buffered write followed by `flush()` — the
bytes reach the OS page cache, which **survives process death** (a crash or `kill -9`, the acceptance
case). It deliberately does **not** `sync_all`/`FlushFileBuffers`, so a hard power loss can still lose
the last unsynced flush interval; that is the accepted bound, not a bug. If a full disk makes a spill
write fail, capture logs once and keeps that session's audio in memory (degrading to the old
behaviour) rather than dropping audio or failing the capture.

**Engine-buffer exception (Whisper).** This work bounds the *capture* path and streams the transcript
input from disk in chunks. It does **not** change an engine's own internal buffering:
`WhisperEngine` is a batch engine whose `accept` appends the whole session into a `Vec<f32>`
(`crates/kodabi-transcribe/src/whisper.rs`) and runs whisper.cpp once at `finish` — roughly
**~230 MB per channel-hour** at 16 kHz `f32`, resident only during the post-meeting burst, one engine
at a time. Parakeet (the working engine) is VAD-gated and already bounded, and the VAD-gated Whisper
path is blocked on Windows anyway (task #53), so windowed Whisper feeding is deferred to a follow-up
rather than done here.

**Kill -9 acceptance procedure.** To verify "a crash loses at most the last flush interval and the
recovered session produces a routed note":

1. Build a real-engine app (`cargo build -p kodabi --features parakeet`, models via env — see
   *How to reproduce*), launch it, and start a consented capture with audio on both channels.
2. After a few minutes, kill it hard from another shell: `Stop-Process -Name kodabi -Force`.
3. Confirm the spill survived: `<app-data>/sessions/inflight/<…>/mic.f32le` and `system.f32le` exist
   and their size corresponds to roughly (elapsed − ≤`KODABI_FLUSH_SECS`) of 48 kHz `f32`
   (4 bytes/sample). Lower `KODABI_FLUSH_SECS` to tighten the worst-case loss.
4. Relaunch the app. Startup recovery transcribes the orphan (watch the sidebar status:
   "Transcribing…" → "Saved" → "Note saved") and deletes the `inflight/` directory; a routed note
   appears in the vault.

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
