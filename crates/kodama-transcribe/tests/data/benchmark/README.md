# Benchmark fixture: real recorded meeting

This directory documents the manifest a real-meeting benchmark fixture must have. **Everything in
here except this README is git-ignored and stays local-only** — see `.gitignore`. Real recorded
audio of real participants doesn't belong in git history (local or on GitHub) even unpushed, so the
fixture itself lives only on the machine that produced it; only the shape it must have is versioned.

## Why this exists

FOUNDING_DOC §3.4/§7/§8: the default transcription engine (Parakeet TDT vs whisper.cpp
large-v3-turbo) is locked by benchmarking both **on one real recorded meeting** — genuine proper
nouns, real pauses, the real headset/loopback two-channel mix — not synthetic audio. This fixture is
that one real meeting; `chore/engine-benchmark` scores both engines against it by reading the local
directory below (env var, not a repo path — see "Loading convention").

## How to produce it

1. **Get consent and announce the recording** before it starts (FOUNDING_DOC §3.7; MA and many states
   require two-party consent). Prefer a real *internal* meeting (team sync, project standup) with
   genuine project/teammate/tool proper nouns over a client call.
2. **Record it** through the real capture pipeline:
   ```sh
   cargo run -p kodama-audio --example record_meeting -- ./out/meeting
   ```
   This drives the same loopback + mic + combiner path the app uses and writes
   `./out/meeting_you_them.wav` (48 kHz stereo master, L=mic/you, R=loopback/them),
   `./out/meeting_you.wav`, and `./out/meeting_them.wav` (48 kHz mono per channel). Confirm the
   printed summary shows both channels non-silent before ending the session.
3. **Trim and downsample** a representative, proper-noun-dense segment (3-5 minutes, one contiguous
   span — not a stitched-together highlight reel, since natural pauses are part of what's being
   benchmarked) of the two mono files to 16 kHz / 16-bit, e.g. with ffmpeg:
   ```sh
   ffmpeg -ss <start_secs> -i meeting_you.wav -t <duration_secs> -ar 16000 -ac 1 -c:a pcm_s16le meeting_you_16k_mono.wav
   ffmpeg -ss <start_secs> -i meeting_them.wav -t <duration_secs> -ar 16000 -ac 1 -c:a pcm_s16le meeting_them_16k_mono.wav
   ```
   (`-ss` before `-i` is a fast, sample-accurate seek for uncompressed PCM; `-t` is an unambiguous
   duration regardless of where `-ss` sits.) Save the outputs into this directory.
4. **Transcribe each channel separately and hand-check every proper noun** against the audio —
   proper-noun accuracy is this benchmark's headline metric, so `reference.jsonl` must be exact
   there. Merge the two channels' segments by `start_ms` (as `kodama_core::raw_session::assemble`
   does) into one `reference.jsonl`.
5. **List the proper nouns** used (project/client/teammate/tool names) in `glossary.yml`.
6. **Fill in `PROVENANCE.md`** (template below). Nothing in this step gets committed — all local.

## Local-only files (git-ignored, never committed)

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
    - term: OKIES
      definition: ""
      aliases: []
  ```
- `PROVENANCE.md` — who consented and to what, recording date, duration, channel layout, sample
  rate, and the sha256 of each local WAV. The full 48 kHz stereo master stays wherever it was
  recorded to (not moved into this directory) — record its location and sha256 here too, in case a
  higher-fidelity re-run is ever needed.

## Loading convention

Because the fixture is never committed, it can't be loaded via a `CARGO_MANIFEST_DIR`-relative path
the way `speech_16k_mono.wav` is. Instead, follow this crate's existing convention for large/
sensitive local-only assets (the uncommitted Parakeet/Whisper model files, wired via `PARAKEET_*` /
`WHISPER_MODEL` env vars in `parakeet_real.rs` / `whisper_real.rs`): `chore/engine-benchmark`'s
harness should read a single env var — e.g. `KODAMA_BENCHMARK_MEETING_DIR` — pointing at this
directory, and load `meeting_you_16k_mono.wav` / `meeting_them_16k_mono.wav` / `reference.jsonl` /
`glossary.yml` from it by fixed filename. Like the model-gated tests, that harness stays
`#[ignore]`d and local-only — it can't run in CI without the fixture.
