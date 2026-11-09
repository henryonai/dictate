//! The Whisper models the app can use: a catalog, a folder that holds them, and a resumable download from
//! Hugging Face. The same download fetches the files that have a fixed address and size (`RemoteFile`), such as the
//! speaker separation models.
//!
//! A model file only ever appears under its final name once every byte has arrived: the download writes to
//! `<file>.part`, checks the size and renames. An interrupted download leaves the `.part` behind (for resuming)
//! and never a model.

use std::fmt;
use std::fs;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::io::{AsyncWriteExt, BufWriter};
use tokio_util::sync::CancellationToken;

use crate::http::{self, ApiError};

/// Where the app downloads models from; `download` appends the file name.
pub const HUGGING_FACE_BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";

/// A file at a fixed address with a known size, downloaded by `download_file`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteFile {
    pub url: &'static str,
    /// The name it is saved under.
    pub file: &'static str,
    /// Its size in bytes: the file is not kept unless the download comes to exactly this.
    pub size: u64,
}

/// One downloadable model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: &'static str,
    /// The file name in the folder and on Hugging Face.
    pub file: &'static str,
    /// Download size in MiB.
    pub size_mb: u32,
    /// The translation key of the one-line description shown next to the model.
    pub note_key: &'static str,
}

/// The models on offer: the plain ones by size, with the quantized large model (almost as good as the full one,
/// a third of the size) in between.
pub const CATALOG: &[ModelInfo] = &[
    ModelInfo {
        id: "tiny",
        file: "ggml-tiny.bin",
        size_mb: 75,
        note_key: "models.note.tiny",
    },
    ModelInfo {
        id: "base",
        file: "ggml-base.bin",
        size_mb: 142,
        note_key: "models.note.base",
    },
    ModelInfo {
        id: "small",
        file: "ggml-small.bin",
        size_mb: 466,
        note_key: "models.note.small",
    },
    ModelInfo {
        id: "large-v3-turbo-q5_0",
        file: "ggml-large-v3-turbo-q5_0.bin",
        size_mb: 547,
        note_key: "models.note.large-v3-turbo-q5_0",
    },
    ModelInfo {
        id: "medium",
        file: "ggml-medium.bin",
        size_mb: 1533,
        note_key: "models.note.medium",
    },
    ModelInfo {
        id: "large-v3-turbo",
        file: "ggml-large-v3-turbo.bin",
        size_mb: 1624,
        note_key: "models.note.large-v3-turbo",
    },
];

/// The catalog entry of a model id.
pub fn find(id: &str) -> Option<&'static ModelInfo> {
    CATALOG.iter().find(|m| m.id == id)
}

/// The model to suggest on a machine with this much RAM.
pub fn recommended(total_ram_gb: u64) -> &'static str {
    match total_ram_gb {
        16.. => "large-v3-turbo-q5_0",
        8..=15 => "small",
        _ => "base",
    }
}

/// The URL of a model file below `base_url`.
fn file_url(base_url: &str, file: &str) -> String {
    format!("{}/{file}", base_url.trim_end_matches('/'))
}

/// The file of a part-way download of a model file.
fn part_name(file: &str) -> String {
    format!("{file}.part")
}

/// One row of the models list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelStatus {
    pub id: &'static str,
    pub size_mb: u32,
    pub downloaded: bool,
    pub recommended: bool,
}

/// The folder the models live in.
#[derive(Debug, Clone)]
pub struct ModelDir {
    root: PathBuf,
}

impl ModelDir {
    pub fn new(root: PathBuf) -> Self {
        ModelDir { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where the file of a model is (or will be). An id outside the catalog gets a name that is no model's, so
    /// an id taken from outside (a setting, the UI) can never point out of the folder.
    pub fn path(&self, id: &str) -> PathBuf {
        self.root
            .join(find(id).map_or("unknown-model", |model| model.file))
    }

    fn part_path(&self, model: &ModelInfo) -> PathBuf {
        self.root.join(part_name(model.file))
    }

    /// Whether the model's final file is there and holds something. A `.part` file does not count.
    pub fn is_downloaded(&self, id: &str) -> bool {
        find(id).is_some_and(|model| {
            fs::metadata(self.root.join(model.file)).is_ok_and(|m| m.is_file() && m.len() > 0)
        })
    }

    /// Every model of the catalog, in its order, with what is on disk and the recommendation for this machine.
    pub fn list(&self) -> Vec<ModelStatus> {
        self.list_with_ram(crate::system::total_ram_gb())
    }

    fn list_with_ram(&self, total_ram_gb: u64) -> Vec<ModelStatus> {
        let recommended = recommended(total_ram_gb);
        CATALOG
            .iter()
            .map(|model| ModelStatus {
                id: model.id,
                size_mb: model.size_mb,
                downloaded: self.is_downloaded(model.id),
                recommended: model.id == recommended,
            })
            .collect()
    }

    /// Removes a model and any half-finished download of it; a model that is not there is not an error.
    pub fn delete(&self, id: &str) -> io::Result<()> {
        let model = find(id).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "There is no such model.")
        })?;
        for path in [self.root.join(model.file), self.part_path(model)] {
            match fs::remove_file(path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
        Ok(())
    }

    /// MiB on disk: the models, and the unfinished downloads of them.
    pub fn disk_usage_mb(&self) -> u64 {
        let bytes: u64 = CATALOG
            .iter()
            .flat_map(|model| [self.root.join(model.file), self.part_path(model)])
            .filter_map(|path| fs::metadata(path).ok())
            .filter(|meta| meta.is_file())
            .map(|meta| meta.len())
            .sum();
        bytes / (1024 * 1024)
    }
}

/// How long a download may go without receiving anything before it gives up. There is no limit on the whole
/// download: a model is up to 1.6 GB.
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Why a download did not give a model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadError {
    /// The caller cancelled it. The part that arrived stays for resuming.
    Cancelled,
    /// The server could not be reached, answered with an error, stalled or broke the connection.
    Http(ApiError),
    /// The disk refused: no space, no permission.
    Io(String),
    /// The file that arrived is not the size the server announced, or the server announced none (or an empty
    /// file). What arrived is thrown away: it cannot be trusted as a model.
    SizeMismatch,
}

impl fmt::Display for DownloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DownloadError::Cancelled => f.write_str("The download was cancelled."),
            DownloadError::Http(e) => write!(f, "{e}"),
            DownloadError::Io(m) => write!(f, "Could not save the model: {m}"),
            DownloadError::SizeMismatch => {
                f.write_str("The downloaded file is not the size the server announced.")
            }
        }
    }
}

impl std::error::Error for DownloadError {}

impl From<ApiError> for DownloadError {
    fn from(e: ApiError) -> Self {
        DownloadError::Http(e)
    }
}

/// Downloads a model into `dir` and returns where it is.
///
/// The bytes go to `<file>.part`; an unfinished one is resumed with a `Range` request. The file takes its final
/// name only when its size is the one the server announced. `on_progress(done, total)` hears about it at most
/// every 250 ms or per 1% (and always at the start and the end). Cancelling stops the transfer at once and keeps
/// the `.part`.
///
/// `base_url` is where `<file>` is fetched from: `HUGGING_FACE_BASE_URL` in the app, a test server in tests.
/// Hugging Face answers with a redirect to its CDN, which is followed (up to 5 hops, only to https or the same
/// origin). No credentials are ever sent.
pub async fn download(
    dir: &ModelDir,
    id: &str,
    base_url: &str,
    on_progress: impl Fn(u64, Option<u64>) + Send,
    cancel: CancellationToken,
) -> Result<PathBuf, DownloadError> {
    download_with(dir, id, base_url, on_progress, cancel, READ_TIMEOUT).await
}

/// `download` with the stall limit as a parameter, for tests.
async fn download_with(
    dir: &ModelDir,
    id: &str,
    base_url: &str,
    on_progress: impl Fn(u64, Option<u64>) + Send,
    cancel: CancellationToken,
    read_timeout: Duration,
) -> Result<PathBuf, DownloadError> {
    let model = find(id).ok_or_else(|| {
        DownloadError::Http(ApiError::Invalid(format!("There is no model called {id}.")))
    })?;
    let target = Target {
        url: &file_url(base_url, model.file),
        dir: dir.root(),
        file: model.file,
        size: None,
    };
    fetch(target, on_progress, cancel, read_timeout, redirect_allowed).await
}

/// Downloads `f` into `dir` and returns where it is: `download` for a file that has its own address and size.
///
/// The same `.part` file, `Range` resume, redirects and progress as `download`. In addition the size the server
/// announces has to be `f.size`: a file of another size is not kept (and a `.part` of it is dropped).
pub async fn download_file(
    dir: &Path,
    f: &RemoteFile,
    on_progress: impl Fn(u64, Option<u64>) + Send,
    cancel: CancellationToken,
) -> Result<PathBuf, DownloadError> {
    download_file_with(dir, f, on_progress, cancel, READ_TIMEOUT, redirect_allowed).await
}

/// `download_file` with the stall limit and the redirect rule as parameters, for tests (where the CDN is another
/// server on plain http).
async fn download_file_with(
    dir: &Path,
    f: &RemoteFile,
    on_progress: impl Fn(u64, Option<u64>) + Send,
    cancel: CancellationToken,
    read_timeout: Duration,
    redirect_ok: RedirectRule,
) -> Result<PathBuf, DownloadError> {
    let target = Target {
        url: f.url,
        dir,
        file: f.file,
        size: Some(f.size),
    };
    fetch(target, on_progress, cancel, read_timeout, redirect_ok).await
}

/// What `fetch` brings in, and where it goes.
struct Target<'a> {
    url: &'a str,
    /// The folder, and the name of the file in it.
    dir: &'a Path,
    file: &'a str,
    /// What the file has to come to, when that is known beforehand. `None` takes the size the server announces.
    size: Option<u64>,
}

/// The shared download: see `download` and `download_file`.
async fn fetch(
    target: Target<'_>,
    on_progress: impl Fn(u64, Option<u64>) + Send,
    cancel: CancellationToken,
    read_timeout: Duration,
    redirect_ok: RedirectRule,
) -> Result<PathBuf, DownloadError> {
    let url = http::parse_url(target.url)?;
    let client = http::client()?;
    tokio::fs::create_dir_all(target.dir)
        .await
        .map_err(io_error)?;
    let part_path = target.dir.join(part_name(target.file));
    let final_path = target.dir.join(target.file);

    // An unfinished download is continued from where it stopped, if the server goes along.
    let mut have = match tokio::fs::metadata(&part_path).await {
        Ok(meta) if meta.is_file() => meta.len(),
        _ => 0,
    };
    // `reply` is None when the part file already holds the whole model.
    let (reply, mut done, total) = loop {
        let response = get(
            &client,
            &url,
            (have > 0).then_some(have),
            &cancel,
            read_timeout,
            redirect_ok,
        )
        .await?;
        match response.status().as_u16() {
            206 => {
                let range = header_str(&response, reqwest::header::CONTENT_RANGE)
                    .and_then(parse_content_range);
                match range {
                    Some(ContentRange {
                        start: Some(start),
                        total: Some(total),
                    }) if start == have => break (Some(response), have, total),
                    _ => {
                        return Err(DownloadError::Http(ApiError::Invalid(
                            "The server did not continue the file where it was asked to.".into(),
                        )))
                    }
                }
            }
            200 => {
                // It sends everything again, so what is in the part file is of no use.
                let Some(total) = response.content_length() else {
                    return Err(DownloadError::SizeMismatch);
                };
                break (Some(response), 0, total);
            }
            416 if have > 0 => {
                let total = header_str(&response, reqwest::header::CONTENT_RANGE)
                    .and_then(parse_content_range)
                    .and_then(|range| range.total);
                if total == Some(have) {
                    break (None, have, have);
                }
                // The part file is longer than the model: it is not this model's. Start again.
                tokio::fs::remove_file(&part_path).await.map_err(io_error)?;
                have = 0;
            }
            _ => return Err(status_failure(response, &cancel, read_timeout).await),
        }
    };

    // Not the file that was asked for, or a part of another one: nothing of it is kept.
    if target.size.is_some_and(|want| want != total) {
        let _ = tokio::fs::remove_file(&part_path).await;
        return Err(DownloadError::SizeMismatch);
    }
    // No model is empty: a server that says so has nothing to give.
    if total == 0 {
        return Err(DownloadError::SizeMismatch);
    }
    on_progress(done, Some(total));
    let mut gate = ProgressGate::new(Instant::now(), done);
    if let Some(mut response) = reply {
        let file = if done == 0 {
            tokio::fs::File::create(&part_path).await
        } else {
            tokio::fs::OpenOptions::new()
                .append(true)
                .open(&part_path)
                .await
        }
        .map_err(io_error)?;
        let mut writer = BufWriter::with_capacity(WRITE_BUFFER, file);
        let outcome: Result<(), DownloadError> = async {
            while let Some(chunk) = within(&cancel, read_timeout, response.chunk()).await? {
                writer.write_all(&chunk).await.map_err(io_error)?;
                done += chunk.len() as u64;
                if gate.due(Instant::now(), done, total) {
                    on_progress(done, Some(total));
                }
            }
            Ok(())
        }
        .await;
        // What arrived is flushed even when the transfer broke off: it is what a resume starts from.
        let flushed = writer.flush().await.map_err(io_error);
        outcome?;
        flushed?;
        writer.get_ref().sync_all().await.map_err(io_error)?;
    }

    let on_disk = tokio::fs::metadata(&part_path)
        .await
        .map_err(io_error)?
        .len();
    if on_disk != total {
        let _ = tokio::fs::remove_file(&part_path).await;
        return Err(DownloadError::SizeMismatch);
    }
    tokio::fs::rename(&part_path, &final_path)
        .await
        .map_err(io_error)?;
    if gate.last_done != done {
        on_progress(done, Some(total));
    }
    Ok(final_path)
}

/// GET `url`, asking for the bytes from `range_from` on. Redirects are followed by hand (the shared client does
/// not follow them): up to `MAX_REDIRECTS`, each to an address `redirect_ok` accepts, with the same `Range`.
/// Only the user agent is ever sent, so nothing secret can reach the CDN a redirect names.
async fn get(
    client: &reqwest::Client,
    url: &reqwest::Url,
    range_from: Option<u64>,
    cancel: &CancellationToken,
    read_timeout: Duration,
    redirect_ok: RedirectRule,
) -> Result<reqwest::Response, DownloadError> {
    let invalid = |message: &str| DownloadError::Http(ApiError::Invalid(message.to_string()));
    let mut url = url.clone();
    for _ in 0..=MAX_REDIRECTS {
        let mut request = client.get(url.clone());
        if let Some(from) = range_from {
            request = request.header(reqwest::header::RANGE, format!("bytes={from}-"));
        }
        let response = within(cancel, read_timeout, request.send()).await?;
        if !REDIRECTS.contains(&response.status().as_u16()) {
            return Ok(response);
        }
        // The address is not put in the messages: a CDN's carries a signature.
        let next = header_str(&response, reqwest::header::LOCATION)
            .and_then(|location| url.join(location).ok())
            .ok_or_else(|| invalid("The server sent a redirect without a usable address."))?;
        if !redirect_ok(&url, &next) {
            return Err(invalid(
                "The server redirected to an address that is not https.",
            ));
        }
        url = next;
    }
    Err(invalid("The server redirected too many times."))
}

fn header_str(response: &reqwest::Response, name: reqwest::header::HeaderName) -> Option<&str> {
    response.headers().get(name)?.to_str().ok()
}

fn io_error(e: io::Error) -> DownloadError {
    DownloadError::Io(e.to_string())
}

/// The error for a reply that is not a success: the status and the start of what the server said.
async fn status_failure(
    mut response: reqwest::Response,
    cancel: &CancellationToken,
    read_timeout: Duration,
) -> DownloadError {
    let status = response.status().as_u16();
    let mut body = Vec::new();
    while body.len() < 2048 {
        match within(cancel, read_timeout, response.chunk()).await {
            Ok(Some(chunk)) => body.extend_from_slice(&chunk),
            _ => break,
        }
    }
    DownloadError::Http(http::status_error(status, &body, None))
}

/// Waits for one step of the transfer (the reply, or the next piece of the body): ends at once when `cancel` is
/// cancelled, and gives up when nothing comes for `limit`. The limit is per step, so a long download that keeps
/// moving never hits it.
async fn within<T>(
    cancel: &CancellationToken,
    limit: Duration,
    step: impl Future<Output = Result<T, reqwest::Error>>,
) -> Result<T, DownloadError> {
    match cancel
        .run_until_cancelled(tokio::time::timeout(limit, step))
        .await
    {
        None => Err(DownloadError::Cancelled),
        Some(Err(_)) => Err(DownloadError::Http(ApiError::Network(format!(
            "stalled: nothing received for {limit:?}"
        )))),
        Some(Ok(result)) => result.map_err(|e| DownloadError::Http(e.into())),
    }
}

/// The statuses that point to another address.
const REDIRECTS: [u16; 5] = [301, 302, 303, 307, 308];

/// How many redirects are followed before the download gives up.
const MAX_REDIRECTS: usize = 5;

/// Tells whether a redirect from the first address to the second may be followed; `redirect_allowed` in the app.
type RedirectRule = fn(&reqwest::Url, &reqwest::Url) -> bool;

/// Whether a redirect from `from` to `to` may be followed: to any https address (Hugging Face sends the file from
/// a host below `hf.co`, GitHub release assets come from `objects.githubusercontent.com` and
/// `release-assets.githubusercontent.com`), and to the same origin even over plain http (a test server, a local
/// mirror).
fn redirect_allowed(from: &reqwest::Url, to: &reqwest::Url) -> bool {
    to.scheme() == "https" || to.origin() == from.origin()
}

/// What a `Content-Range` header says: where the body starts in the file and how long the file is, when known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ContentRange {
    start: Option<u64>,
    total: Option<u64>,
}

/// Reads `bytes 5-9/10`, `bytes 5-9/*` or `bytes */10`.
fn parse_content_range(value: &str) -> Option<ContentRange> {
    let (range, total) = value.strip_prefix("bytes ")?.split_once('/')?;
    let total = match total {
        "*" => None,
        n => Some(n.parse().ok()?),
    };
    let start = match range {
        "*" => None,
        r => Some(r.split_once('-')?.0.parse().ok()?),
    };
    Some(ContentRange { start, total })
}

/// The disk is written through a buffer of this size: a 64 KiB chunk per call would be a lot of small writes.
const WRITE_BUFFER: usize = 256 * 1024;

/// How often progress is reported: every 250 ms or 1% of the file, whichever comes first.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

struct ProgressGate {
    last_at: Instant,
    last_done: u64,
}

impl ProgressGate {
    fn new(now: Instant, done: u64) -> Self {
        ProgressGate {
            last_at: now,
            last_done: done,
        }
    }

    /// Whether `done` of `total` bytes is worth a report now; if so, it counts as reported.
    fn due(&mut self, now: Instant, done: u64, total: u64) -> bool {
        let one_percent = (total / 100).max(1);
        let due = now.duration_since(self.last_at) >= PROGRESS_EVERY
            || done.saturating_sub(self.last_done) >= one_percent;
        if due {
            self.last_at = now;
            self.last_done = done;
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    const KIB: usize = 1024;
    const MIB: usize = 1024 * 1024;
    type Calls = Arc<Mutex<Vec<(u64, Option<u64>)>>>;

    fn dir() -> (tempfile::TempDir, ModelDir) {
        let tmp = tempfile::tempdir().unwrap();
        let models = ModelDir::new(tmp.path().join("models"));
        fs::create_dir_all(models.root()).unwrap();
        (tmp, models)
    }

    /// Bytes that tell their own position, so a wrongly joined file cannot match by luck.
    fn body(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    fn part_file(models: &ModelDir) -> PathBuf {
        models.root().join("ggml-tiny.bin.part")
    }

    fn recorder() -> (impl Fn(u64, Option<u64>) + Send, Calls) {
        let calls: Calls = Arc::default();
        let sink = Arc::clone(&calls);
        (
            move |done, total| sink.lock().unwrap().push((done, total)),
            calls,
        )
    }

    /// Serves `body` as `/ggml-tiny.bin` the way Hugging Face does: all of it, or the tail a `Range: bytes=N-`
    /// asks for with a 206 (a 416 when N is not inside the file).
    struct RangeServer(Vec<u8>);

    impl Respond for RangeServer {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let len = self.0.len();
            let from = request
                .headers
                .get("range")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| {
                    v.strip_prefix("bytes=")?
                        .strip_suffix('-')?
                        .parse::<usize>()
                        .ok()
                });
            match from {
                None => ResponseTemplate::new(200).set_body_bytes(self.0.clone()),
                Some(from) if from < len => ResponseTemplate::new(206)
                    .insert_header("content-range", format!("bytes {from}-{}/{len}", len - 1))
                    .set_body_bytes(self.0[from..].to_vec()),
                Some(_) => ResponseTemplate::new(416)
                    .insert_header("content-range", format!("bytes */{len}")),
            }
        }
    }

    async fn serve(responder: impl Respond + 'static) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/ggml-tiny.bin"))
            .respond_with(responder)
            .mount(&server)
            .await;
        server
    }

    #[derive(Clone, Copy)]
    enum Ending {
        /// Close the connection after the reply.
        Close,
        /// Keep it open and say nothing more.
        Stall,
    }

    /// A server that answers every connection with `reply` verbatim, then ends it as told: for the replies
    /// wiremock will not send (a body shorter than its Content-Length, a stall).
    async fn raw_server(reply: Vec<u8>, ending: Ending) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let uri = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let reply = reply.clone();
                tokio::spawn(async move {
                    let mut seen = Vec::new();
                    let mut buf = [0u8; 1024];
                    while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => seen.extend_from_slice(&buf[..n]),
                        }
                    }
                    if !reply.is_empty() {
                        let _ = stream.write_all(&reply).await;
                        let _ = stream.flush().await;
                    }
                    if let Ending::Stall = ending {
                        std::future::pending::<()>().await;
                    }
                });
            }
        });
        uri
    }

    /// A reply that announces `announced` bytes and sends `sent` of them.
    fn short_reply(announced: usize, sent: usize) -> Vec<u8> {
        let mut reply =
            format!("HTTP/1.1 200 OK\r\nContent-Length: {announced}\r\n\r\n").into_bytes();
        reply.extend(body(sent));
        reply
    }

    async fn download_tiny(
        models: &ModelDir,
        uri: &str,
    ) -> (Result<PathBuf, DownloadError>, Vec<(u64, Option<u64>)>) {
        let (on_progress, calls) = recorder();
        let result = download(models, "tiny", uri, on_progress, CancellationToken::new()).await;
        let calls = calls.lock().unwrap().clone();
        (result, calls)
    }

    #[test]
    fn catalog_order_and_files() {
        assert_eq!(
            CATALOG.iter().map(|m| m.id).collect::<Vec<_>>(),
            [
                "tiny",
                "base",
                "small",
                "large-v3-turbo-q5_0",
                "medium",
                "large-v3-turbo"
            ]
        );
        assert_eq!(
            CATALOG
                .iter()
                .map(|m| (m.file, m.size_mb))
                .collect::<Vec<_>>(),
            [
                ("ggml-tiny.bin", 75),
                ("ggml-base.bin", 142),
                ("ggml-small.bin", 466),
                ("ggml-large-v3-turbo-q5_0.bin", 547),
                ("ggml-medium.bin", 1533),
                ("ggml-large-v3-turbo.bin", 1624),
            ]
        );
    }

    #[test]
    fn every_model_has_its_own_note_key() {
        let mut keys: Vec<_> = CATALOG.iter().map(|m| m.note_key).collect();
        assert_eq!(keys.len(), 6);
        assert!(keys.iter().all(|k| k.starts_with("models.note.")));
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), 6);
    }

    #[test]
    fn find_looks_a_model_up_by_id() {
        assert_eq!(find("small").map(|m| m.file), Some("ggml-small.bin"));
        assert_eq!(find("huge"), None);
        assert_eq!(find(""), None);
    }

    #[test]
    fn recommendation_by_ram() {
        assert_eq!(recommended(32), "large-v3-turbo-q5_0");
        assert_eq!(recommended(16), "large-v3-turbo-q5_0");
        assert_eq!(recommended(15), "small");
        assert_eq!(recommended(8), "small");
        assert_eq!(recommended(7), "base");
        assert_eq!(recommended(4), "base");
        assert_eq!(recommended(0), "base");
    }

    #[test]
    fn the_recommendation_is_always_in_the_catalog() {
        for gb in [0, 4, 8, 16, 64] {
            assert!(find(recommended(gb)).is_some());
        }
    }

    #[test]
    fn files_are_fetched_from_hugging_face() {
        assert_eq!(
            HUGGING_FACE_BASE_URL,
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main"
        );
        assert_eq!(
            file_url(HUGGING_FACE_BASE_URL, "ggml-tiny.bin"),
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin"
        );
    }

    #[test]
    fn file_url_ignores_a_trailing_slash() {
        assert_eq!(
            file_url("http://localhost:1234/models/", "ggml-tiny.bin"),
            "http://localhost:1234/models/ggml-tiny.bin"
        );
    }

    #[test]
    fn a_model_lives_under_its_catalog_file_name() {
        let (_tmp, models) = dir();
        assert_eq!(models.path("small"), models.root().join("ggml-small.bin"));
    }

    #[test]
    fn only_a_non_empty_final_file_is_a_downloaded_model() {
        let (_tmp, models) = dir();
        assert!(!models.is_downloaded("tiny"), "missing");

        fs::write(models.path("tiny"), b"").unwrap();
        assert!(!models.is_downloaded("tiny"), "empty");

        fs::write(models.path("tiny"), b"ggml").unwrap();
        assert!(models.is_downloaded("tiny"));
        assert!(!models.is_downloaded("base"), "another model");
    }

    #[test]
    fn a_part_file_is_not_a_model() {
        let (_tmp, models) = dir();
        fs::write(models.root().join("ggml-tiny.bin.part"), vec![1u8; 1024]).unwrap();
        assert!(!models.is_downloaded("tiny"));
    }

    #[test]
    fn a_folder_with_the_file_name_is_not_a_model() {
        let (_tmp, models) = dir();
        fs::create_dir(models.path("tiny")).unwrap();
        assert!(!models.is_downloaded("tiny"));
    }

    #[test]
    fn an_id_outside_the_catalog_never_leaves_the_model_folder() {
        let (tmp, models) = dir();
        let outside = tmp.path().join("precious.bin");
        fs::write(&outside, b"keep me").unwrap();

        for id in [
            "../precious.bin",
            "..",
            "",
            "/etc/passwd",
            "tiny/../../precious.bin",
        ] {
            assert!(models.path(id).starts_with(models.root()), "{id:?}");
            assert!(!models.is_downloaded(id), "{id:?}");
            let err = models.delete(id).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{id:?}");
        }
        assert_eq!(fs::read(&outside).unwrap(), b"keep me");
    }

    #[test]
    fn list_marks_downloaded_and_recommended() {
        let (_tmp, models) = dir();
        fs::write(models.path("base"), b"ggml").unwrap();
        fs::write(models.path("medium"), b"ggml").unwrap();
        // Not a model yet.
        fs::write(models.root().join("ggml-small.bin.part"), b"gg").unwrap();

        let list = models.list_with_ram(8);

        assert_eq!(
            list.iter().map(|m| m.id).collect::<Vec<_>>(),
            [
                "tiny",
                "base",
                "small",
                "large-v3-turbo-q5_0",
                "medium",
                "large-v3-turbo"
            ]
        );
        assert_eq!(
            list.iter().map(|m| m.downloaded).collect::<Vec<_>>(),
            [false, true, false, false, true, false]
        );
        assert_eq!(
            list.iter().map(|m| m.recommended).collect::<Vec<_>>(),
            [false, false, true, false, false, false]
        );
        assert_eq!(
            list.iter().map(|m| m.size_mb).collect::<Vec<_>>(),
            [75, 142, 466, 547, 1533, 1624]
        );
    }

    #[test]
    fn list_of_a_folder_that_does_not_exist_is_all_not_downloaded() {
        let models = ModelDir::new(PathBuf::from("/definitely/not/here"));
        let list = models.list_with_ram(32);
        assert_eq!(list.len(), CATALOG.len());
        assert!(list.iter().all(|m| !m.downloaded));
        assert_eq!(models.disk_usage_mb(), 0);
    }

    #[test]
    fn delete_removes_the_model_and_its_part_file() {
        let (_tmp, models) = dir();
        fs::write(models.path("tiny"), b"ggml").unwrap();
        fs::write(models.root().join("ggml-tiny.bin.part"), b"gg").unwrap();
        fs::write(models.path("base"), b"ggml").unwrap();

        models.delete("tiny").unwrap();

        assert!(!models.path("tiny").exists());
        assert!(!models.root().join("ggml-tiny.bin.part").exists());
        assert!(models.is_downloaded("base"), "other models stay");
    }

    #[test]
    fn deleting_a_model_that_is_not_there_is_fine() {
        let (_tmp, models) = dir();
        models.delete("tiny").unwrap();
        models.delete("tiny").unwrap();
    }

    #[test]
    fn disk_usage_counts_models_and_unfinished_downloads() {
        let (_tmp, models) = dir();
        const MIB: usize = 1024 * 1024;
        assert_eq!(models.disk_usage_mb(), 0);

        fs::write(models.path("tiny"), vec![0u8; 3 * MIB]).unwrap();
        fs::write(models.root().join("ggml-base.bin.part"), vec![0u8; 2 * MIB]).unwrap();
        // Not the app's files.
        fs::write(models.root().join("notes.txt"), vec![0u8; 5 * MIB]).unwrap();

        assert_eq!(models.disk_usage_mb(), 5);
    }

    #[tokio::test]
    async fn download_reports_progress_and_renames() {
        let (_tmp, models) = dir();
        let server = serve(RangeServer(body(MIB))).await;

        let (result, calls) = download_tiny(&models, &server.uri()).await;

        assert_eq!(result.unwrap(), models.path("tiny"));
        assert_eq!(fs::read(models.path("tiny")).unwrap(), body(MIB));
        assert!(!part_file(&models).exists(), "the .part is gone");
        assert!(models.is_downloaded("tiny"));
        assert_eq!(calls.first(), Some(&(0, Some(MIB as u64))));
        assert_eq!(calls.last(), Some(&(MIB as u64, Some(MIB as u64))));
        assert!(
            calls.windows(2).all(|w| w[0].0 <= w[1].0),
            "progress never goes back: {calls:?}"
        );
    }

    #[tokio::test]
    async fn a_download_sends_no_credentials() {
        let (_tmp, models) = dir();
        let server = serve(RangeServer(body(KIB))).await;

        download_tiny(&models, &server.uri()).await.0.unwrap();

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        for name in ["authorization", "cookie", "x-api-key"] {
            assert!(requests[0].headers.get(name).is_none(), "{name}");
        }
    }

    #[tokio::test]
    async fn interrupted_download_leaves_no_model() {
        let (_tmp, models) = dir();
        let uri = raw_server(short_reply(MIB, 100 * KIB), Ending::Close).await;

        let (result, _) = download_tiny(&models, &uri).await;

        assert!(
            matches!(result, Err(DownloadError::Http(ApiError::Network(_)))),
            "{result:?}"
        );
        assert!(!models.is_downloaded("tiny"));
        assert!(!models.path("tiny").exists());
        assert_eq!(
            fs::read(part_file(&models)).unwrap(),
            body(100 * KIB),
            "what arrived is kept for resuming"
        );
    }

    #[tokio::test]
    async fn an_error_status_is_reported_and_leaves_nothing() {
        let (_tmp, models) = dir();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404).set_body_string("Entry not found"))
            .mount(&server)
            .await;

        let (result, calls) = download_tiny(&models, &server.uri()).await;

        assert_eq!(
            result,
            Err(DownloadError::Http(ApiError::Status {
                status: 404,
                message: "Entry not found".into()
            }))
        );
        assert!(calls.is_empty());
        assert!(!models.path("tiny").exists());
        assert!(!part_file(&models).exists());
    }

    #[tokio::test]
    async fn a_reply_without_a_length_cannot_be_checked_so_it_is_not_a_model() {
        let (_tmp, models) = dir();
        let mut reply = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
        reply.extend(body(KIB));
        let uri = raw_server(reply, Ending::Close).await;

        let (result, _) = download_tiny(&models, &uri).await;

        assert_eq!(result, Err(DownloadError::SizeMismatch));
        assert!(!models.path("tiny").exists());
        assert!(!part_file(&models).exists());
    }

    #[tokio::test]
    async fn an_id_outside_the_catalog_is_refused_without_a_request() {
        let (_tmp, models) = dir();
        let server = serve(RangeServer(body(KIB))).await;

        let result = download(
            &models,
            "../../evil",
            &server.uri(),
            |_, _| {},
            CancellationToken::new(),
        )
        .await;

        assert!(
            matches!(result, Err(DownloadError::Http(ApiError::Invalid(_)))),
            "{result:?}"
        );
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_base_url_that_is_not_a_url_is_refused() {
        let (_tmp, models) = dir();
        let result = download(
            &models,
            "tiny",
            "not a url",
            |_, _| {},
            CancellationToken::new(),
        )
        .await;
        assert!(
            matches!(result, Err(DownloadError::Http(ApiError::Invalid(_)))),
            "{result:?}"
        );
    }

    #[test]
    fn the_download_can_run_on_another_thread() {
        fn assert_send<T: Send>(_: &T) {}
        let models = ModelDir::new(PathBuf::new());
        let future = download(
            &models,
            "tiny",
            "http://localhost",
            |_, _| {},
            CancellationToken::new(),
        );
        assert_send(&future);
    }

    #[test]
    fn errors_read_as_sentences() {
        assert_eq!(
            DownloadError::Cancelled.to_string(),
            "The download was cancelled."
        );
        assert_eq!(
            DownloadError::Http(ApiError::Network("refused".into())).to_string(),
            "Network error: refused"
        );
        assert_eq!(
            DownloadError::Io("disk full".into()).to_string(),
            "Could not save the model: disk full"
        );
        assert_eq!(
            DownloadError::SizeMismatch.to_string(),
            "The downloaded file is not the size the server announced."
        );
    }

    fn range_header(request: &Request) -> Option<String> {
        let value = request.headers.get("range")?;
        Some(value.to_str().ok()?.to_owned())
    }

    #[tokio::test]
    async fn resumes_from_part_file() {
        let (_tmp, models) = dir();
        fs::write(part_file(&models), &body(MIB)[..100 * KIB]).unwrap();
        let server = serve(RangeServer(body(MIB))).await;

        let (result, calls) = download_tiny(&models, &server.uri()).await;

        assert_eq!(result.unwrap(), models.path("tiny"));
        assert_eq!(fs::read(models.path("tiny")).unwrap(), body(MIB));
        assert!(!part_file(&models).exists());
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1, "one request, for the rest only");
        assert_eq!(range_header(&requests[0]).as_deref(), Some("bytes=102400-"));
        assert_eq!(calls.first(), Some(&(102_400, Some(MIB as u64))));
        assert_eq!(calls.last(), Some(&(MIB as u64, Some(MIB as u64))));
    }

    #[tokio::test]
    async fn server_ignoring_range_restarts_cleanly() {
        let (_tmp, models) = dir();
        fs::write(part_file(&models), vec![0xFF; 100 * KIB]).unwrap();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body(MIB)))
            .mount(&server)
            .await;

        let (result, calls) = download_tiny(&models, &server.uri()).await;

        assert_eq!(result.unwrap(), models.path("tiny"));
        assert_eq!(
            fs::read(models.path("tiny")).unwrap(),
            body(MIB),
            "the stale bytes are gone"
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(range_header(&requests[0]).as_deref(), Some("bytes=102400-"));
        assert_eq!(calls.first(), Some(&(0, Some(MIB as u64))));
    }

    #[tokio::test]
    async fn a_part_file_that_is_already_whole_is_finished_without_a_download() {
        // The app was closed between the last byte and the rename.
        let (_tmp, models) = dir();
        fs::write(part_file(&models), body(MIB)).unwrap();
        let server = serve(RangeServer(body(MIB))).await;

        let (result, calls) = download_tiny(&models, &server.uri()).await;

        assert_eq!(result.unwrap(), models.path("tiny"));
        assert_eq!(fs::read(models.path("tiny")).unwrap(), body(MIB));
        assert!(!part_file(&models).exists());
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1, "only the question whether there is more");
        assert_eq!(
            range_header(&requests[0]).as_deref(),
            Some("bytes=1048576-")
        );
        assert_eq!(calls, [(MIB as u64, Some(MIB as u64))]);
    }

    #[tokio::test]
    async fn a_part_file_longer_than_the_model_starts_over() {
        let (_tmp, models) = dir();
        fs::write(part_file(&models), vec![0xFF; MIB + KIB]).unwrap();
        let server = serve(RangeServer(body(MIB))).await;

        let (result, _) = download_tiny(&models, &server.uri()).await;

        assert_eq!(result.unwrap(), models.path("tiny"));
        assert_eq!(fs::read(models.path("tiny")).unwrap(), body(MIB));
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2);
        assert!(
            range_header(&requests[1]).is_none(),
            "the second try asks for everything"
        );
    }

    #[tokio::test]
    async fn a_206_that_does_not_continue_the_part_file_is_refused() {
        for content_range in [
            Some("bytes 0-1048575/1048576"),
            Some("bytes 102400-1048575/*"),
            Some("nonsense"),
            None,
        ] {
            let (_tmp, models) = dir();
            fs::write(part_file(&models), &body(MIB)[..100 * KIB]).unwrap();
            let server = MockServer::start().await;
            let mut reply = ResponseTemplate::new(206).set_body_bytes(body(MIB - 100 * KIB));
            if let Some(value) = content_range {
                reply = reply.insert_header("content-range", value);
            }
            Mock::given(method("GET"))
                .respond_with(reply)
                .mount(&server)
                .await;

            let (result, _) = download_tiny(&models, &server.uri()).await;

            assert!(
                matches!(result, Err(DownloadError::Http(ApiError::Invalid(_)))),
                "{content_range:?}: {result:?}"
            );
            assert!(!models.path("tiny").exists());
            assert_eq!(
                fs::read(part_file(&models)).unwrap(),
                &body(MIB)[..100 * KIB],
                "{content_range:?}: the part file is left alone"
            );
        }
    }

    #[tokio::test]
    async fn bytes_that_disagree_with_the_announced_total_are_not_a_model() {
        // The 206 says the file is `claimed` bytes long, but the bytes make it MIB.
        for claimed in [MIB + 5 * KIB, MIB - 5 * KIB] {
            let (_tmp, models) = dir();
            fs::write(part_file(&models), &body(MIB)[..100 * KIB]).unwrap();
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(
                    ResponseTemplate::new(206)
                        .insert_header(
                            "content-range",
                            format!("bytes 102400-{}/{claimed}", claimed - 1),
                        )
                        .set_body_bytes(body(MIB)[100 * KIB..].to_vec()),
                )
                .mount(&server)
                .await;

            let (result, _) = download_tiny(&models, &server.uri()).await;

            assert_eq!(result, Err(DownloadError::SizeMismatch), "{claimed}");
            assert!(!models.path("tiny").exists());
            assert!(
                !part_file(&models).exists(),
                "it cannot be trusted, so it is dropped"
            );
        }
    }

    #[test]
    fn progress_is_reported_every_quarter_second_or_percent() {
        let t0 = Instant::now();
        let total = 1_000_000;
        let mut gate = ProgressGate::new(t0, 0);
        let ms = |n| t0 + Duration::from_millis(n);

        assert!(!gate.due(ms(10), 5_000, total), "0.5% and 10 ms");
        assert!(gate.due(ms(20), 10_000, total), "1% of the file");
        assert!(
            !gate.due(ms(30), 15_000, total),
            "1% is counted from the last report"
        );
        assert!(
            gate.due(ms(30 + 250), 15_001, total),
            "250 ms since the last report"
        );
        assert!(!gate.due(ms(300), 15_002, total));
        assert!(gate.due(ms(300), 25_002, total));
    }

    #[test]
    fn a_tiny_or_empty_total_does_not_break_the_gate() {
        let t0 = Instant::now();
        let mut gate = ProgressGate::new(t0, 0);
        assert!(
            gate.due(t0, 1, 50),
            "1% of 50 bytes is under a byte: report per byte"
        );
        assert!(!gate.due(t0, 1, 0), "an empty total cannot divide by zero");
    }

    #[test]
    fn a_content_range_is_read_in_all_its_forms() {
        assert_eq!(
            parse_content_range("bytes 5-9/10"),
            Some(ContentRange {
                start: Some(5),
                total: Some(10)
            })
        );
        assert_eq!(
            parse_content_range("bytes 5-9/*"),
            Some(ContentRange {
                start: Some(5),
                total: None
            })
        );
        assert_eq!(
            parse_content_range("bytes */10"),
            Some(ContentRange {
                start: None,
                total: Some(10)
            })
        );
        for bad in [
            "",
            "bytes",
            "bytes 5-9",
            "items 5-9/10",
            "bytes a-b/c",
            "bytes 5-9/x",
        ] {
            assert_eq!(parse_content_range(bad), None, "{bad:?}");
        }
    }

    /// Redirects `hops` times, through `/hop/1`, `/hop/2`..., with `status`, and then serves `file` like
    /// `RangeServer`.
    struct RedirectChain {
        hops: usize,
        status: u16,
        file: Vec<u8>,
    }

    impl Respond for RedirectChain {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let at = request
                .url
                .path()
                .strip_prefix("/hop/")
                .and_then(|n| n.parse::<usize>().ok())
                .unwrap_or(0);
            if at < self.hops {
                ResponseTemplate::new(self.status)
                    .insert_header("location", format!("/hop/{}", at + 1))
            } else {
                RangeServer(self.file.clone()).respond(request)
            }
        }
    }

    async fn serve_chain(hops: usize, status: u16) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(RedirectChain {
                hops,
                status,
                file: body(MIB),
            })
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn follows_a_redirect_to_the_file() {
        // Hugging Face answers with a 302 to its CDN.
        let (_tmp, models) = dir();
        let server = serve_chain(1, 302).await;

        let (result, _) = download_tiny(&models, &server.uri()).await;

        assert_eq!(result.unwrap(), models.path("tiny"));
        assert_eq!(fs::read(models.path("tiny")).unwrap(), body(MIB));
        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests.iter().map(|r| r.url.path()).collect::<Vec<_>>(),
            ["/ggml-tiny.bin", "/hop/1"]
        );
        for request in &requests {
            for name in ["authorization", "cookie", "x-api-key"] {
                assert!(request.headers.get(name).is_none(), "{name}");
            }
        }
    }

    #[tokio::test]
    async fn every_redirect_status_is_followed() {
        for status in [301, 302, 303, 307, 308] {
            let (_tmp, models) = dir();
            let server = serve_chain(1, status).await;

            let (result, _) = download_tiny(&models, &server.uri()).await;

            assert!(result.is_ok(), "{status}: {result:?}");
        }
    }

    #[tokio::test]
    async fn the_range_header_goes_along_to_the_redirect_target() {
        let (_tmp, models) = dir();
        fs::write(part_file(&models), &body(MIB)[..100 * KIB]).unwrap();
        let server = serve_chain(2, 302).await;

        let (result, calls) = download_tiny(&models, &server.uri()).await;

        assert_eq!(result.unwrap(), models.path("tiny"));
        assert_eq!(fs::read(models.path("tiny")).unwrap(), body(MIB));
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 3);
        for request in &requests {
            assert_eq!(range_header(request).as_deref(), Some("bytes=102400-"));
        }
        assert_eq!(calls.first(), Some(&(102_400, Some(MIB as u64))));
    }

    #[tokio::test]
    async fn an_absolute_location_on_the_same_origin_is_followed() {
        let (_tmp, models) = dir();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/ggml-tiny.bin"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", format!("{}/cdn/blob", server.uri())),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/cdn/blob"))
            .respond_with(RangeServer(body(KIB)))
            .mount(&server)
            .await;

        let (result, _) = download_tiny(&models, &server.uri()).await;

        assert_eq!(result.unwrap(), models.path("tiny"));
        assert_eq!(fs::read(models.path("tiny")).unwrap(), body(KIB));
    }

    #[tokio::test]
    async fn five_redirects_are_followed_a_sixth_is_not() {
        let (_tmp, models) = dir();
        let server = serve_chain(5, 302).await;
        let (result, _) = download_tiny(&models, &server.uri()).await;
        assert!(result.is_ok(), "{result:?}");

        let (_tmp, models) = dir();
        let server = serve_chain(6, 302).await;
        let (result, calls) = download_tiny(&models, &server.uri()).await;
        assert!(
            matches!(result, Err(DownloadError::Http(ApiError::Invalid(_)))),
            "{result:?}"
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 6);
        assert!(calls.is_empty());
        assert!(!models.path("tiny").exists());
    }

    #[tokio::test]
    async fn a_redirect_to_another_origin_over_plain_http_is_refused() {
        let (_tmp, models) = dir();
        let elsewhere = serve(RangeServer(body(KIB))).await;
        let origin = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", format!("{}/ggml-tiny.bin", elsewhere.uri())),
            )
            .mount(&origin)
            .await;

        let (result, _) = download_tiny(&models, &origin.uri()).await;

        assert!(
            matches!(result, Err(DownloadError::Http(ApiError::Invalid(_)))),
            "{result:?}"
        );
        assert!(elsewhere.received_requests().await.unwrap().is_empty());
        assert!(!models.path("tiny").exists());
        assert!(!part_file(&models).exists());
    }

    #[tokio::test]
    async fn a_redirect_to_an_https_address_is_followed() {
        // Nothing listens there, so the failure is the connection's: the redirect was taken.
        let (_tmp, models) = dir();
        let origin = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", "https://127.0.0.1:1/ggml-tiny.bin"),
            )
            .mount(&origin)
            .await;

        let (result, _) = download_tiny(&models, &origin.uri()).await;

        assert!(
            matches!(result, Err(DownloadError::Http(ApiError::Network(_)))),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn a_redirect_without_a_usable_location_is_an_error() {
        for location in [None, Some("http://[::1"), Some("")] {
            let (_tmp, models) = dir();
            let origin = MockServer::start().await;
            let mut reply = ResponseTemplate::new(302);
            if let Some(location) = location {
                reply = reply.insert_header("location", location);
            }
            Mock::given(method("GET"))
                .respond_with(reply)
                .mount(&origin)
                .await;

            let (result, _) = download_tiny(&models, &origin.uri()).await;

            assert!(
                matches!(result, Err(DownloadError::Http(ApiError::Invalid(_)))),
                "{location:?}: {result:?}"
            );
        }
    }

    #[test]
    fn redirects_go_to_https_or_stay_on_the_origin() {
        let url = |u: &str| reqwest::Url::parse(u).unwrap();
        let allowed = |from: &str, to: &str| redirect_allowed(&url(from), &url(to));

        assert!(allowed(
            "https://huggingface.co/a",
            "https://cas-bridge.xethub.hf.co/b?sig=1"
        ));
        assert!(allowed(
            "http://localhost:8080/a",
            "https://cdn.example.com/b"
        ));
        assert!(allowed(
            "http://localhost:8080/a",
            "http://localhost:8080/b"
        ));
        assert!(allowed("https://example.com/a", "https://example.com/b"));

        assert!(
            !allowed("http://localhost:8080/a", "http://localhost:9090/a"),
            "other port"
        );
        assert!(
            !allowed("http://localhost:8080/a", "http://example.com:8080/a"),
            "other host"
        );
        assert!(
            !allowed("https://example.com/a", "http://example.com/a"),
            "https to http"
        );
        assert!(!allowed(
            "https://example.com/a",
            "http://cdn.example.com/a"
        ));
        assert!(!allowed("https://example.com/a", "ftp://example.com/a"));
        assert!(!allowed("https://example.com/a", "file:///etc/passwd"));
    }

    // ---- download_file: the same download for a file with a fixed address and size

    const BLOB: &str = "model.onnx";

    /// A `RemoteFile` for a test server: its address is `'static` in the app, so the test's is leaked.
    fn remote(url: String, size: usize) -> RemoteFile {
        RemoteFile {
            url: Box::leak(url.into_boxed_str()),
            file: BLOB,
            size: size as u64,
        }
    }

    fn blob_path(models: &ModelDir) -> PathBuf {
        models.root().join(BLOB)
    }

    /// Serves `body` as `at` the way `RangeServer` does.
    async fn serve_at(at: &str, responder: impl Respond + 'static) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(at))
            .respond_with(responder)
            .mount(&server)
            .await;
        server
    }

    /// Redirects `/model.onnx` to `target`.
    async fn redirect_to(target: String) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/model.onnx"))
            .respond_with(ResponseTemplate::new(302).insert_header("location", target))
            .mount(&server)
            .await;
        server
    }

    /// Stands in for the rule of the app where the CDN is a second test server: the app's rule, and any plain-http
    /// address on this machine.
    fn cdn_stand_in(from: &reqwest::Url, to: &reqwest::Url) -> bool {
        redirect_allowed(from, to) || (to.scheme() == "http" && to.host_str() == Some("127.0.0.1"))
    }

    async fn fetch_remote(
        models: &ModelDir,
        file: &RemoteFile,
        redirect_ok: RedirectRule,
    ) -> (Result<PathBuf, DownloadError>, Vec<(u64, Option<u64>)>) {
        let (on_progress, calls) = recorder();
        let result = download_file_with(
            models.root(),
            file,
            on_progress,
            CancellationToken::new(),
            READ_TIMEOUT,
            redirect_ok,
        )
        .await;
        let calls = calls.lock().unwrap().clone();
        (result, calls)
    }

    #[tokio::test]
    async fn download_file_follows_cdn_redirect_and_checks_size() {
        // Hugging Face and GitHub answer with a 302 to a CDN that sends the bytes.
        let (_tmp, models) = dir();
        let cdn = serve_at("/blob/model.onnx", RangeServer(body(MIB))).await;
        let origin = redirect_to(format!("{}/blob/model.onnx", cdn.uri())).await;
        let file = remote(format!("{}/model.onnx", origin.uri()), MIB);

        let (result, calls) = fetch_remote(&models, &file, cdn_stand_in).await;

        assert_eq!(result.unwrap(), blob_path(&models));
        assert_eq!(fs::read(blob_path(&models)).unwrap(), body(MIB));
        assert!(!models.root().join("model.onnx.part").exists());
        assert_eq!(calls.first(), Some(&(0, Some(MIB as u64))));
        assert_eq!(calls.last(), Some(&(MIB as u64, Some(MIB as u64))));
        assert_eq!(origin.received_requests().await.unwrap().len(), 1);
        let requests = cdn.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        for name in ["authorization", "cookie", "x-api-key"] {
            assert!(requests[0].headers.get(name).is_none(), "{name}");
        }

        // The size the file is declared to have is checked against what the CDN announces.
        for wrong in [MIB + 1, MIB - 1] {
            let (_tmp, models) = dir();
            let file = remote(format!("{}/model.onnx", origin.uri()), wrong);

            let (result, calls) = fetch_remote(&models, &file, cdn_stand_in).await;

            assert_eq!(result, Err(DownloadError::SizeMismatch), "{wrong}");
            assert!(calls.is_empty(), "nothing is written for it: {calls:?}");
            assert!(!blob_path(&models).exists());
            assert!(!models.root().join("model.onnx.part").exists());
        }
    }

    #[tokio::test]
    async fn download_file_resumes_a_part_file_through_the_redirect() {
        let (_tmp, models) = dir();
        fs::write(
            models.root().join("model.onnx.part"),
            &body(MIB)[..100 * KIB],
        )
        .unwrap();
        let cdn = serve_at("/blob/model.onnx", RangeServer(body(MIB))).await;
        let origin = redirect_to(format!("{}/blob/model.onnx", cdn.uri())).await;
        let file = remote(format!("{}/model.onnx", origin.uri()), MIB);

        let (result, calls) = fetch_remote(&models, &file, cdn_stand_in).await;

        assert_eq!(result.unwrap(), blob_path(&models));
        assert_eq!(fs::read(blob_path(&models)).unwrap(), body(MIB));
        let requests = cdn.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1, "the rest only");
        assert_eq!(range_header(&requests[0]).as_deref(), Some("bytes=102400-"));
        assert_eq!(calls.first(), Some(&(102_400, Some(MIB as u64))));
    }

    #[tokio::test]
    async fn download_file_drops_a_part_file_that_is_not_this_file() {
        // The part holds 100 KiB, and the server says the file is another length than the one expected.
        let (_tmp, models) = dir();
        fs::write(
            models.root().join("model.onnx.part"),
            &body(MIB)[..100 * KIB],
        )
        .unwrap();
        let server = serve_at("/model.onnx", RangeServer(body(MIB))).await;
        let file = remote(format!("{}/model.onnx", server.uri()), MIB + KIB);

        let (result, _) = fetch_remote(&models, &file, redirect_allowed).await;

        assert_eq!(result, Err(DownloadError::SizeMismatch));
        assert!(!blob_path(&models).exists());
        assert!(!models.root().join("model.onnx.part").exists());
    }

    #[tokio::test]
    async fn download_file_follows_only_the_redirects_the_rule_allows() {
        let (_tmp, models) = dir();
        let elsewhere = serve_at("/blob/model.onnx", RangeServer(body(KIB))).await;
        let origin = redirect_to(format!("{}/blob/model.onnx", elsewhere.uri())).await;
        let file = remote(format!("{}/model.onnx", origin.uri()), KIB);

        // The app's rule: another origin over plain http is refused.
        let (result, _) = fetch_remote(&models, &file, redirect_allowed).await;

        assert!(
            matches!(result, Err(DownloadError::Http(ApiError::Invalid(_)))),
            "{result:?}"
        );
        assert!(elsewhere.received_requests().await.unwrap().is_empty());
        assert!(!blob_path(&models).exists());
    }

    #[test]
    fn the_model_cdns_are_reached_over_https_only() {
        let url = |u: &str| reqwest::Url::parse(u).unwrap();
        let allowed = |from: &str, to: &str| redirect_allowed(&url(from), &url(to));
        let hf = "https://huggingface.co/csukuangfj/m/resolve/main/model.onnx";
        let github = "https://github.com/k2-fsa/sherpa-onnx/releases/download/t/m.onnx";

        // Hugging Face sends the file from a host below hf.co, and GitHub release assets come from these two.
        assert!(allowed(
            hf,
            "https://cas-bridge.xethub.hf.co/xet-bridge-us/abc?X-Amz-Signature=1"
        ));
        assert!(allowed(
            github,
            "https://objects.githubusercontent.com/github-production-release-asset/1?sig=1"
        ));
        assert!(allowed(
            github,
            "https://release-assets.githubusercontent.com/github-production-release-asset/1?sig=1"
        ));

        for plain in [
            "http://cas-bridge.xethub.hf.co/abc",
            "http://objects.githubusercontent.com/abc",
            "http://release-assets.githubusercontent.com/abc",
        ] {
            assert!(!allowed(hf, plain), "{plain}");
            assert!(!allowed(github, plain), "{plain}");
        }
    }

    #[test]
    fn download_file_can_run_on_another_thread() {
        fn assert_send<T: Send>(_: &T) {}
        let file = RemoteFile {
            url: "http://localhost/model.onnx",
            file: BLOB,
            size: 1,
        };
        let future = download_file(Path::new(""), &file, |_, _| {}, CancellationToken::new());
        assert_send(&future);
    }

    /// Fails the test instead of hanging it when a download that should end does not.
    async fn within_10_seconds<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::time::timeout(Duration::from_secs(10), future)
            .await
            .expect("the download did not end")
    }

    #[tokio::test]
    async fn a_stalled_transfer_times_out_and_keeps_what_arrived() {
        let (_tmp, models) = dir();
        let uri = raw_server(short_reply(MIB, 100 * KIB), Ending::Stall).await;

        let result = within_10_seconds(download_with(
            &models,
            "tiny",
            &uri,
            |_, _| {},
            CancellationToken::new(),
            Duration::from_millis(300),
        ))
        .await;

        let Err(DownloadError::Http(ApiError::Network(message))) = result else {
            panic!("expected a network error, got {result:?}");
        };
        assert!(message.starts_with("stalled"), "{message}");
        assert!(!models.is_downloaded("tiny"));
        assert_eq!(fs::read(part_file(&models)).unwrap(), body(100 * KIB));
    }

    #[tokio::test]
    async fn a_server_that_never_answers_times_out() {
        let (_tmp, models) = dir();
        let uri = raw_server(Vec::new(), Ending::Stall).await;

        let result = within_10_seconds(download_with(
            &models,
            "tiny",
            &uri,
            |_, _| {},
            CancellationToken::new(),
            Duration::from_millis(300),
        ))
        .await;

        let Err(DownloadError::Http(ApiError::Network(message))) = result else {
            panic!("expected a network error, got {result:?}");
        };
        assert!(message.starts_with("stalled"), "{message}");
        assert!(!part_file(&models).exists());
    }

    #[tokio::test]
    async fn a_slow_but_steady_transfer_is_not_a_stall() {
        // The limit is per read, not for the whole download: 3 pieces 150 ms apart under a 400 ms limit.
        let (_tmp, models) = dir();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let uri = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf).await;
            let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", 3 * KIB);
            let _ = stream.write_all(head.as_bytes()).await;
            for piece in body(3 * KIB).chunks(KIB) {
                let _ = stream.write_all(piece).await;
                let _ = stream.flush().await;
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
        });

        let result = within_10_seconds(download_with(
            &models,
            "tiny",
            &uri,
            |_, _| {},
            CancellationToken::new(),
            Duration::from_millis(400),
        ))
        .await;

        assert_eq!(result.unwrap(), models.path("tiny"));
        assert_eq!(fs::read(models.path("tiny")).unwrap(), body(3 * KIB));
    }

    #[tokio::test]
    async fn cancel_stops_and_keeps_part() {
        let (_tmp, models) = dir();
        // The server stalls after 100 KiB: only the cancel can end this download in time.
        let uri = raw_server(short_reply(MIB, 100 * KIB), Ending::Stall).await;
        let cancel = CancellationToken::new();
        let stop = cancel.clone();

        let result = within_10_seconds(download(
            &models,
            "tiny",
            &uri,
            move |done, _| {
                if done > 0 {
                    stop.cancel();
                }
            },
            cancel,
        ))
        .await;

        assert_eq!(result, Err(DownloadError::Cancelled));
        assert!(!models.is_downloaded("tiny"));
        let kept = fs::read(part_file(&models)).unwrap();
        assert!(
            !kept.is_empty() && kept.len() <= 100 * KIB,
            "{} bytes kept",
            kept.len()
        );
        assert_eq!(
            kept,
            body(kept.len()),
            "what is kept is a true start of the file"
        );
    }

    #[tokio::test]
    async fn a_cancelled_download_can_be_resumed() {
        let (_tmp, models) = dir();
        let stalling = raw_server(short_reply(MIB, 100 * KIB), Ending::Stall).await;
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        let first = within_10_seconds(download(
            &models,
            "tiny",
            &stalling,
            move |done, _| {
                if done > 0 {
                    stop.cancel();
                }
            },
            cancel,
        ))
        .await;
        assert_eq!(first, Err(DownloadError::Cancelled));
        let kept = fs::metadata(part_file(&models)).unwrap().len();

        let server = serve(RangeServer(body(MIB))).await;
        let (second, calls) = download_tiny(&models, &server.uri()).await;

        assert_eq!(second.unwrap(), models.path("tiny"));
        assert_eq!(fs::read(models.path("tiny")).unwrap(), body(MIB));
        assert_eq!(calls.first(), Some(&(kept, Some(MIB as u64))));
    }

    #[tokio::test]
    async fn cancelling_while_waiting_for_the_server_ends_the_download() {
        let (_tmp, models) = dir();
        let uri = raw_server(Vec::new(), Ending::Stall).await;
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            stop.cancel();
        });

        let result = within_10_seconds(download(&models, "tiny", &uri, |_, _| {}, cancel)).await;

        assert_eq!(result, Err(DownloadError::Cancelled));
        assert!(!models.is_downloaded("tiny"));
    }

    #[tokio::test]
    async fn a_token_cancelled_beforehand_downloads_nothing() {
        let (_tmp, models) = dir();
        let server = serve(RangeServer(body(KIB))).await;
        let cancel = CancellationToken::new();
        cancel.cancel();

        let result = download(&models, "tiny", &server.uri(), |_, _| {}, cancel).await;

        assert_eq!(result, Err(DownloadError::Cancelled));
        assert!(server.received_requests().await.unwrap().is_empty());
        assert!(!part_file(&models).exists());
    }

    #[tokio::test]
    async fn an_empty_reply_is_not_a_model() {
        let (_tmp, models) = dir();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let (result, _) = download_tiny(&models, &server.uri()).await;

        assert_eq!(result, Err(DownloadError::SizeMismatch));
        assert!(!models.path("tiny").exists());
        assert!(!part_file(&models).exists());
    }
}
