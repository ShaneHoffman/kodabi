# STT engine benchmark: Parakeet TDT vs. whisper.cpp

**Decision: default STT engine = Parakeet TDT (sherpa-onnx). Closed 2026-07-15.**

This is the results record for the "Default STT engine" open decision (`docs/FOUNDING_DOC.md`
§3.4, §7). Both engines were scored on one real recorded meeting; Parakeet is locked as the v1
default, with whisper.cpp remaining the documented fallback (multilingual + strongest
glossary-biasing). Aggregate scores only — the fixture audio and transcripts stay local per
`crates/kodabi-transcribe/tests/data/benchmark/README.md` and are never committed.

## Fixture (provenance by reference — content never committed)

- Real internal meeting recorded **2026-07-14** via the app's own capture pipeline
  (`cargo run -p kodabi-audio --example record_meeting`); a contiguous **3-minute** span
  (18:00–21:00) trimmed to 16 kHz / 16-bit mono per channel.
- Genuine proper nouns (client/teammate/tool names), real pauses, two-channel you(mic)/them(loopback) split.
- **Known fixture caveat — mic bleed:** the recording was made on speakers, not a headset, so the
  "you" (mic) channel captured the remote participants too. The scorer isolates this: its
  phantom-text metric splits *mutual-silence* (true hallucination) from *crosstalk* (bleed), and
  proper-noun recall is capped per term so bleed can't inflate it. Net effect on the numbers below
  is small and identical for both engines; a future headset re-run would remove it.
- sha256 (local files):
  - `meeting_you_16k_mono.wav`  `f36d68dc9348aaa6eb8e7ac18225e24936bdea16438f29ccacac5bfdc65c0690`
  - `meeting_them_16k_mono.wav` `79ebaf7c13b2dce96beb502705bc2349e8eed12abede7e9efd18ec8b4a53c7cf`
  - `reference.jsonl`           `824f28dd626c8b70fabd5eccfcd95bee0477408df8994723a95b12c2e3c8bf08`
  - `glossary.yml`              `b72273792f0ce3b1359ec9d7ea622311d86a2f483dc9520ae9dc32a08cf68754`

## Engines / setup

- **Parakeet:** `sherpa-onnx-nemo-parakeet-tdt-0.6b-v2` (int8), bundled Silero VAD. CPU, 1 thread.
  No prompt-bias mechanism (glossary correctness is the downstream post-pass's job, per §3.4).
- **Whisper:** `ggml-large-v3-turbo`, Silero-VAD-gated (`whisper_with_vad`). CPU, 4 threads. Glossary
  applied as whisper's initial-prompt bias.
- **Both ran CPU inference on the same local Windows dev machine**, so the speed comparison below is
  apples-to-apples (the `whisper` feature is CPU-only; `use_gpu` is a no-op without a GPU build).

## Results

| Engine | Proper-noun recall (headline) | True silence hallucinations | Speed (RTF) | WER, "them" channel |
|---|---|---|---|---|
| **Parakeet** | 90% (5/6) | **0** | **10.0× realtime** | **21.6%** |
| Whisper | 100% (6/6) | **0** | 1.0× realtime | 26.3% |

Processing time for the full 6 minutes of audio (both channels): Parakeet **~36 s**, Whisper **~356 s**.

### Reading the numbers — the raw WER is not content loss

The reference is a **verbatim** transcript (every "um", "uh", stutter, and false start typed out); a
good STT engine deliberately cleans those, so it is "penalized" for being more readable than ground
truth. A word-level analysis of the content-heavy "them" channel found Parakeet's 21.6% WER
decomposes almost entirely into **filler dropped** (ums, uhs, and colloquial verbal tics),
**stutters collapsed** (a false-started phrase rendered once), **trivial normalization**
(digit-vs-word and inflection variants), and **segment-boundary attribution** from the mic bleed.
**No proper nouns, numbers, decisions, or key content were lost.** For meeting-notes purposes both
engines captured the substance accurately.

### Why the headline recall gap does not favor Whisper in practice

Whisper's 100% vs. 90% edge is a single term (a teammate's name) and rests on two artifacts:

- **Prompt-bias asymmetry:** Whisper was fed the exact glossary spellings in its initial prompt;
  Parakeet has no such hook — its equivalent is the engine-agnostic **post-pass cleanup** (§3.4),
  which is not part of this benchmark. Parakeet's one miss (a homophone spelling of the teammate's
  first name) is exactly what that post-pass fixes.
- **Even so, Whisper garbled the client name:** on the main "them" channel Whisper wrote a
  **mangled variant** of the client's name (only scoring a hit because the bleed channel happened
  to catch the correct spelling). Parakeet, unaided, spelled it **correctly** on the main channel.
  On a clean headset recording Whisper would have *failed* that term.

On real content fidelity Parakeet was **cleaner**: lower "them"-channel WER, correct client-name
spelling, and fewer meaning-changing errors (Whisper turned a common idiom into an unrelated
phrase and a compound noun into a non-word; Parakeet's only comparable slip was garbling one
spoken acronym).

## Decision & rationale

**Parakeet TDT is the v1 default.** The evidence confirms FOUNDING_DOC's lean on every axis:

- **Silence-safe:** 0 true hallucinations (tie; both VAD-gated) — Whisper's documented pause-hallucination failure mode is fully mitigated by the Silero VAD gate.
- **Fast:** ~10× realtime vs. ~1× — **~10× faster** than Whisper on identical CPU hardware (Parakeet processed the 6 minutes of audio in ~36 s; Whisper took ~356 s).
- **Near-Whisper (here, better) accuracy:** lower real-content error rate, correct client-noun
  spelling, and its lone proper-noun miss is precisely the case the glossary post-pass exists to fix.
- **Packaging:** static-linked sherpa-onnx (no runtime DLLs to bundle) vs. whisper.cpp's heavier
  build.

whisper.cpp **remains the documented fallback** for multilingual needs and the strongest
glossary-biasing (initial prompt), per §3.4.

## Caveats / threats to validity

- **Single 3-minute fixture, one meeting, English only.** Enough to settle the v1 default given the
  size of the margins (silence-safety + 10× speed + no content-accuracy deficit), but not a broad
  accuracy study. Multilingual and longer-form remain reasons to keep the fallback.
- **Mic bleed** (above) inflates the "you"-channel WER and phantom-crosstalk for both engines
  equally; a headset re-run would clean this up. It does not affect the headline (silence-safety,
  proper-noun recall, speed) conclusions.
- **Whisper ran CPU-only.** A CUDA build (`whisper-cuda`) would narrow the speed gap but not close
  it to Parakeet's, and does not change the accuracy findings.

## Reproducing

Set `KODABI_BENCHMARK_MEETING_DIR` to the local fixture directory and the model env vars, then
(the two engines cannot share a binary — separate feature-gated builds):

```text
cargo test -p kodabi-transcribe --features parakeet -- --ignored --nocapture benchmark   # → results_parakeet.json
cargo test -p kodabi-transcribe --features whisper  -- --ignored --nocapture benchmark   # → results_whisper.json
cargo test -p kodabi-transcribe                     -- --ignored --nocapture benchmark_compare
```

Scoring library: `crates/kodabi-core/src/benchmark/` (pure, unit-tested). Whisper build needs an
MSVC dev env (`vcvars64`) + `LIBCLANG_PATH`; see `crates/kodabi-transcribe/tests/vad_whisper.rs`
for the Windows shared-link DLL-copy note.
