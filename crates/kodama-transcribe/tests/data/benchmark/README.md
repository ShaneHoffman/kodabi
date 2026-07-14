# Benchmark fixture: real recorded meeting

Status: **not yet populated.** This directory documents the manifest a real-meeting benchmark
fixture must have; the audio and reference transcript themselves still need to be recorded and
hand-checked during an actual consented meeting (a step no automated agent can perform — it requires
real participants and a human verifying the transcript against the audio).

## Why this exists

FOUNDING_DOC §3.4/§7/§8: the default transcription engine (Parakeet TDT vs whisper.cpp
large-v3-turbo) is locked by benchmarking both **on one real recorded meeting** — genuine proper
nouns, real pauses, the real headset/loopback two-channel mix — not synthetic audio. This fixture is
that one real meeting; `chore/engine-benchmark` scores both engines against it.

## How to produce it

1. **Get consent and announce the recording** before it starts (FOUNDING_DOC §3.7; MA and many states
   require two-party consent). Prefer a real *internal* meeting (team sync, project standup) with
   genuine project/teammate/tool proper nouns over a client call — that keeps the fixture honest
   without committing client-confidential content into this AGPL repo.
2. **Record it** through the real capture pipeline:
   ```sh
   cargo run -p kodama-audio --example record_meeting -- ./out/meeting
   ```
   This drives the same loopback + mic + combiner path the app uses and writes
   `./out/meeting_you_them.wav` (48 kHz stereo master, L=mic/you, R=loopback/them),
   `./out/meeting_you.wav`, and `./out/meeting_them.wav` (48 kHz mono per channel). Confirm the
   printed summary shows both channels non-silent before ending the session.
3. **Trim and downsample** a representative, proper-noun-dense segment (a few minutes) of the two
   mono files to 16 kHz / 16-bit — what the engines consume, and small enough to commit without Git
   LFS (matching the existing `speech_16k_mono.wav` fixture). For example, with ffmpeg:
   ```sh
   ffmpeg -i meeting_you.wav -ss <start> -to <end> -ar 16000 -sample_fmt s16 meeting_you_16k_mono.wav
   ffmpeg -i meeting_them.wav -ss <start> -to <end> -ar 16000 -sample_fmt s16 meeting_them_16k_mono.wav
   ```
4. **Transcribe each channel separately and hand-check every proper noun** against the audio —
   proper-noun accuracy is this benchmark's headline metric, so `reference.jsonl` must be exact
   there. Merge the two channels' segments by `start_ms` (as `kodama_core::raw_session::assemble`
   does) into one `reference.jsonl`.
5. **List the proper nouns** used (project/client/teammate/tool names) in `glossary.yml`.
6. **Fill in `PROVENANCE.md`** (template below) and commit the four files plus this README.

## Committed files (once produced)

- `meeting_you_16k_mono.wav`, `meeting_them_16k_mono.wav` — the benchmark inputs.
- `reference.jsonl` — the transcript-of-record: newline-delimited `TranscriptSegment` JSON, one
  object per line, matching `crates/kodama-core/src/raw_session.rs` and
  `docs/MCP_TOOL_SURFACE.md` field-for-field:
  ```json
  {"index": 0, "channel": "you", "speaker": null, "start_ms": 0, "end_ms": 1200, "text": "..."}
  ```
  `channel` is `"you"` (mic) or `"them"` (loopback); `speaker` is always `null` in v1 (no
  diarization — the two-channel split is the substitute).
- `glossary.yml` — proper nouns present, in the project-glossary shape
  (`crates/kodama-core/src/glossary.rs`):
  ```yaml
  terms:
    - term: MERIDIAN
      definition: ""
      aliases: []
  ```
- `PROVENANCE.md` — who consented and to what, recording date, duration, channel layout, sample
  rate, and the sha256 of each committed WAV. The full 48 kHz stereo master stays author-held (not
  committed) — record its location and sha256 here too, in case a higher-fidelity re-run is ever
  needed. Note the retention posture this fixture is an intentional exception to (§3.7: raw
  audio/transcripts are meant to be discardable after N days; this one is kept indefinitely as a
  benchmark input, by explicit consent).

## Loading convention

Once populated, load fixtures the same way `crates/kodama-transcribe/tests/parakeet_real.rs` and
`whisper_real.rs` load `speech_16k_mono.wav`: a `CARGO_MANIFEST_DIR`-relative path, read with
`hound::WavReader`, `#[ignore]`d and run manually (see those tests' module doc-comments) — this
crate's native engines aren't in CI.
