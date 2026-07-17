//! Build script for `kodabi-transcribe`.
//!
//! On Windows, the `whisper` feature builds sherpa-onnx in its `shared` link
//! mode, whose upstream build script copies the sherpa/ONNX Runtime runtime DLLs
//! (`onnxruntime.dll`, `sherpa-onnx-c-api.dll`, ...) into `target/<profile>/` and
//! `target/<profile>/examples/` — but **not** into `target/<profile>/deps/`,
//! where `cargo test` and bench binaries actually run. A binary in `deps/` does
//! not find `onnxruntime.dll` in its own directory (Windows DLL search position
//! #1), so the loader falls back to a stale system-wide
//! `C:\Windows\System32\onnxruntime.dll` (an old Windows-ML/DirectML build, ORT
//! 1.17.1, API <=17) that ranks ahead (#2) of the correct DLL on `PATH` (last).
//! `sherpa-onnx-c-api.dll` is built against a newer ORT (API 27), so loading
//! 1.17.1 aborts the process with a `STATUS_ACCESS_VIOLATION` at VAD init.
//!
//! This script mirrors those DLLs from `target/<profile>/` into
//! `target/<profile>/deps/` so the correct `onnxruntime.dll` sits in the test
//! binary's own directory (#1) and wins over System32 — automating the manual
//! workaround documented in `tests/vad_whisper.rs`. It is a best-effort
//! convenience: any failure is reported via `cargo:warning` and never fails the
//! build.
//!
//! Ordering matters: `target/<profile>/` is populated by sherpa-onnx-sys's own
//! build script, so ours must run *after* it. Cargo only orders build scripts
//! through a direct dependency on a `links` crate, so `Cargo.toml` takes a direct
//! optional dependency on `sherpa-onnx-sys` (which sets `links = "sherpa-onnx"`)
//! for exactly that reason — without it the two scripts race and this one can run
//! first and find nothing.

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

type BuildError = Box<dyn std::error::Error>;

fn main() {
    // Only the `whisper` feature drives sherpa-onnx into `shared` link mode (and
    // so produces runtime DLLs); `vad`-alone and `parakeet` link statically, with
    // nothing to copy. `whisper-cuda` enables `whisper`, so it is covered too.
    if env::var_os("CARGO_FEATURE_WHISPER").is_none() {
        return;
    }
    // The DLL-search collision is Windows-only. Guard on the *target* OS, not the
    // build host.
    if env::var("CARGO_CFG_TARGET_OS").ok().as_deref() != Some("windows") {
        return;
    }

    if let Err(err) = mirror_runtime_dlls_into_deps() {
        // A convenience copy must never break the build.
        println!(
            "cargo:warning=kodabi-transcribe: could not mirror sherpa-onnx runtime DLLs into deps/: {err}"
        );
    }
}

fn mirror_runtime_dlls_into_deps() -> Result<(), BuildError> {
    let profile_dir = profile_dir()?;
    let deps_dir = profile_dir.join("deps");
    fs::create_dir_all(&deps_dir)?;

    let mut saw_dll = false;
    for entry in fs::read_dir(&profile_dir)? {
        let source = entry?.path();
        if !is_runtime_dll(&source) {
            continue;
        }
        let Some(name) = source.file_name() else {
            continue;
        };
        saw_dll = true;
        let dest = deps_dir.join(name);

        // Re-copy when the upstream DLL changes (e.g. a sherpa/ORT bump refreshes
        // `target/<profile>/`) or when the mirrored copy is deleted — but stay
        // idle when neither moved, so this does not thrash on every build.
        println!("cargo:rerun-if-changed={}", source.display());
        println!("cargo:rerun-if-changed={}", dest.display());

        // Best-effort *per file*: overwriting a currently-loaded DLL (e.g. while
        // a test binary is running) fails on Windows. Warn and keep going so one
        // locked or unreadable DLL never blocks mirroring the rest — above all
        // `onnxruntime.dll`, whose absence is the access violation this fixes.
        if let Err(err) = copy_file_atomically(&source, &dest) {
            println!(
                "cargo:warning=kodabi-transcribe: could not mirror {} into deps/: {err}",
                name.to_string_lossy()
            );
        }
    }

    if !saw_dll {
        // With no runtime DLL found we emit no per-file `rerun-if-changed`
        // above, so Cargo would fall back to "rerun if a package file changed" —
        // which never fires here, stranding `deps/` empty across later builds
        // even after the upstream build script populates `target/<profile>/`.
        // Watch the profile directory so a DLL appearing there re-runs us.
        println!("cargo:rerun-if-changed={}", profile_dir.display());
        println!(
            "cargo:warning=kodabi-transcribe: found no runtime DLLs in {} to mirror into deps/ (the sherpa-onnx shared build may not have populated it — expected under a custom Cargo profile, whose output dir the upstream copy step does not locate)",
            profile_dir.display()
        );
    }

    Ok(())
}

/// The sherpa/ONNX Runtime runtime DLLs the shared build stages into
/// `target/<profile>/`: `onnxruntime.dll`, `onnxruntime_providers_*.dll`,
/// `sherpa-onnx-c-api.dll`, `sherpa-onnx-cxx-api.dll`. Restricting to these
/// name prefixes keeps the mirror from copying — and clobbering — unrelated
/// DLLs other workspace crates may drop into the shared profile directory.
fn is_runtime_dll(path: &Path) -> bool {
    if path.extension() != Some(OsStr::new("dll")) {
        return false;
    }
    match path.file_name().and_then(OsStr::to_str) {
        Some(name) => {
            let name = name.to_ascii_lowercase();
            name.starts_with("onnxruntime") || name.starts_with("sherpa-onnx")
        }
        None => false,
    }
}

/// `target/<profile>/` — resolved *structurally* from `OUT_DIR`, which Cargo
/// lays out as `<target>/<profile>/build/<pkg>-<hash>/out`. Taking the parent
/// of the `build` component yields the profile directory without matching on the
/// `PROFILE` env var — which only ever holds `debug`/`release` and so cannot
/// locate a custom profile's `target/<name>/` output dir. Because `OUT_DIR`
/// already sits under the real target directory, this inherits `CARGO_TARGET_DIR`
/// and `--target <triple>` layouts for free.
///
/// Caveat: the upstream sherpa-onnx-sys copy step *does* key off `PROFILE`, so
/// under a custom profile it will not have populated `target/<name>/` and the
/// mirror finds nothing to copy (a warning, not a build failure).
fn profile_dir() -> Result<PathBuf, BuildError> {
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    out_dir
        .ancestors()
        .find(|path| path.file_name() == Some(OsStr::new("build")))
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            format!(
                "could not locate the profile directory above OUT_DIR ({}): no `build` component",
                out_dir.display()
            )
            .into()
        })
}

/// Copy `src` over `dst` via a temp file + rename so a concurrent reader never
/// observes a torn DLL. Mirrors sherpa-onnx-sys's `copy_file_atomically`.
/// Overwriting a *currently-loaded* DLL fails on Windows; the caller treats that
/// (like any error here) as best-effort and only warns.
fn copy_file_atomically(src: &Path, dst: &Path) -> Result<(), BuildError> {
    let mut temp_name = dst.as_os_str().to_owned();
    temp_name.push(".part");
    let temp = PathBuf::from(temp_name);

    if temp.exists() {
        let _ = fs::remove_file(&temp);
    }
    fs::copy(src, &temp)?;
    fs::rename(&temp, dst)?;
    Ok(())
}
