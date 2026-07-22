//! Compiles the vendored speexdsp echo canceller and preprocessor (see
//! `vendor/README.md`) into this crate, no bindgen/CMake/system library
//! required — the whole surface `src/ffi.rs` needs is ~10 functions, so the
//! bindings are hand-written instead of generated.

fn main() {
    let vendor = "vendor/speexdsp";

    let mut build = cc::Build::new();
    build
        .include(vendor)
        .define("FLOATING_POINT", None)
        .define("USE_KISS_FFT", None)
        // `EXPORT` prefixes every public function (e.g. `EXPORT SpeexEchoState
        // *speex_echo_state_init(...)`) and is normally supplied by the
        // autotools-generated `config.h` we deliberately don't vendor. It must
        // expand to nothing, not to `1` — `-DEXPORT` (no value) would leave
        // `1 SpeexEchoState *speex_echo_state_init(...)`, a syntax error.
        .define("EXPORT", Some(""));

    // kiss_fft/kiss_fftr are an extremely commonly-vendored FFT — a release
    // build also links `kodabi-transcribe`'s sherpa-onnx (feature `parakeet`),
    // which bundles its own copy under the same symbol names
    // (`kiss_fftr`/`kiss_fftr_alloc`/`kiss_fftri` collided at link time: LNK2005
    // "already defined"). Since these are internal to `fftwrap.c`'s FFT
    // backend — never called through `src/ffi.rs`'s public surface — they're
    // safe to rename via straight token substitution, the standard fix for two
    // vendored copies of the same C library ending up in one binary.
    for symbol in [
        "kiss_fft_alloc",
        "kiss_fft",
        "kiss_fft_stride",
        "kiss_fft_cleanup",
        "kiss_fftr_alloc",
        "kiss_fftr",
        "kiss_fftri",
        "kiss_fftr2",
        "kiss_fftri2",
    ] {
        build.define(symbol, Some(format!("kodabi_aec_{symbol}").as_str()));
    }

    build
        .file(format!("{vendor}/mdf.c"))
        .file(format!("{vendor}/preprocess.c"))
        .file(format!("{vendor}/filterbank.c"))
        .file(format!("{vendor}/fftwrap.c"))
        .file(format!("{vendor}/kiss_fft.c"))
        .file(format!("{vendor}/kiss_fftr.c"))
        .warnings(false)
        .compile("kodabi_speexdsp");

    println!("cargo:rerun-if-changed={vendor}");
}
