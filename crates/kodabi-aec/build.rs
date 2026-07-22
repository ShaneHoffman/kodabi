//! Compiles the vendored speexdsp echo canceller and preprocessor (see
//! `vendor/README.md`) into this crate, no bindgen/CMake/system library
//! required — the whole surface `src/ffi.rs` needs is ~10 functions, so the
//! bindings are hand-written instead of generated.

fn main() {
    let vendor = "vendor/speexdsp";

    cc::Build::new()
        .include(vendor)
        .define("FLOATING_POINT", None)
        .define("USE_KISS_FFT", None)
        // `EXPORT` prefixes every public function (e.g. `EXPORT SpeexEchoState
        // *speex_echo_state_init(...)`) and is normally supplied by the
        // autotools-generated `config.h` we deliberately don't vendor. It must
        // expand to nothing, not to `1` — `-DEXPORT` (no value) would leave
        // `1 SpeexEchoState *speex_echo_state_init(...)`, a syntax error.
        .define("EXPORT", Some(""))
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
