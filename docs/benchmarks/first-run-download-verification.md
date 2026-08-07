# First-run model download: end-to-end verification against `models-v1`

**Result: PASS on every check. Verified 2026-08-07 against the published `models-v1` release.**

`models-v1` was published on 2026-08-07 and, until this run, the app had never downloaded from
it. What had been checked was that the ten URLs resolve (200, and 206 for a range request) — a
different claim from "Kodabi fetches the whole set and passes SHA-256 verification", which is
what every new user hits on first launch. The gap mattered because the first publish of that
release was silently broken and every *local* check passed while it was: `upload-models.ps1`
verified each file's size and digest against the manifest, then published all ten under the wrong
asset names (`gh release upload path#name` sets a display label, not the name), so the app asked
for `parakeet-tdt-0.6b-v2-int8.encoder.int8.onnx` and got a 404. Fixed in `fefb177`; the script
now reads the published names back. The class of bug was "we verified the wrong side", so this
record exercises the published side.

## What was run

A **release-profile** build with the shipping feature set, driven programmatically over CDP:

```
cargo build -p kodabi --release --features tauri/custom-protocol,parakeet,embed --locked
```

Debug builds use `MockEngine` and no embedder (`enabled_features()` returns `[]`, so
`model_status` reports `ready: true` and the nudge never renders) — they cannot exercise this at
all. The only delta from the shipping binary is a compile-time `additionalBrowserArgs` carrying a
CDP debug port, plus an unreachable updater endpoint so the release check could not race the
model nudge. The download, verify and resume code paths are byte-identical to shipping.

Each launch set `KODABI_SANDBOX` to a throwaway base, so `<base>/.models` started genuinely
empty. The harness **deletes** `PARAKEET_ENCODER/DECODER/JOINER/TOKENS/VAD_MODEL` and
`KODABI_EMBED_MODEL_DIR` from the child environment: with any of those set the corresponding set
reports `env_overridden`, the app is `ready`, and the whole verification would have "passed"
without fetching a byte. The preflight asserts no set is `env_overridden` before anything else
happens.

## Baseline (clean sandbox, before any download)

| Check | Observed |
| --- | --- |
| `model_status.ready` | `false` |
| Sets reporting `env_overridden` | none |
| `bytesRequired` | `795640427` — exactly the manifest sum |
| `bytesPresent` | `0` |
| `<base>/.models` | absent |
| First-run nudge | "Set up transcription", button **"Download 796 MB"** |
| Startup embedding sweep | `stopping startup embedding sweep: … cannot read embedding model file …bge-small-en-v1.5\model.onnx` |

That last line is the negative control for the embedder: it must turn into a success once the
files land, and it does (below).

## Checks

### Every file lands and passes SHA-256; no 404 for any asset name

All **10 files across the three sets** downloaded, each emitting a `verifying` event, terminating
in `ready`. Zero `retrying` events across the whole run — no transport retry, and no 404 (a
missing asset name would have surfaced as a terminal `error`, which is exactly how the original
mislabelling would have presented).

The files were then re-hashed **independently with `certutil -hashfile … SHA256`**, not the app's
own hasher, against `crates/kodabi-core/src/models/manifest.json`:

| | Result |
| --- | --- |
| Files checked | 10 |
| Size mismatches | 0 |
| SHA-256 mismatches | 0 |
| Bytes on disk | `795640427` (matches the manifest sum exactly) |
| Leftover `.part` files | 0 |
| `NOTICE.txt` written | yes |

Layout is as the manifest declares, including the one easy-to-get-wrong case: `silero_vad.onnx`
sits at the models-dir **root** (its set's `dir` is empty), not in a subfolder.

This was confirmed on **two independent downloads** — the interrupted run below and a separate
clean run — both hashing clean.

### Resume works after a hard kill

The app was killed with `taskkill /T /F` mid-transfer of the 652 MB encoder, then relaunched.

- Killed with the `.part` at ~357 MB; **377,421,824 bytes** survived on disk.
- On relaunch, `model_status` reported the parakeet set `partial` with `bytesPresent`
  `377421824`, and the nudge offered **"Download 418 MB"** (796 − 378).
- The first `downloading` event after the restart carried
  `file_received: 377421824` — the resume began exactly at the surviving byte count rather than
  at zero. This is the 206 range request working against the real GitHub CDN, which had never
  been exercised.

### Cancel leaves a resumable partial, not a corrupt file

`cancel_model_download` (via the nudge's "Cancel download") was issued with the encoder `.part`
at ~134 MB.

- `downloading` went `false` within ~0.5 s, terminal event `cancelled`.
- The `.part` **survived** at `133,839,105` bytes and was stable afterwards (identical on two
  samples 3 s apart) — the download stopped rather than being abandoned mid-write.
- `model_status` reported the set `partial` with `bytesPresent` exactly `133839105`.
- Resuming produced a first `downloading` event at `file_received: 133839105`, and the next at
  `133855489` (+16,384 — one chunk appended).

Across the entire interrupted run — one cancel, one hard kill, one relaunch — the encoder
`.part` **never shrank once**, and peaked at exactly `652184296`, the manifest size, before being
renamed into place. The bytes assembled across both interruptions still hashed clean, which is
the stronger form of the SHA-256 result above.

### The nudge and the Settings Models row agree throughout

| Moment | Nudge | Settings → Models |
| --- | --- | --- |
| Clean sandbox | "Download 796 MB" | download offered |
| After cancel at 134 MB | "Download 662 MB" | partial |
| After kill at 377 MB | "Download 418 MB" | partial |
| Complete | "Models ready. Transcription and search are fully available." | **"Installed"** |

The remaining figure tracks bytes already on disk at every step, and the Settings row read
"Installed" — **not** "Developer override", confirming the run genuinely used the download path
rather than an environment override.

## The models are usable, not merely present

**Engine level**, pointed at the freshly downloaded files:

- `cargo test -p kodabi-transcribe --features parakeet --test parakeet_real -- --ignored` —
  4 passed, 0 failed (with the five `PARAKEET_*` variables aimed into `<base>/.models`).
- `cargo test -p kodabi-embed --features bge -- --ignored` — 2 passed, 0 failed, including
  `semantically_similar_notes_rank_nearer_than_an_unrelated_one`.

**Through the app**, with no override set:

- The startup embedding sweep that failed in the baseline now runs clean — bge loaded from the
  downloaded directory.
- **Semantic search.** The query `sprinkler equipment supplier quote` returned
  "Q3 budget and irrigation contractor shortlist" at **rank 1**. Not one of those four words
  occurs anywhere in that note (asserted against the note file on disk, not assumed), so FTS5
  cannot have produced the hit — it is necessarily the vector arm.
- **Real capture.** A capture recorded through the app transcribed the committed speech fixture
  played back over the speakers, `transcription:state` going `transcribing → saved`:

  ```
  {"index":0,"channel":"them","start_ms":2342,"end_ms":3590,"text":"Testing 1-2-3."}
  {"index":2,"channel":"them","start_ms":4422,"end_ms":7558,
   "text":"This is a short recording for the Kautama transcription engine."}
  ```

  Both channels carry the audio because it was played on speakers, so the mic leg heard it too.
  The fixture's proper noun ("Kodama", the pre-rename project name) comes back as
  "Kautama"/"Conova" — ordinary out-of-vocabulary behaviour for a proper noun, not a defect.

## Timing — the number for the README

One clean, uninterrupted first-run download, which is the path a new user actually takes:

| | |
| --- | --- |
| Wall clock, click to "Models ready." | **17.3 s** |
| First byte after the click | 521 ms |
| Effective throughput | 45.9 MB/s (≈ 367 Mbit/s) |
| Payload | 795,640,427 bytes across 10 files |

That figure **includes** SHA-256 verification of all ten files, not transfer alone. It is
link-limited and should be quoted as such: the same 796 MB is roughly **1 minute on 100 Mbit**
and **4–5 minutes on 25 Mbit**. The interrupted run's three active segments summed to 16.4 s,
consistent with the clean run — resuming cost nothing beyond the bytes already fetched.

## Notes for whoever touches this next

- **The size is 796 MB, not 760.** The manifest sums to 795,640,427 bytes; `formatMegabytes` is
  decimal, so the UI renders "796 MB". "759 MB" and "~760 MB" are the same quantity in MiB.
  `README.md` currently says "~760 MB" and `ModelDownloadNudge.tsx`'s doc comment says "760 MB",
  neither matching what the app displays. Worth reconciling on the README install pass so a user
  reading the page and a user reading the dialog see one number.
- **`src-tauri/tauri.e2e.conf.json` cannot be used with a release build.** Its updater endpoint is
  `http://`, which Tauri accepts in debug but rejects in release
  (`The configured updater endpoint must use a secure protocol like https`), panicking at
  startup. A release-profile CDP run needs an `https` variant of that overlay.
- **A webview invoke can arrive before `.manage()` has registered state.** One cold launch
  answered `acknowledge_consent` with
  `state not managed for field 'state' on command 'acknowledge_consent'`; a retry two seconds
  later succeeded. Unrelated to model provisioning, and it did not recur, but it is a real
  startup race rather than harness noise.
- Nothing here is automated. There is no test that touches the real network, deliberately — this
  document is the record, and the check to repeat is the one at the top of
  `scripts/upload-models.ps1`: verify a fresh install downloads and passes verification before
  announcing a build.
