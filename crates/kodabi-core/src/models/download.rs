//! Fetching the missing files, safely enough to be killed at any moment.
//!
//! Three properties matter more than speed here, because the app is downloading
//! most of a gigabyte over a connection it does not control:
//!
//! - **Nothing half-written ever takes a real filename.** Bytes land in
//!   `<name>.part` and are renamed only after the SHA-256 matches, so a killed
//!   app cannot leave behind a file that passes the existence check in
//!   [`status`](super::status) and then fails inside the ONNX runtime.
//! - **A second attempt resumes.** `.part` files survive cancellation and
//!   failure on purpose, and the next run asks for `Range: bytes=<len>-`.
//! - **Progress is reported, not guessed.** The caller passes a callback and
//!   gets byte counts for the current file and the run as a whole.
//!
//! The transport is behind [`Fetcher`] so every one of those properties is
//! tested against a scripted fake rather than a network.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use super::manifest::{Manifest, ModelSet};
use super::ModelsError;

/// Suffix for a download in flight. Also read by [`status`](super::status),
/// which counts these bytes as progress but never as readiness.
const PART_SUFFIX: &str = ".part";

/// The attribution file written beside the models.
const NOTICE_FILE: &str = "NOTICE.txt";

/// Where a file's bytes accumulate before it earns its real name.
pub fn part_path(target: &Path) -> PathBuf {
    let mut name = target.as_os_str().to_os_string();
    name.push(PART_SUFFIX);
    PathBuf::from(name)
}

/// A transport that can start a body at an arbitrary offset.
pub trait Fetcher: Send + Sync {
    /// Requests `url`, asking to start at `range_start`.
    ///
    /// Implementations must report through [`FetchResponse::resumed`] whether
    /// the server honoured the range. A server that ignores it and sends the
    /// whole body is not an error, but the caller has to know so it can start
    /// the file over rather than append a second copy.
    fn fetch(&self, url: &str, range_start: u64) -> Result<FetchResponse, ModelsError>;
}

pub struct FetchResponse {
    pub body: Box<dyn Read + Send>,
    /// True when the server answered `206 Partial Content`.
    pub resumed: bool,
}

/// The real transport: blocking HTTP over rustls.
///
/// Blocking on purpose. Every long-running job in this app is a detached
/// `std::thread`, and there is no async runtime to join; a download is the same
/// shape. `ureq` is built here without its default features because `gzip`
/// would decompress the body transparently, which silently breaks the byte
/// arithmetic that resume depends on.
pub struct HttpFetcher {
    agent: ureq::Agent,
}

impl HttpFetcher {
    pub fn new() -> Self {
        let agent = ureq::Agent::config_builder()
            // Only the handshake and the response head are bounded. A total
            // timeout would abort a healthy download of a 631 MB file on a slow
            // connection, which is the case this whole module exists to serve.
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_response(Some(Duration::from_secs(60)))
            .user_agent(concat!("kodabi/", env!("CARGO_PKG_VERSION")))
            .build()
            .new_agent();
        Self { agent }
    }
}

impl Default for HttpFetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Fetcher for HttpFetcher {
    fn fetch(&self, url: &str, range_start: u64) -> Result<FetchResponse, ModelsError> {
        let mut request = self.agent.get(url);
        if range_start > 0 {
            request = request.header("Range", format!("bytes={range_start}-"));
        }
        let response = request.call().map_err(|err| ModelsError::Http {
            file: url.to_string(),
            message: err.to_string(),
        })?;
        // Trust the status, not the request: a redirect chain that dropped the
        // Range header answers 200, and the caller restarts the file rather
        // than appending a second copy of it.
        let resumed = response.status().as_u16() == 206;
        Ok(FetchResponse {
            body: Box::new(response.into_body().into_reader()),
            resumed,
        })
    }
}

/// What the downloader is doing, reported as it happens.
#[derive(Debug, Clone, PartialEq)]
pub enum Progress<'a> {
    FileStarted {
        file: &'a str,
        index: usize,
        count: usize,
        resumed_from: u64,
        total: u64,
    },
    Bytes {
        file: &'a str,
        /// Position of this file in the run, carried on every report so a
        /// consumer never has to remember the last `FileStarted` to render
        /// "file 3 of 5".
        index: usize,
        count: usize,
        file_received: u64,
        file_total: u64,
        overall_received: u64,
        overall_total: u64,
    },
    /// Hashing a finished file. Worth reporting: verifying 631 MB takes seconds
    /// during which byte progress is deliberately frozen.
    Verifying { file: &'a str },
    Retrying {
        file: &'a str,
        attempt: u32,
        max_attempts: u32,
        message: String,
    },
}

#[derive(Debug, Clone)]
pub struct DownloadOptions {
    pub max_attempts: u32,
    /// Waited between attempts, indexed by the attempt just failed. The last
    /// entry repeats if there are more attempts than delays.
    pub backoff: Vec<Duration>,
    /// Floor on the gap between [`Progress::Bytes`] reports. Without it a 64 KiB
    /// chunk size turns a 631 MB file into ten thousand IPC messages.
    pub progress_min_interval: Duration,
    pub chunk_size: usize,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            max_attempts: 4,
            backoff: vec![
                Duration::from_secs(1),
                Duration::from_secs(5),
                Duration::from_secs(15),
            ],
            progress_min_interval: Duration::from_millis(200),
            chunk_size: 64 * 1024,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PlannedFile {
    /// Machine-independent path used in progress and errors, so no message ever
    /// carries a home directory.
    pub display: String,
    pub url: String,
    pub target: PathBuf,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Default)]
pub struct DownloadPlan {
    pub files: Vec<PlannedFile>,
    /// Total size of the files in this plan, which is what overall progress
    /// counts toward.
    pub bytes_total: u64,
}

impl DownloadPlan {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// Works out what to fetch: every file of every required set that is not
/// already on disk at its declared size.
///
/// Environment-overridden sets are skipped entirely — a developer pointing the
/// app at their own copies has not asked us to download anything.
pub fn plan(
    manifest: &Manifest,
    models_dir: &Path,
    enabled_features: &[&str],
    env_overridden: &dyn Fn(&str) -> bool,
) -> DownloadPlan {
    let mut files = Vec::new();
    for set in manifest.required_sets(enabled_features) {
        if env_overridden(&set.id) {
            continue;
        }
        for file in &set.files {
            let target = file.target_path(models_dir, set);
            if std::fs::metadata(&target)
                .is_ok_and(|meta| meta.is_file() && meta.len() == file.size)
            {
                continue;
            }
            files.push(PlannedFile {
                display: set.display_path(file),
                url: file.url(&manifest.base_url),
                target,
                size: file.size,
                sha256: file.sha256.clone(),
            });
        }
    }
    let bytes_total = files.iter().map(|file| file.size).sum();
    DownloadPlan { files, bytes_total }
}

/// Fetches everything in the plan, in order.
///
/// Returns [`ModelsError::Cancelled`] promptly when `cancel` is set; partial
/// files are left in place so the next call resumes.
pub fn download(
    plan: &DownloadPlan,
    fetcher: &dyn Fetcher,
    options: &DownloadOptions,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(Progress<'_>),
) -> Result<(), ModelsError> {
    let count = plan.files.len();
    // Bytes belonging to files this run has already finished. Overall progress
    // is this plus the current file's own count, which keeps the total honest
    // across a restart that truncates a partial file.
    let mut completed_bytes = 0u64;
    for (index, file) in plan.files.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(ModelsError::Cancelled);
        }
        let context = FileContext {
            index,
            count,
            completed_bytes,
            overall_total: plan.bytes_total,
        };
        fetch_with_retries(file, &context, fetcher, options, cancel, on_progress)?;
        completed_bytes += file.size;
    }
    Ok(())
}

/// Where one file sits in the run as a whole.
struct FileContext {
    index: usize,
    count: usize,
    completed_bytes: u64,
    overall_total: u64,
}

fn fetch_with_retries(
    file: &PlannedFile,
    context: &FileContext,
    fetcher: &dyn Fetcher,
    options: &DownloadOptions,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(Progress<'_>),
) -> Result<(), ModelsError> {
    let max_attempts = options.max_attempts.max(1);
    for attempt in 1..=max_attempts {
        let outcome = fetch_once(file, context, fetcher, options, cancel, on_progress);
        let error = match outcome {
            Ok(()) => return Ok(()),
            // Cancellation is the caller's decision, not a transport failure.
            Err(ModelsError::Cancelled) => return Err(ModelsError::Cancelled),
            Err(error) => error,
        };
        if attempt == max_attempts {
            return Err(error);
        }
        on_progress(Progress::Retrying {
            file: &file.display,
            attempt,
            max_attempts,
            message: error.to_string(),
        });
        let index = (attempt as usize - 1).min(options.backoff.len().saturating_sub(1));
        let delay = options.backoff.get(index).copied().unwrap_or_default();
        if !sleep_cancellable(delay, cancel) {
            return Err(ModelsError::Cancelled);
        }
    }
    // `max_attempts` is at least 1, so the loop always returns.
    Err(ModelsError::Cancelled)
}

fn fetch_once(
    file: &PlannedFile,
    context: &FileContext,
    fetcher: &dyn Fetcher,
    options: &DownloadOptions,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(Progress<'_>),
) -> Result<(), ModelsError> {
    let part = part_path(&file.target);
    if let Some(parent) = file.target.parent() {
        std::fs::create_dir_all(parent).map_err(|source| ModelsError::Io {
            file: file.display.clone(),
            source,
        })?;
    }

    let mut present = file_len(&part).unwrap_or(0);
    if present > file.size {
        // Longer than promised, so it is not a prefix of what we want.
        let _ = std::fs::remove_file(&part);
        present = 0;
    }

    on_progress(Progress::FileStarted {
        file: &file.display,
        index: context.index,
        count: context.count,
        resumed_from: present,
        total: file.size,
    });

    if present < file.size {
        let response = fetcher.fetch(&file.url, present)?;
        // A server that ignored the range restarts the body at zero; appending
        // it to what we already have would produce a plausible-length file that
        // is entirely wrong, which is the failure this branch exists to avoid.
        let resume_from = if response.resumed && present > 0 {
            present
        } else {
            0
        };
        let sink = if resume_from == 0 {
            std::fs::File::create(&part)
        } else {
            std::fs::OpenOptions::new().append(true).open(&part)
        }
        .map_err(|source| ModelsError::Io {
            file: file.display.clone(),
            source,
        })?;
        stream_to_file(
            response.body,
            sink,
            resume_from,
            file,
            context,
            options,
            cancel,
            on_progress,
        )?;
    }

    let written = file_len(&part).unwrap_or(0);
    if written != file.size {
        // A short file is a truncated transfer and stays put, so the retry
        // resumes it. An overlong one is garbage and goes.
        if written > file.size {
            let _ = std::fs::remove_file(&part);
        }
        return Err(ModelsError::SizeMismatch {
            file: file.display.clone(),
            expected: file.size,
            actual: written,
        });
    }

    on_progress(Progress::Verifying {
        file: &file.display,
    });
    let actual = sha256_file(&part, options.chunk_size, cancel).map_err(|err| match err {
        HashError::Cancelled => ModelsError::Cancelled,
        HashError::Io(source) => ModelsError::Io {
            file: file.display.clone(),
            source,
        },
    })?;
    if !actual.eq_ignore_ascii_case(&file.sha256) {
        let _ = std::fs::remove_file(&part);
        return Err(ModelsError::ShaMismatch {
            file: file.display.clone(),
            expected: file.sha256.clone(),
            actual,
        });
    }

    // Windows `rename` refuses to replace an existing file, and a stale one can
    // be here: a wrong-sized leftover is exactly why this file was replanned.
    let _ = std::fs::remove_file(&file.target);
    std::fs::rename(&part, &file.target).map_err(|source| ModelsError::Io {
        file: file.display.clone(),
        source,
    })
}

#[allow(clippy::too_many_arguments)]
fn stream_to_file(
    mut body: Box<dyn Read + Send>,
    mut sink: std::fs::File,
    resume_from: u64,
    file: &PlannedFile,
    context: &FileContext,
    options: &DownloadOptions,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(Progress<'_>),
) -> Result<(), ModelsError> {
    let mut buffer = vec![0u8; options.chunk_size.max(1)];
    let mut received = resume_from;
    let mut last_report: Option<Instant> = None;

    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = sink.flush();
            return Err(ModelsError::Cancelled);
        }
        let read = body.read(&mut buffer).map_err(|err| ModelsError::Http {
            file: file.display.clone(),
            message: err.to_string(),
        })?;
        if read == 0 {
            break;
        }
        sink.write_all(&buffer[..read])
            .map_err(|source| ModelsError::Io {
                file: file.display.clone(),
                source,
            })?;
        received += read as u64;

        let now = Instant::now();
        if last_report.is_none_or(|last| now.duration_since(last) >= options.progress_min_interval)
        {
            last_report = Some(now);
            on_progress(bytes_progress(file, context, received));
        }
    }

    sink.flush().map_err(|source| ModelsError::Io {
        file: file.display.clone(),
        source,
    })?;
    // Always land on the true final count, whatever the rate limiter skipped.
    on_progress(bytes_progress(file, context, received));
    Ok(())
}

fn bytes_progress<'a>(file: &'a PlannedFile, context: &FileContext, received: u64) -> Progress<'a> {
    Progress::Bytes {
        file: &file.display,
        index: context.index,
        count: context.count,
        file_received: received,
        file_total: file.size,
        overall_received: context.completed_bytes + received,
        overall_total: context.overall_total,
    }
}

enum HashError {
    Cancelled,
    Io(std::io::Error),
}

fn sha256_file(path: &Path, chunk_size: usize, cancel: &AtomicBool) -> Result<String, HashError> {
    let mut file = std::fs::File::open(path).map_err(HashError::Io)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; chunk_size.max(1)];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(HashError::Cancelled);
        }
        let read = file.read(&mut buffer).map_err(HashError::Io)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

fn file_len(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .ok()
        .filter(|meta| meta.is_file())
        .map(|meta| meta.len())
}

/// Sleeps, but wakes early to notice cancellation. Returns false if cancelled.
fn sleep_cancellable(total: Duration, cancel: &AtomicBool) -> bool {
    const TICK: Duration = Duration::from_millis(100);
    let mut slept = Duration::ZERO;
    while slept < total {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        let step = TICK.min(total - slept);
        std::thread::sleep(step);
        slept += step;
    }
    !cancel.load(Ordering::Relaxed)
}

/// Writes the attribution file that ships beside the models.
///
/// Parakeet is CC-BY-4.0, which requires the licence and the creator to travel
/// with the files; the rest are MIT, which requires the same. Rewritten in full
/// on every successful run so it always describes what is actually installed.
pub fn write_notice(models_dir: &Path, sets: &[&ModelSet]) -> std::io::Result<()> {
    let mut text = String::from(
        "Kodabi downloads the machine-learning models below on first run. They are \
         third-party works, redistributed under their own licences, and are not \
         covered by Kodabi's AGPL-3.0 licence.\n",
    );
    for set in sets {
        text.push_str("\n----------------------------------------------------------------\n");
        text.push_str(&format!("{} ({})\n\n", set.id, set.license.name));
        text.push_str(set.license.notice.trim_end());
        text.push('\n');
    }
    std::fs::create_dir_all(models_dir)?;
    std::fs::write(models_dir.join(NOTICE_FILE), text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::manifest;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// How the fake transport behaves for one call.
    #[derive(Debug, Clone)]
    enum Behaviour {
        /// Honour the range and send the rest.
        Honour,
        /// Answer 200 with the whole body, as a range-blind server does.
        IgnoreRange,
        /// Honour the range but stop early, as a dropped connection does.
        TruncateAfter(usize),
        /// Fail before sending anything.
        Fail(&'static str),
        /// Send `n` bytes and then raise a read error mid-body.
        ErrorAfter(usize),
    }

    struct FakeFetcher {
        content: Vec<u8>,
        script: Mutex<VecDeque<Behaviour>>,
        /// Range offsets requested, in order, so tests can assert resumption.
        requests: Mutex<Vec<u64>>,
    }

    impl FakeFetcher {
        fn new(content: Vec<u8>, script: Vec<Behaviour>) -> Self {
            Self {
                content,
                script: Mutex::new(script.into()),
                requests: Mutex::new(Vec::new()),
            }
        }

        fn always(content: Vec<u8>) -> Self {
            Self::new(content, Vec::new())
        }

        fn requests(&self) -> Vec<u64> {
            self.requests.lock().expect("not poisoned").clone()
        }
    }

    /// A reader that yields bytes then raises an error, standing in for a
    /// connection that dies mid-body.
    struct FailingReader {
        remaining: std::io::Cursor<Vec<u8>>,
    }

    impl Read for FailingReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let read = self.remaining.read(buf)?;
            if read == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "connection reset by peer",
                ));
            }
            Ok(read)
        }
    }

    impl Fetcher for FakeFetcher {
        fn fetch(&self, _url: &str, range_start: u64) -> Result<FetchResponse, ModelsError> {
            self.requests
                .lock()
                .expect("not poisoned")
                .push(range_start);
            let behaviour = self
                .script
                .lock()
                .expect("not poisoned")
                .pop_front()
                .unwrap_or(Behaviour::Honour);
            let start = range_start as usize;
            match behaviour {
                Behaviour::Honour => Ok(FetchResponse {
                    body: Box::new(std::io::Cursor::new(self.content[start..].to_vec())),
                    resumed: start > 0,
                }),
                Behaviour::IgnoreRange => Ok(FetchResponse {
                    body: Box::new(std::io::Cursor::new(self.content.clone())),
                    resumed: false,
                }),
                Behaviour::TruncateAfter(count) => {
                    let end = (start + count).min(self.content.len());
                    Ok(FetchResponse {
                        body: Box::new(std::io::Cursor::new(self.content[start..end].to_vec())),
                        resumed: start > 0,
                    })
                }
                Behaviour::ErrorAfter(count) => {
                    let end = (start + count).min(self.content.len());
                    Ok(FetchResponse {
                        body: Box::new(FailingReader {
                            remaining: std::io::Cursor::new(self.content[start..end].to_vec()),
                        }),
                        resumed: start > 0,
                    })
                }
                Behaviour::Fail(message) => Err(ModelsError::Http {
                    file: "test".to_string(),
                    message: message.to_string(),
                }),
            }
        }
    }

    /// Payload used throughout: 300 bytes with a recognisable pattern.
    fn payload() -> Vec<u8> {
        (0..300u32).map(|index| (index % 251) as u8).collect()
    }

    fn sha_of(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    /// Options with no real sleeping, so retry tests stay instant.
    fn fast_options() -> DownloadOptions {
        DownloadOptions {
            max_attempts: 4,
            backoff: vec![Duration::ZERO],
            progress_min_interval: Duration::ZERO,
            chunk_size: 64,
        }
    }

    fn one_file_plan(dir: &Path, content: &[u8]) -> DownloadPlan {
        let target = dir.join("stt").join("model.onnx");
        DownloadPlan {
            bytes_total: content.len() as u64,
            files: vec![PlannedFile {
                display: "stt/model.onnx".to_string(),
                url: "https://example/model.onnx".to_string(),
                target,
                size: content.len() as u64,
                sha256: sha_of(content),
            }],
        }
    }

    fn run(
        plan: &DownloadPlan,
        fetcher: &dyn Fetcher,
        options: &DownloadOptions,
    ) -> (Result<(), ModelsError>, Vec<String>) {
        let cancel = AtomicBool::new(false);
        let mut events = Vec::new();
        let result = download(plan, fetcher, options, &cancel, &mut |progress| {
            events.push(format!("{progress:?}"));
        });
        (result, events)
    }

    #[test]
    fn a_clean_run_writes_the_file_and_removes_the_partial() {
        let dir = tempfile::tempdir().expect("tempdir");
        let content = payload();
        let plan = one_file_plan(dir.path(), &content);
        let fetcher = FakeFetcher::always(content.clone());

        let (result, _) = run(&plan, &fetcher, &fast_options());
        result.expect("should succeed");

        let target = &plan.files[0].target;
        assert_eq!(std::fs::read(target).expect("read"), content);
        assert!(!part_path(target).exists(), "the .part must be gone");
        assert_eq!(fetcher.requests(), vec![0]);
    }

    #[test]
    fn an_existing_partial_is_resumed_rather_than_restarted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let content = payload();
        let plan = one_file_plan(dir.path(), &content);
        let part = part_path(&plan.files[0].target);
        std::fs::create_dir_all(part.parent().expect("parent")).expect("mkdir");
        std::fs::write(&part, &content[..120]).expect("seed the partial");

        let fetcher = FakeFetcher::always(content.clone());
        let (result, _) = run(&plan, &fetcher, &fast_options());
        result.expect("should succeed");

        // The transport was asked to continue from exactly what was on disk.
        assert_eq!(fetcher.requests(), vec![120]);
        assert_eq!(
            std::fs::read(&plan.files[0].target).expect("read"),
            content,
            "resumed bytes must reassemble the original"
        );
    }

    #[test]
    fn a_server_that_ignores_the_range_restarts_the_file_instead_of_appending() {
        let dir = tempfile::tempdir().expect("tempdir");
        let content = payload();
        let plan = one_file_plan(dir.path(), &content);
        let part = part_path(&plan.files[0].target);
        std::fs::create_dir_all(part.parent().expect("parent")).expect("mkdir");
        std::fs::write(&part, &content[..120]).expect("seed the partial");

        let fetcher = FakeFetcher::new(content.clone(), vec![Behaviour::IgnoreRange]);
        let (result, _) = run(&plan, &fetcher, &fast_options());
        result.expect("should succeed");

        // Appending would have produced 420 bytes of nonsense that still hashed
        // to nothing; restarting is the only correct response.
        assert_eq!(std::fs::read(&plan.files[0].target).expect("read"), content);
    }

    #[test]
    fn a_file_already_complete_on_disk_is_not_refetched() {
        let dir = tempfile::tempdir().expect("tempdir");
        let content = payload();
        let manifest = manifest::parse(&format!(
            r#"{{"schema_version":1,"release_tag":"t","base_url":"https://e",
                "model_sets":[{{"id":"stt","requires_any":["parakeet"],"dir":"stt",
                  "license":{{"name":"MIT","notice":"n"}},
                  "files":[{{"name":"model.onnx","asset":"a.onnx","size":{},"sha256":"{}"}}]}}]}}"#,
            content.len(),
            sha_of(&content)
        ))
        .expect("parses");

        let target = dir.path().join("stt").join("model.onnx");
        std::fs::create_dir_all(target.parent().expect("parent")).expect("mkdir");
        std::fs::write(&target, &content).expect("write");

        let plan = plan(&manifest, dir.path(), &["parakeet"], &|_| false);
        assert!(plan.is_empty(), "nothing left to fetch");
    }

    #[test]
    fn a_truncated_transfer_retries_and_resumes_from_what_landed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let content = payload();
        let plan = one_file_plan(dir.path(), &content);
        let fetcher = FakeFetcher::new(content.clone(), vec![Behaviour::TruncateAfter(100)]);

        let (result, events) = run(&plan, &fetcher, &fast_options());
        result.expect("should succeed on the retry");

        assert_eq!(
            fetcher.requests(),
            vec![0, 100],
            "the retry must resume at the truncation point"
        );
        assert!(events.iter().any(|event| event.contains("Retrying")));
        assert_eq!(std::fs::read(&plan.files[0].target).expect("read"), content);
    }

    #[test]
    fn a_connection_dropped_mid_body_retries_and_resumes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let content = payload();
        let plan = one_file_plan(dir.path(), &content);
        let fetcher = FakeFetcher::new(content.clone(), vec![Behaviour::ErrorAfter(64)]);

        let (result, _) = run(&plan, &fetcher, &fast_options());
        result.expect("should succeed on the retry");
        assert_eq!(fetcher.requests(), vec![0, 64]);
        assert_eq!(std::fs::read(&plan.files[0].target).expect("read"), content);
    }

    #[test]
    fn a_hash_mismatch_deletes_the_partial_and_names_both_hashes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let content = payload();
        let mut plan = one_file_plan(dir.path(), &content);
        let expected = sha_of(b"something else entirely");
        plan.files[0].sha256 = expected.clone();

        let fetcher = FakeFetcher::always(content.clone());
        let mut options = fast_options();
        options.max_attempts = 2;
        let (result, _) = run(&plan, &fetcher, &options);

        let err = result.expect_err("a wrong file must not be accepted");
        match err {
            ModelsError::ShaMismatch {
                ref file,
                expected: ref want,
                ..
            } => {
                assert_eq!(file, "stt/model.onnx");
                assert_eq!(want, &expected);
            }
            other => panic!("expected a sha mismatch, got {other:?}"),
        }
        assert!(!plan.files[0].target.exists(), "nothing may be finalized");
        assert!(
            !part_path(&plan.files[0].target).exists(),
            "a wrong partial must not be left to resume"
        );
    }

    #[test]
    fn a_persistent_transport_failure_gives_up_after_max_attempts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let content = payload();
        let plan = one_file_plan(dir.path(), &content);
        let fetcher = FakeFetcher::new(
            content,
            vec![
                Behaviour::Fail("no route to host"),
                Behaviour::Fail("no route to host"),
                Behaviour::Fail("no route to host"),
            ],
        );
        let mut options = fast_options();
        options.max_attempts = 3;

        let (result, _) = run(&plan, &fetcher, &options);
        let err = result.expect_err("should give up");
        assert!(err.to_string().contains("no route to host"), "{err}");
        assert_eq!(fetcher.requests().len(), 3, "exactly max_attempts tries");
    }

    #[test]
    fn cancellation_stops_promptly_and_keeps_the_partial_for_resume() {
        let dir = tempfile::tempdir().expect("tempdir");
        let content = payload();
        let plan = one_file_plan(dir.path(), &content);
        let fetcher = FakeFetcher::always(content);
        let cancel = AtomicBool::new(false);
        let mut options = fast_options();
        options.chunk_size = 32;

        let mut chunks = 0;
        let result = download(&plan, &fetcher, &options, &cancel, &mut |progress| {
            if matches!(progress, Progress::Bytes { .. }) {
                chunks += 1;
                if chunks == 2 {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
        });

        assert!(matches!(result, Err(ModelsError::Cancelled)), "{result:?}");
        assert!(!plan.files[0].target.exists(), "nothing finalized");
        let part = part_path(&plan.files[0].target);
        assert!(part.exists(), "the partial must survive for the next run");
        assert!(file_len(&part).unwrap_or(0) > 0);
    }

    #[test]
    fn finalizing_replaces_a_stale_wrong_sized_file() {
        // Windows rename refuses to overwrite, so a leftover from an earlier
        // manifest would otherwise strand the download forever.
        let dir = tempfile::tempdir().expect("tempdir");
        let content = payload();
        let plan = one_file_plan(dir.path(), &content);
        let target = &plan.files[0].target;
        std::fs::create_dir_all(target.parent().expect("parent")).expect("mkdir");
        std::fs::write(target, b"an older, shorter model").expect("write");

        let fetcher = FakeFetcher::always(content.clone());
        let (result, _) = run(&plan, &fetcher, &fast_options());
        result.expect("should replace the stale file");
        assert_eq!(std::fs::read(target).expect("read"), content);
    }

    #[test]
    fn an_overlong_partial_is_discarded_rather_than_resumed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let content = payload();
        let plan = one_file_plan(dir.path(), &content);
        let part = part_path(&plan.files[0].target);
        std::fs::create_dir_all(part.parent().expect("parent")).expect("mkdir");
        std::fs::write(&part, vec![7u8; content.len() + 50]).expect("seed");

        let fetcher = FakeFetcher::always(content.clone());
        let (result, _) = run(&plan, &fetcher, &fast_options());
        result.expect("should restart cleanly");
        assert_eq!(fetcher.requests(), vec![0]);
        assert_eq!(std::fs::read(&plan.files[0].target).expect("read"), content);
    }

    #[test]
    fn progress_counts_every_file_toward_one_overall_total() {
        let dir = tempfile::tempdir().expect("tempdir");
        let content = payload();
        let sha = sha_of(&content);
        let files: Vec<PlannedFile> = ["a.onnx", "b.onnx"]
            .iter()
            .map(|name| PlannedFile {
                display: format!("stt/{name}"),
                url: format!("https://example/{name}"),
                target: dir.path().join("stt").join(name),
                size: content.len() as u64,
                sha256: sha.clone(),
            })
            .collect();
        let plan = DownloadPlan {
            bytes_total: content.len() as u64 * 2,
            files,
        };

        let fetcher = FakeFetcher::always(content.clone());
        let cancel = AtomicBool::new(false);
        let mut overall = Vec::new();
        download(&plan, &fetcher, &fast_options(), &cancel, &mut |progress| {
            if let Progress::Bytes {
                overall_received,
                overall_total,
                ..
            } = progress
            {
                assert_eq!(overall_total, 600);
                overall.push(overall_received);
            }
        })
        .expect("should succeed");

        assert_eq!(overall.last().copied(), Some(600), "ends at the true total");
        assert!(
            overall.windows(2).all(|pair| pair[1] >= pair[0]),
            "overall progress must never go backwards: {overall:?}"
        );
    }

    #[test]
    fn the_notice_file_carries_every_installed_licence() {
        let dir = tempfile::tempdir().expect("tempdir");
        let manifest = manifest::embedded().expect("parses");
        let sets = manifest.required_sets(&["parakeet", "embed"]);
        write_notice(dir.path(), &sets).expect("write");

        let notice = std::fs::read_to_string(dir.path().join(NOTICE_FILE)).expect("read");
        assert!(
            notice.contains("CC BY 4.0"),
            "the Parakeet licence is named"
        );
        assert!(notice.contains("NVIDIA"), "the creator is named");
        assert!(notice.contains("bge-small-en-v1.5"));
        assert!(notice.contains("Silero"));
    }
}
