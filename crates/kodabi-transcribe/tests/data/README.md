# Test fixture: `speech_16k_mono.wav`

The one committed speech fixture, shared by the four real-model tests that transcribe a short clip
(`parakeet_real.rs`, `whisper_real.rs`, `vad_whisper.rs`, `resource_budget.rs`), each of which
loads it through an identical `CARGO_MANIFEST_DIR`-relative `read_speech_wav()`. The crate's other
real-model tests — `benchmark_parakeet.rs`, `benchmark_whisper.rs`, `benchmark_compare.rs` — score a
full meeting instead and read the separate, uncommitted `benchmark/` fixture; they are unaffected by
anything below. Unlike that fixture next door, this one is committed — so it has to be audio this
repo may redistribute under a licence, which is what this file records.

Source: LibriSpeech ASR corpus, <https://www.openslr.org/12/> — `test-clean` subset, utterance
`6930-75918-0017`
License: Creative Commons Attribution 4.0 International,
<https://creativecommons.org/licenses/by/4.0/> (LibriSpeech (c) 2014 Vassil Panayotov, per the
corpus `LICENSE.TXT`). **This WAV is CC BY 4.0, not AGPL-3.0** — the repository licence does not
cover it, and the attribution below has to travel with the file.

Attribution, from the corpus's own `SPEAKERS.TXT` / `CHAPTERS.TXT` / `BOOKS.TXT`:

- Read by **Nolan Fout** (LibriVox reader 6930)
- Chapter "11 - Night" of the LibriVox recording of *Ten Years Later* by Alexandre Dumas (LibriVox
  project 5863, from Project Gutenberg ebook 2681) — public-domain text, CC BY 4.0 recording
- Corpus citation: V. Panayotov, G. Chen, D. Povey and S. Khudanpur, "LibriSpeech: an ASR corpus
  based on public domain audio books", ICASSP 2015

Spoken content: "BUT IN THIS FRIENDLY PRESSURE RAOUL COULD DETECT THE NERVOUS AGITATION OF A GREAT
INTERNAL CONFLICT"

Properties: 16 kHz, mono, 16-bit PCM, 98,560 frames = 6.16 s, peak -6.6 dBFS. 197,164 bytes,
sha256 `936666f499ebc4d374b1959e2bdf8b314770be30d9d81475594d48222219d0f4`.

The corpus audio (already 16 kHz mono) was only decoded from FLAC to PCM — no trimming,
resampling, gain change or normalisation, so what plays is the corpus utterance unaltered:

```sh
ffmpeg -i 6930-75918-0017.flac -ar 16000 -ac 1 -c:a pcm_s16le \
  -map_metadata -1 -fflags +bitexact -flags:a +bitexact speech_16k_mono.wav
```

(`-map_metadata -1` and the two bitexact flags keep the header free of encoder strings, so the
committed bytes are reproducible.)

To regenerate: fetch the utterance's FLAC either from the Hugging Face `openslr/librispeech_asr`
dataset (rows API, config `clean`, split `test` — the per-row `audio[0].src` URLs are signed and
expire, so re-fetch the row listing each time) or, for the bit-exact official distribution, as
`LibriSpeech/test-clean/6930/75918/6930-75918-0017.flac` out of
<https://www.openslr.org/resources/12/test-clean.tar.gz>. Then run the command above. The reader
and chapter metadata come from <https://www.openslr.org/resources/12/raw-metadata.tar.gz>.

## Replacing this clip

Nothing asserts the transcript, so a different clip is fine — but the chunked-feed regression tests
in `parakeet_real.rs` and `vad_whisper.rs` build a `3s silence + clip + 1s silence + clip` timeline
and assert absolute session-clock milliseconds against it. A replacement therefore has to be:

- 16 kHz, mono, 16-bit PCM (asserted directly in each `read_speech_wav()`)
- **6.0-6.5 s long** — shorter than ~6.0 s drops the second utterance's onset below the `>= 9800`ms
  assert or the last end below `> 15_000`ms (parakeet) / `> 14_500`ms (whisper); much longer than
  ~7 s pushes the first utterance out of the first 10 s chunk and stops exercising the VAD
  window-carry bug those tests exist for
- VAD-detectable English speech starting within ~2 s of t=0 (the first-segment assert is
  `2000..=5000`ms) and running to near the clip's end
- free of the words "MERIDIAN" and "TeeTrack", which `whisper_real.rs`'s glossary-bias test relies
  on the fixture *not* saying

Any length change also moves the derived figures in the comments of both chunked-feed tests and the
fixture row in `docs/RESOURCE_BUDGET.md`, and the measured `speed_x` there has to be re-run.
