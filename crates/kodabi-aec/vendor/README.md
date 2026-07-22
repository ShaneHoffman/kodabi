# Vendored speexdsp

Source: https://github.com/xiph/speexdsp
Commit: `7a158783df74efe7c2d1c6ee8363c1e695c71226` (2025-07-05)
License: BSD-3-Clause-style, see `COPYING` in this directory.

Only the files the acoustic echo canceller (`mdf.c`) and its residual echo /
noise suppressor (`preprocess.c`) need are vendored, built with the
`USE_KISS_FFT` FFT backend (no external FFT dependency):

- `mdf.c`, `preprocess.c`, `filterbank.c`, `fftwrap.c`, `kiss_fft.c`,
  `kiss_fftr.c` — implementation
- `arch.h`, `os_support.h`, `math_approx.h`, `pseudofloat.h`,
  `filterbank.h`, `fftwrap.h`, `kiss_fft.h`, `kiss_fftr.h`,
  `_kiss_fft_guts.h` — internal headers these files include
- `speex/speex_echo.h`, `speex/speex_preprocess.h`,
  `speex/speexdsp_types.h` — the public API `crates/kodabi-aec/src/ffi.rs`
  binds against

Files are otherwise unmodified from upstream. `resample.c`, `buffer.c`,
`jitter.c`, `scal.c` and the ARM/Blackfin/fixed-point headers are not needed
(this build is `FLOATING_POINT`, x86_64 only) and were not copied.

`kiss_fft`/`kiss_fftr`'s public function names (`kiss_fft_alloc`, `kiss_fft`,
`kiss_fft_stride`, `kiss_fftr_alloc`, `kiss_fftr`, `kiss_fftri`, `kiss_fftr2`,
`kiss_fftri2`) are renamed to a `kodabi_aec_`-prefixed form at compile time
(`../build.rs`, plain `-D` token substitution) — a release build also links
`kodabi-transcribe`'s sherpa-onnx (feature `parakeet`), which bundles its own
copy of kiss_fft under the same symbol names, and two definitions of the same
C symbol in one binary is a link error (`LNK2005`), not a warning. None of
these names are called through `../src/ffi.rs`'s public surface, so renaming
them is invisible outside this crate. If a future upstream sync adds another
externally-visible `kiss_fft*` function, add it to `build.rs`'s rename list
too — a build with `--features parakeet` (`pnpm tauri:build` or
`cargo build -p kodabi --release --features parakeet`) is the only gate that
catches a missed one; `cargo check`/`clippy` do not link and pass either way.

To update: re-clone the commit above (or a newer one), diff the files listed
here against `libspeexdsp/` and `include/speex/` in the upstream tree, and
copy over any that changed.
