//! The loopback server that plays a recording's media to the meetings window.
//!
//! The media of a recording is anywhere on disk (an imported file is never copied), and WebKitGTK cannot play from the
//! `asset:` scheme, so every system plays from `http://127.0.0.1:<port>/<token>/<n>` instead. The server answers GET and
//! HEAD for the files that were given an address (`MediaServer::url_for`) and for nothing else; `Range` is served, which
//! an `audio.m4a` needs: the system's own encoder writes its `moov` atom at the end of the file, so a player has to ask
//! for the end first.
//!
//! The address holds a random 128-bit token, so that another program or a web page can't ask for a file by guessing: a
//! wrong token and a number that was never given out are the same 404.
//!
//! tiny_http's accept thread ends at its first error and takes the listener with it (a player that bails out of a seek can
//! cause one, and so can running out of file handles); `recv` then answers that one error and waits for ever. So the server
//! makes another listener on a fresh port when `recv` fails, with the same token and the same numbers. **The addresses given
//! out before that stop working**, and `url_for` gives the new port: whoever plays from an address (the window) asks for a
//! fresh one when playing fails.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

/// How many bytes of randomness the token has.
const TOKEN_BYTES: usize = 16;

/// tiny_http sends a body of 32 KiB or more as `Transfer-Encoding: chunked`, with no `Content-Length`, unless it is given a
/// threshold that no body reaches. A player needs the length to know how long a recording is and to seek in it.
const NEVER_CHUNKED: usize = usize::MAX;

/// The wait after the server could not take a request or listen again, and the longest it grows to.
const RETRY_FIRST: Duration = Duration::from_millis(100);
const RETRY_LONGEST: Duration = Duration::from_secs(5);

/// Makes a tiny_http server on a fresh port of 127.0.0.1. The tests give one that sometimes fails.
type Bind = dyn Fn() -> io::Result<Server> + Send + Sync;

/// Waits for the given time. The tests give one that only notes it.
type Pause = dyn Fn(Duration) + Send + Sync;

pub struct MediaServer {
    shared: Arc<Shared>,
}

/// What the thread that takes requests, the threads that answer them and `url_for` share.
struct Shared {
    token: String,
    /// The files that have an address: the address of the one at place `n` ends in `/n`. They stay when the listener is made
    /// again.
    paths: Mutex<Vec<PathBuf>>,
    live: Mutex<Live>,
    /// Told whenever `live` holds another server.
    replaced: Condvar,
    stopped: AtomicBool,
}

/// The listener that takes requests now.
struct Live {
    server: Arc<Server>,
    port: u16,
    /// How many times the listener was made again.
    generation: u64,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A server on a free port of 127.0.0.1, and the port.
fn bind_loopback() -> io::Result<Server> {
    Server::http("127.0.0.1:0").map_err(io::Error::other)
}

fn port_of(server: &Server) -> io::Result<u16> {
    server
        .server_addr()
        .to_ip()
        .map(|address| address.port())
        .ok_or_else(|| io::Error::other("the media server has no port"))
}

impl MediaServer {
    /// Starts the server on a free port of 127.0.0.1, on a thread of its own.
    pub fn start() -> io::Result<MediaServer> {
        Self::start_with(Arc::new(bind_loopback), Arc::new(thread::sleep))
    }

    fn start_with(bind: Arc<Bind>, pause: Arc<Pause>) -> io::Result<MediaServer> {
        let server = bind()?;
        let port = port_of(&server)?;
        let shared = Arc::new(Shared {
            token: new_token()?,
            paths: Mutex::new(Vec::new()),
            live: Mutex::new(Live {
                server: Arc::new(server),
                port,
                generation: 0,
            }),
            replaced: Condvar::new(),
            stopped: AtomicBool::new(false),
        });
        let worker = shared.clone();
        thread::Builder::new()
            .name("media-server".into())
            .spawn(move || serve(&worker, &*bind, &*pause))?;
        Ok(MediaServer { shared })
    }

    /// The address of `path`, which the server serves from now on: the same one each time for the same path, as long as the
    /// listener is the same. After it was made again the address is on its new port, and the one from before is dead.
    pub fn url_for(&self, path: &Path) -> String {
        let n = {
            let mut paths = lock(&self.shared.paths);
            match paths.iter().position(|known| known == path) {
                Some(n) => n,
                None => {
                    paths.push(path.to_path_buf());
                    paths.len() - 1
                }
            }
        };
        let port = lock(&self.shared.live).port;
        format!("http://127.0.0.1:{port}/{}/{n}", self.shared.token)
    }
}

#[cfg(test)]
impl MediaServer {
    /// Makes the thread that takes requests meet an error, as an accept error does. (The listener of tiny_http stays, so
    /// the old address answers until the server is made again, which is when it is let go of.)
    fn fail_accept(&self) {
        lock(&self.shared.live).server.unblock();
    }

    /// Waits until the listener was made again `generation` times.
    fn wait_for_generation(&self, generation: u64) {
        let mut live = lock(&self.shared.live);
        while live.generation < generation {
            // The limit is only a net under the test: the server is made again at once.
            let (next, waited) = self
                .shared
                .replaced
                .wait_timeout(live, Duration::from_secs(30))
                .unwrap_or_else(PoisonError::into_inner);
            live = next;
            assert!(
                !waited.timed_out() || live.generation >= generation,
                "the listener was not made again"
            );
        }
    }
}

impl Drop for MediaServer {
    fn drop(&mut self) {
        // Before the listener is read: a thread that is making another one then sees that it is to stop, or the one it made is
        // the one that is unblocked here.
        self.shared.stopped.store(true, Ordering::SeqCst);
        lock(&self.shared.live).server.unblock();
    }
}

/// A random token of `TOKEN_BYTES` bytes from the system, as hex.
fn new_token() -> io::Result<String> {
    let mut bytes = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes)
        .map_err(|e| io::Error::other(format!("the system gave no random bytes: {e}")))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Takes the requests one by one until the server is stopped, making the listener again when it fails. Each request is
/// answered on a thread of its own: a player that is seeking in a long recording asks for the end of the file while the
/// start is still being sent, and a client that is slow to read must not hold up the others.
fn serve(shared: &Arc<Shared>, bind: &Bind, pause: &Pause) {
    let mut failing = Backoff::default();
    let mut server = lock(&shared.live).server.clone();
    loop {
        match server.recv() {
            Ok(request) => {
                failing.recovered();
                let shared = shared.clone();
                let spawned = thread::Builder::new()
                    .name("media-answer".into())
                    .spawn(move || answer(request, &shared));
                if let Err(e) = spawned {
                    log::warn!("The media server could not start a thread: {e}");
                }
            }
            Err(_) if shared.stopped.load(Ordering::SeqCst) => return,
            Err(e) => {
                let (wait, first) = failing.failed();
                // Once for a run of failures: a listener that keeps failing would fill the log.
                if first {
                    log::warn!(
                        "The media server stopped taking requests ({e}): it starts again on another port, and the addresses given out before stop working"
                    );
                }
                pause(wait);
                match listen_again(shared, bind, pause, &mut failing, server) {
                    Some(fresh) => server = fresh,
                    None => return,
                }
            }
        }
    }
}

/// Makes another server, waiting longer after each try that fails, and puts it where `url_for` finds it. `old` is let go of
/// first, so that when the new address is out the old one is closed. `None` when the server was stopped meanwhile.
fn listen_again(
    shared: &Shared,
    bind: &Bind,
    pause: &Pause,
    failing: &mut Backoff,
    old: Arc<Server>,
) -> Option<Arc<Server>> {
    let mut old = Some(old);
    let mut said = false;
    loop {
        if shared.stopped.load(Ordering::SeqCst) {
            return None;
        }
        match bind().and_then(|server| port_of(&server).map(|port| (server, port))) {
            Ok((fresh, port)) => {
                let fresh = Arc::new(fresh);
                let mut live = lock(&shared.live);
                // The last holders of the old server, which closes its listener when it is dropped.
                drop(old.take());
                drop(std::mem::replace(&mut live.server, fresh.clone()));
                live.port = port;
                live.generation += 1;
                shared.replaced.notify_all();
                drop(live);
                // `Drop` sets the flag before it reads `live`: either it read this server and unblocks it, or the flag is
                // set by now.
                return (!shared.stopped.load(Ordering::SeqCst)).then_some(fresh);
            }
            Err(e) => {
                if !said {
                    log::warn!("The media server could not listen again: {e}");
                    said = true;
                }
                let (wait, _) = failing.failed();
                pause(wait);
            }
        }
    }
}

/// How long to wait after the server failed to take a request or to listen again: a little at first, twice as long each time it
/// fails again, and never more than `RETRY_LONGEST`.
#[derive(Default)]
struct Backoff {
    failures: u32,
}

impl Backoff {
    /// How long to wait now, and whether this is the first failure of a run (the only one that is logged).
    fn failed(&mut self) -> (Duration, bool) {
        let first = self.failures == 0;
        let wait = RETRY_FIRST
            .saturating_mul(2u32.saturating_pow(self.failures))
            .min(RETRY_LONGEST);
        self.failures = self.failures.saturating_add(1);
        (wait, first)
    }

    /// A request was taken: the next failure starts a new run.
    fn recovered(&mut self) {
        self.failures = 0;
    }
}

/// What the `Range` header of a request asks for, of a file that is `len` bytes long.
#[derive(Debug, PartialEq, Eq)]
enum Wanted {
    /// All of it: no range, or one that is not a range of bytes (which a server may answer in full).
    Whole,
    /// These bytes, the first and the last of them.
    Part(u64, u64),
    /// A range that starts past the end.
    Unsatisfiable,
}

/// `Range: bytes=a-b`, `bytes=a-` and `bytes=-n`. Anything else (another unit, several ranges, an end before the start)
/// is not served as a range.
fn wanted(header: Option<&str>, len: u64) -> Wanted {
    let Some(spec) = header.and_then(|value| value.trim().strip_prefix("bytes=")) else {
        return Wanted::Whole;
    };
    let Some((first, last)) = spec.trim().split_once('-') else {
        return Wanted::Whole;
    };
    let (first, last) = (first.trim(), last.trim());
    let range = match (first.is_empty(), last.is_empty()) {
        // The last `n` bytes.
        (true, false) => match last.parse::<u64>() {
            Ok(0) => return Wanted::Unsatisfiable,
            Ok(n) => (len.saturating_sub(n), len.saturating_sub(1)),
            Err(_) => return Wanted::Whole,
        },
        // From a place to the end.
        (false, true) => match first.parse::<u64>() {
            Ok(from) => (from, len.saturating_sub(1)),
            Err(_) => return Wanted::Whole,
        },
        (false, false) => match (first.parse::<u64>(), last.parse::<u64>()) {
            (Ok(from), Ok(to)) if from <= to => (from, to.min(len.saturating_sub(1))),
            _ => return Wanted::Whole,
        },
        (true, true) => return Wanted::Whole,
    };
    if range.0 >= len {
        Wanted::Unsatisfiable
    } else {
        Wanted::Part(range.0, range.1)
    }
}

/// The type of the file by its extension, which is what the player goes by.
fn content_type(path: &Path) -> &'static str {
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase());
    match extension.as_deref() {
        Some("m4a") => "audio/mp4",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("webm") => "video/webm",
        Some("mp4" | "m4v") => "video/mp4",
        Some("mov") => "video/quicktime",
        _ => "application/octet-stream",
    }
}

/// A header of ASCII text that this file writes itself.
fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name, value).expect("a header of ASCII text")
}

/// The file at the place that a request names, if it names one this server gave out.
fn registered(url: &str, shared: &Shared) -> Option<PathBuf> {
    let path = url.split(['?', '#']).next()?.strip_prefix('/')?;
    let (given, n) = path.split_once('/')?;
    if !same(given, &shared.token) {
        return None;
    }
    let n: usize = n.parse().ok()?;
    lock(&shared.paths).get(n).cloned()
}

/// Whether two strings are equal, in a time that does not tell how much of the first one is right.
fn same(given: &str, token: &str) -> bool {
    given.len() == token.len()
        && given
            .bytes()
            .zip(token.bytes())
            .fold(0, |different, (a, b)| different | (a ^ b))
            == 0
}

fn answer(request: Request, shared: &Shared) {
    let found = matches!(request.method(), Method::Get | Method::Head)
        .then(|| registered(request.url(), shared))
        .flatten()
        .and_then(|path| File::open(&path).ok().map(|file| (path, file)))
        .and_then(|(path, file)| {
            let len = file.metadata().ok().filter(|meta| meta.is_file())?.len();
            Some((path, file, len))
        });
    let Some((path, mut file, len)) = found else {
        // A client that went away is no concern of ours.
        let _ = request.respond(Response::empty(StatusCode(404)));
        return;
    };

    let range = request
        .headers()
        .iter()
        .find(|header| header.field.equiv("Range"))
        .map(|header| header.value.as_str().to_owned());
    let headers = |extra: Vec<Header>| {
        let mut all = vec![
            header("Content-Type", content_type(&path)),
            header("Accept-Ranges", "bytes"),
            header("X-Content-Type-Options", "nosniff"),
        ];
        all.extend(extra);
        all
    };
    let sent = match wanted(range.as_deref(), len) {
        Wanted::Whole => request.respond(
            Response::new(
                StatusCode(200),
                headers(Vec::new()),
                file,
                usize::try_from(len).ok(),
                None,
            )
            .with_chunked_threshold(NEVER_CHUNKED),
        ),
        Wanted::Part(first, last) => {
            let size = last - first + 1;
            if file.seek(SeekFrom::Start(first)).is_err() {
                let _ = request.respond(Response::empty(StatusCode(500)));
                return;
            }
            request.respond(
                Response::new(
                    StatusCode(206),
                    headers(vec![header(
                        "Content-Range",
                        &format!("bytes {first}-{last}/{len}"),
                    )]),
                    file.take(size),
                    usize::try_from(size).ok(),
                    None,
                )
                .with_chunked_threshold(NEVER_CHUNKED),
            )
        }
        Wanted::Unsatisfiable => request.respond(
            Response::empty(StatusCode(416))
                .with_header(header("Content-Range", &format!("bytes */{len}"))),
        ),
    };
    if let Err(e) = sent {
        // The player closed the connection (it does that whenever it seeks): not worth more than a debug line.
        log::debug!("A media answer was cut short: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::sync::atomic::AtomicUsize;

    /// What a request got: the status, the headers (names in lower case) and the body.
    struct Reply {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    impl Reply {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.as_str())
        }
    }

    /// A request over a plain socket, as a browser's media element makes it: `url` is `http://127.0.0.1:<port>/<path>`.
    fn ask(url: &str, method: &str, headers: &[(&str, &str)]) -> Reply {
        let rest = url.strip_prefix("http://").expect("an http address");
        let (host, path) = rest.split_once('/').expect("a path");
        let mut stream = TcpStream::connect(host).expect("the server is listening");
        let mut head =
            format!("{method} /{path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
        for (name, value) in headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes()).unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();

        let split = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("the end of the head");
        let head = String::from_utf8_lossy(&raw[..split]).into_owned();
        let mut lines = head.split("\r\n");
        let status = lines
            .next()
            .and_then(|line| line.split(' ').nth(1))
            .and_then(|code| code.parse().ok())
            .expect("a status line");
        let headers = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
            .collect();
        Reply {
            status,
            headers,
            body: raw[split + 4..].to_vec(),
        }
    }

    fn get(url: &str) -> Reply {
        ask(url, "GET", &[])
    }

    fn range(url: &str, value: &str) -> Reply {
        ask(url, "GET", &[("Range", value)])
    }

    /// A file of 100 bytes, each the number of its place.
    fn counted(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, (0..100u8).collect::<Vec<_>>()).unwrap();
        path
    }

    #[test]
    fn media_server_serves_range_and_404s_unregistered() {
        let dir = tempfile::tempdir().unwrap();
        let file = counted(dir.path(), "audio.m4a");
        let server = MediaServer::start().unwrap();
        let url = server.url_for(&file);

        let part = range(&url, "bytes=10-19");
        assert_eq!(part.status, 206);
        assert_eq!(part.body, (10..20u8).collect::<Vec<_>>());
        assert_eq!(part.header("content-range"), Some("bytes 10-19/100"));
        assert_eq!(part.header("content-length"), Some("10"));
        assert_eq!(part.header("accept-ranges"), Some("bytes"));

        // The whole file, for a request with no range.
        let whole = get(&url);
        assert_eq!(whole.status, 200);
        assert_eq!(whole.body, (0..100u8).collect::<Vec<_>>());
        assert_eq!(whole.header("accept-ranges"), Some("bytes"));
        assert_eq!(whole.header("content-type"), Some("audio/mp4"));

        // A wrong token and a number that was never given out are the same 404.
        let (prefix, _) = url.rsplit_once('/').unwrap();
        let (base, token) = prefix.rsplit_once('/').unwrap();
        assert_eq!(
            get(&format!("{base}/{}/0", "0".repeat(token.len()))).status,
            404
        );
        assert_eq!(get(&format!("{prefix}/1")).status, 404);
        assert_eq!(get(&format!("{prefix}/x")).status, 404);
        assert_eq!(get(&format!("{base}/{token}")).status, 404);
        assert_eq!(get(&format!("{base}/")).status, 404);

        // HEAD says how long the file is and sends no body.
        let head = ask(&url, "HEAD", &[]);
        assert_eq!(head.status, 200);
        assert_eq!(head.header("content-length"), Some("100"));
        assert!(head.body.is_empty());
        let head = ask(&url, "HEAD", &[("Range", "bytes=10-19")]);
        assert_eq!(head.status, 206);
        assert_eq!(head.header("content-length"), Some("10"));
        assert!(head.body.is_empty());
    }

    #[test]
    fn media_server_serves_non_ascii_paths_with_range() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Cuộc họp 1");
        fs::create_dir_all(&folder).unwrap();
        let file = counted(&folder, "âm thanh.m4a");
        let server = MediaServer::start().unwrap();
        let url = server.url_for(&file);

        // The address is plain ASCII, whatever the path is.
        assert!(url.is_ascii() && !url.contains(' '), "{url}");
        let part = range(&url, "bytes=90-99");
        assert_eq!(part.status, 206);
        assert_eq!(part.body, (90..100u8).collect::<Vec<_>>());
        assert_eq!(part.header("content-range"), Some("bytes 90-99/100"));
    }

    #[test]
    fn media_server_reads_open_and_suffix_ranges_and_refuses_those_past_the_end() {
        let dir = tempfile::tempdir().unwrap();
        let file = counted(dir.path(), "audio.m4a");
        let server = MediaServer::start().unwrap();
        let url = server.url_for(&file);

        // From a place to the end: what a player asks for first.
        let open = range(&url, "bytes=95-");
        assert_eq!(open.status, 206);
        assert_eq!(open.body, (95..100u8).collect::<Vec<_>>());
        assert_eq!(open.header("content-range"), Some("bytes 95-99/100"));

        // The last bytes: where the `moov` atom of an m4a is.
        let last = range(&url, "bytes=-4");
        assert_eq!(last.status, 206);
        assert_eq!(last.body, (96..100u8).collect::<Vec<_>>());
        assert_eq!(last.header("content-range"), Some("bytes 96-99/100"));

        // An end past the file is the end of the file.
        let long = range(&url, "bytes=98-500");
        assert_eq!(long.status, 206);
        assert_eq!(long.body, vec![98, 99]);
        assert_eq!(long.header("content-range"), Some("bytes 98-99/100"));

        // A suffix longer than the file is the whole of it.
        let all = range(&url, "bytes=-500");
        assert_eq!(all.status, 206);
        assert_eq!(all.body.len(), 100);

        // A start past the end can't be served: the answer says how long the file is.
        for unsatisfiable in ["bytes=100-", "bytes=200-300", "bytes=-0"] {
            let refused = range(&url, unsatisfiable);
            assert_eq!(refused.status, 416, "{unsatisfiable}");
            assert_eq!(refused.header("content-range"), Some("bytes */100"));
            assert!(refused.body.is_empty());
        }

        // What isn't a range of bytes is not one: the whole file.
        for unknown in [
            "bytes=5-2",
            "items=0-5",
            "bytes=a-b",
            "bytes=0-1,5-6",
            "bytes",
        ] {
            let whole = range(&url, unknown);
            assert_eq!((whole.status, whole.body.len()), (200, 100), "{unknown}");
        }
    }

    /// A GET that may get no answer (nobody listens on the port, or the connection is dropped): the status when there was one.
    fn try_status(url: &str) -> Option<u16> {
        let rest = url.strip_prefix("http://")?;
        let (host, path) = rest.split_once('/')?;
        let mut stream = TcpStream::connect(host).ok()?;
        // Only a net under the test: a listener that is gone answers with a reset at once.
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .ok()?;
        write!(
            stream,
            "GET /{path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
        )
        .ok()?;
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).ok()?;
        String::from_utf8_lossy(&raw)
            .lines()
            .next()?
            .split(' ')
            .nth(1)?
            .parse()
            .ok()
    }

    /// The port and what follows it in an address.
    fn port_and_path(url: &str) -> (u16, String) {
        let rest = url.strip_prefix("http://127.0.0.1:").expect("loopback");
        let (port, path) = rest.split_once('/').expect("a path");
        (port.parse().expect("a port"), path.to_owned())
    }

    #[test]
    fn a_listener_that_died_is_replaced_and_new_addresses_come_from_the_new_one() {
        // tiny_http's accept thread ends at its first error and takes the listener with it, and `recv` then waits for ever: one
        // transient error (a player that bailed out of a seek) would end the playback of every recording until the app was
        // restarted. The server makes another listener, with the same token and the same numbers.
        let dir = tempfile::tempdir().unwrap();
        let (one, two) = (
            counted(dir.path(), "one.m4a"),
            counted(dir.path(), "two.m4a"),
        );
        let server = MediaServer::start().unwrap();
        let before = server.url_for(&one);
        assert_eq!(get(&before).status, 200);

        server.fail_accept();
        server.wait_for_generation(1);

        // The same file, the same token and number, on the port of the new listener, which serves it.
        let after = server.url_for(&one);
        let ((port_before, path_before), (port_after, path_after)) =
            (port_and_path(&before), port_and_path(&after));
        assert_ne!(port_before, port_after);
        assert_eq!(path_before, path_after);
        let served = get(&after);
        assert_eq!(served.status, 200);
        assert_eq!(served.body, (0..100u8).collect::<Vec<_>>());
        // A file that is given an address now gets the next number, and is served too.
        let later = server.url_for(&two);
        assert_eq!(port_and_path(&later).0, port_after);
        assert!(later.ends_with("/1"), "{later}");
        assert_eq!(get(&later).status, 200);
        // The address from before the rebuild is dead: nothing answers on the old port any more.
        assert_eq!(try_status(&before), None);
    }

    /// A way to make a server that fails to bind a given number of times after the first, and notes how long it was asked to
    /// wait, without waiting.
    struct Flaky {
        binds: Arc<AtomicUsize>,
        waits: Arc<Mutex<Vec<Duration>>>,
    }

    impl Flaky {
        /// A server whose binds number `from` up to `until` (counted from 0, the first one is the start) fail.
        fn start(from: usize, until: usize) -> (MediaServer, Flaky) {
            let flaky = Flaky {
                binds: Arc::new(AtomicUsize::new(0)),
                waits: Arc::new(Mutex::new(Vec::new())),
            };
            let (binds, waits) = (flaky.binds.clone(), flaky.waits.clone());
            let server = MediaServer::start_with(
                Arc::new(move || {
                    let n = binds.fetch_add(1, Ordering::SeqCst);
                    if (from..until).contains(&n) {
                        Err(io::Error::other("no port to be had"))
                    } else {
                        bind_loopback()
                    }
                }),
                Arc::new(move |wait| waits.lock().unwrap().push(wait)),
            )
            .unwrap();
            (server, flaky)
        }

        fn waits_ms(&self) -> Vec<u128> {
            self.waits
                .lock()
                .unwrap()
                .iter()
                .map(Duration::as_millis)
                .collect()
        }
    }

    #[test]
    fn a_listener_that_cannot_be_made_again_at_once_is_tried_again_after_longer_waits() {
        let dir = tempfile::tempdir().unwrap();
        let file = counted(dir.path(), "one.m4a");
        // The first bind starts the server; the next two (the first two tries of the rebuild) fail; the third works.
        let (server, flaky) = Flaky::start(1, 3);
        let before = server.url_for(&file);

        server.fail_accept();
        server.wait_for_generation(1);

        // The failure itself waited 100 ms, and each failed try twice as long as the one before.
        assert_eq!(flaky.waits_ms(), [100, 200, 400]);
        assert_eq!(flaky.binds.load(Ordering::SeqCst), 4);
        let after = server.url_for(&file);
        assert_eq!(get(&after).status, 200);
        assert_eq!(try_status(&before), None);
    }

    #[test]
    fn the_wait_starts_over_once_a_request_was_taken() {
        let dir = tempfile::tempdir().unwrap();
        let file = counted(dir.path(), "one.m4a");
        let (server, flaky) = Flaky::start(usize::MAX, usize::MAX);

        server.fail_accept();
        server.wait_for_generation(1);
        // Another failure at once, with no request taken between: the wait is the longer one.
        server.fail_accept();
        server.wait_for_generation(2);
        assert_eq!(flaky.waits_ms(), [100, 200]);

        // A request that is taken ends the run: the next failure waits as the first one did.
        assert_eq!(get(&server.url_for(&file)).status, 200);
        server.fail_accept();
        server.wait_for_generation(3);
        assert_eq!(flaky.waits_ms(), [100, 200, 100]);
    }

    #[test]
    fn the_wait_after_a_failure_grows_to_a_limit_and_only_the_first_of_a_run_is_logged() {
        let mut backoff = Backoff::default();
        let ms = |(wait, first): (Duration, bool)| (wait.as_millis(), first);

        assert_eq!(ms(backoff.failed()), (100, true));
        assert_eq!(ms(backoff.failed()), (200, false));
        assert_eq!(ms(backoff.failed()), (400, false));
        for _ in 0..4 {
            backoff.failed();
        }
        // 100 ms doubled 7 times is 12.8 s: it stops at 5 s, however many failures come.
        assert_eq!(ms(backoff.failed()), (5_000, false));
        for _ in 0..200 {
            assert_eq!(backoff.failed().0, Duration::from_secs(5));
        }

        // A request that was taken ends the run: the next failure is logged again, and the wait starts over.
        backoff.recovered();
        assert_eq!(ms(backoff.failed()), (100, true));
    }

    #[test]
    fn a_file_over_32_kib_is_sent_with_its_length_and_never_chunked() {
        // tiny_http sends a body of 32 KiB or more as `Transfer-Encoding: chunked` unless told not to, which leaves a player
        // with no length: it can't tell how long the recording is, and can't seek in it. Every real recording is that big.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("talk.m4a");
        let bytes: Vec<u8> = (0..100 * 1024u32).map(|i| (i % 251) as u8).collect();
        fs::write(&file, &bytes).unwrap();
        let server = MediaServer::start().unwrap();
        let url = server.url_for(&file);

        let whole = get(&url);
        assert_eq!(whole.status, 200);
        assert_eq!(whole.header("content-length"), Some("102400"));
        assert_eq!(whole.header("transfer-encoding"), None);
        assert_eq!(whole.body, bytes);

        let part = range(&url, "bytes=1000-50999");
        assert_eq!(part.status, 206);
        assert_eq!(part.header("content-length"), Some("50000"));
        assert_eq!(
            part.header("content-range"),
            Some("bytes 1000-50999/102400")
        );
        assert_eq!(part.header("transfer-encoding"), None);
        assert_eq!(part.body, bytes[1000..51_000]);

        // The end of the file, which is where an m4a keeps its `moov` atom, and an open range of the whole of it.
        let end = range(&url, "bytes=-40000");
        assert_eq!(end.header("content-length"), Some("40000"));
        assert_eq!(end.header("transfer-encoding"), None);
        assert_eq!(end.body, bytes[bytes.len() - 40_000..]);

        // HEAD says how long the file is (or the range), with no body and no chunks.
        let head = ask(&url, "HEAD", &[]);
        assert_eq!(head.status, 200);
        assert_eq!(head.header("content-length"), Some("102400"));
        assert_eq!(head.header("transfer-encoding"), None);
        assert!(head.body.is_empty());
        let head = ask(&url, "HEAD", &[("Range", "bytes=0-49999")]);
        assert_eq!(head.status, 206);
        assert_eq!(head.header("content-length"), Some("50000"));
        assert_eq!(head.header("transfer-encoding"), None);
        assert!(head.body.is_empty());
    }

    #[test]
    fn media_server_gives_each_path_one_address_and_a_type_by_extension() {
        let dir = tempfile::tempdir().unwrap();
        let server = MediaServer::start().unwrap();
        let types = [
            ("a.m4a", "audio/mp4"),
            ("a.MP3", "audio/mpeg"),
            ("a.wav", "audio/wav"),
            ("a.webm", "video/webm"),
            ("a.mp4", "video/mp4"),
            ("a.m4v", "video/mp4"),
            ("a.mov", "video/quicktime"),
            ("a.mkv", "application/octet-stream"),
            ("a", "application/octet-stream"),
        ];
        for (name, expected) in types {
            let url = server.url_for(&counted(dir.path(), name));
            assert_eq!(get(&url).header("content-type"), Some(expected), "{name}");
            // Asking again is the same address, so that the list of what is served doesn't grow with every look.
            assert_eq!(server.url_for(&dir.path().join(name)), url, "{name}");
        }
    }

    #[test]
    fn media_server_answers_404_for_a_file_that_is_gone_and_for_other_methods() {
        let dir = tempfile::tempdir().unwrap();
        let file = counted(dir.path(), "audio.m4a");
        let server = MediaServer::start().unwrap();
        let url = server.url_for(&file);

        assert_eq!(ask(&url, "POST", &[]).status, 404);
        assert_eq!(ask(&url, "DELETE", &[]).status, 404);
        fs::remove_file(&file).unwrap();
        assert_eq!(get(&url).status, 404);
        assert_eq!(ask(&url, "HEAD", &[]).status, 404);
    }

    #[test]
    fn media_server_listens_on_loopback_with_a_token_of_128_random_bits() {
        let dir = tempfile::tempdir().unwrap();
        let file = counted(dir.path(), "audio.m4a");
        let (one, two) = (MediaServer::start().unwrap(), MediaServer::start().unwrap());

        let (a, b) = (one.url_for(&file), two.url_for(&file));
        let shape = |url: &str| {
            let rest = url.strip_prefix("http://127.0.0.1:").expect("loopback");
            let (port, path) = rest.split_once('/').unwrap();
            let (token, n) = path.split_once('/').unwrap();
            assert!(port.parse::<u16>().is_ok(), "{url}");
            assert!(n.parse::<usize>().is_ok(), "{url}");
            token.to_owned()
        };
        let (token_a, token_b) = (shape(&a), shape(&b));
        // 128 bits as hex.
        assert_eq!(token_a.len(), 32);
        assert!(token_a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(token_a, token_b);
        assert_ne!(a, b);
        // One server's token opens nothing on the other.
        let path_a = a.splitn(4, '/').nth(3).unwrap();
        let port_b = b["http://127.0.0.1:".len()..].split('/').next().unwrap();
        assert_eq!(
            get(&format!("http://127.0.0.1:{port_b}/{path_a}")).status,
            404
        );
    }

    #[test]
    fn media_server_keeps_serving_while_a_big_file_is_being_read() {
        // A player seeking in a long recording asks for the end of the file while the start is still coming: one slow
        // answer must not hold up the others.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("big.m4a");
        fs::write(&file, vec![7u8; 8 * 1024 * 1024]).unwrap();
        let small = counted(dir.path(), "small.m4a");
        let server = MediaServer::start().unwrap();
        let (big_url, small_url) = (server.url_for(&file), server.url_for(&small));

        // A client that asks for all of the big file and reads none of it.
        let host = big_url.strip_prefix("http://").unwrap();
        let (addr, path) = host.split_once('/').unwrap();
        let mut stalled = TcpStream::connect(addr).unwrap();
        write!(
            stalled,
            "GET /{path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
        )
        .unwrap();

        let part = range(&small_url, "bytes=0-9");
        assert_eq!((part.status, part.body.len()), (206, 10));
        drop(stalled);
    }
}
