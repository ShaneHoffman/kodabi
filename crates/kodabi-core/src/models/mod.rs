//! Model provisioning: what the app needs on disk before it can transcribe or
//! embed, whether it is there, and how to fetch what is missing.
//!
//! Kodabi ships a small installer and downloads its models on first run, so the
//! release build is a few megabytes rather than three quarters of a gigabyte and
//! an app update does not re-ship the models it did not change. The set that has
//! to arrive is described by [`manifest`] — a versioned JSON document compiled
//! into the binary — and provisioning is the two halves below:
//!
//! - [`status`] answers "is it here?" cheaply, by name and byte length, so a
//!   launch or a settings view can ask on every render without hashing 631 MB.
//! - [`download`] fetches what is missing, verifying each file's SHA-256 before
//!   it is allowed to take its final name.
//!
//! **Nothing here reads the environment or touches Tauri.** The shell supplies
//! the models directory, the compiled feature list, and a predicate naming the
//! sets a developer has pointed at their own files; this module supplies the
//! logic and the tests. That split is also what lets `kodabi-embed` keep its
//! promise never to fetch a model at runtime: the app provisions the files, and
//! the crate only ever reads them.

pub mod download;
pub mod manifest;
pub mod status;

pub use download::{
    download, plan, write_notice, DownloadOptions, DownloadPlan, FetchResponse, Fetcher,
    HttpFetcher, Progress,
};
pub use manifest::{License, Manifest, ModelFile, ModelSet};
pub use status::{status, ModelsStatus, SetState, SetStatus};

/// Why provisioning stopped.
///
/// Every variant names the file it concerns, because the user-facing message is
/// this string and "the download failed" is not something anyone can act on.
#[derive(Debug, thiserror::Error)]
pub enum ModelsError {
    /// The compiled-in manifest did not parse, or declares a schema this build
    /// does not know. Only reachable via a hand-edited manifest or a downgrade.
    #[error("the model manifest is unusable: {0}")]
    Manifest(String),
    /// The transfer itself failed: DNS, TLS, a non-success status, a dropped
    /// connection mid-body.
    #[error("could not download {file}: {message}")]
    Http { file: String, message: String },
    /// The bytes arrived intact by length but are not the bytes we asked for.
    /// Treated as hostile, never as "close enough": the partial file is deleted.
    #[error("{file} failed verification (expected sha256 {expected}, got {actual})")]
    ShaMismatch {
        file: String,
        expected: String,
        actual: String,
    },
    /// The server sent a different number of bytes than the manifest promised.
    #[error("{file} is the wrong size (expected {expected} bytes, got {actual})")]
    SizeMismatch {
        file: String,
        expected: u64,
        actual: u64,
    },
    /// A local filesystem failure: no space, no permission, a vanished directory.
    #[error("could not write {file}: {source}")]
    Io {
        file: String,
        #[source]
        source: std::io::Error,
    },
    /// The caller asked to stop. Partial `.part` files are deliberately kept so
    /// the next attempt resumes rather than restarts.
    #[error("the model download was cancelled")]
    Cancelled,
}
