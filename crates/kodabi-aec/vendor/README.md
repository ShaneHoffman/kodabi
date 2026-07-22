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

To update: re-clone the commit above (or a newer one), diff the files listed
here against `libspeexdsp/` and `include/speex/` in the upstream tree, and
copy over any that changed.
