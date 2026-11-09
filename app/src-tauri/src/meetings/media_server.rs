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

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

/// How many bytes of randomness the token has.
const TOKEN_BYTES: usize = 16;

pub struct MediaServer {
    port: u16,
    token: String,
    /// The files that have an address: the address of the one at place `n` ends in `/n`.
    paths: Arc<Mutex<Vec<PathBuf>>>,
    server: Arc<Server>,
    stopped: Arc<AtomicBool>,
}

impl MediaServer {
    /// Starts the server on a free port of 127.0.0.1, on a thread of its own.
    pub fn start() -> io::Result<MediaServer> {
        let server = Arc::new(Server::http("127.0.0.1:0").map_err(io::Error::other)?);
        let port = server
            .server_addr()
            .to_ip()
            .map(|address| address.port())
            .ok_or_else(|| io::Error::other("the media server has no port"))?;
        let token = new_token()?;
        let paths = Arc::new(Mutex::new(Vec::new()));
        let stopped = Arc::new(AtomicBool::new(false));

        let listening = (
            server.clone(),
            token.clone(),
            paths.clone(),
            stopped.clone(),
        );
        thread::Builder::new()
            .name("media-server".into())
            .spawn(move || serve(&listening.0, &listening.1, &listening.2, &listening.3))?;
        Ok(MediaServer {
            port,
            token,
            paths,
            server,
            stopped,
        })
    }

    /// The address of `path`, which the server serves from now on: the same one each time for the same path.
    pub fn url_for(&self, path: &Path) -> String {
        let mut paths = self.paths.lock().unwrap_or_else(PoisonError::into_inner);
        let n = match paths.iter().position(|known| known == path) {
            Some(n) => n,
            None => {
                paths.push(path.to_path_buf());
                paths.len() - 1
            }
        };
        format!("http://127.0.0.1:{}/{}/{n}", self.port, self.token)
    }
}

impl Drop for MediaServer {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.server.unblock();
    }
}

/// A random token of `TOKEN_BYTES` bytes from the system, as hex.
fn new_token() -> io::Result<String> {
    let mut bytes = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes)
        .map_err(|e| io::Error::other(format!("the system gave no random bytes: {e}")))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Takes the requests one by one until the server is stopped. Each is answered on a thread of its own: a player that is
/// seeking in a long recording asks for the end of the file while the start is still being sent, and a client that is
/// slow to read must not hold up the others.
fn serve(server: &Server, token: &str, paths: &Arc<Mutex<Vec<PathBuf>>>, stopped: &AtomicBool) {
    loop {
        match server.recv() {
            Ok(request) => {
                let (token, paths) = (token.to_owned(), paths.clone());
                let spawned = thread::Builder::new()
                    .name("media-answer".into())
                    .spawn(move || answer(request, &token, &paths));
                if let Err(e) = spawned {
                    log::warn!("The media server could not start a thread: {e}");
                }
            }
            Err(_) if stopped.load(Ordering::SeqCst) => return,
            Err(e) => {
                log::warn!("The media server could not take a request: {e}");
                thread::sleep(Duration::from_millis(100));
            }
        }
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
fn registered(url: &str, token: &str, paths: &Mutex<Vec<PathBuf>>) -> Option<PathBuf> {
    let path = url.split(['?', '#']).next()?.strip_prefix('/')?;
    let (given, n) = path.split_once('/')?;
    if !same(given, token) {
        return None;
    }
    let n: usize = n.parse().ok()?;
    paths
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(n)
        .cloned()
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

fn answer(request: Request, token: &str, paths: &Mutex<Vec<PathBuf>>) {
    let found = matches!(request.method(), Method::Get | Method::Head)
        .then(|| registered(request.url(), token, paths))
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
        Wanted::Whole => request.respond(Response::new(
            StatusCode(200),
            headers(Vec::new()),
            file,
            usize::try_from(len).ok(),
            None,
        )),
        Wanted::Part(first, last) => {
            let size = last - first + 1;
            if file.seek(SeekFrom::Start(first)).is_err() {
                let _ = request.respond(Response::empty(StatusCode(500)));
                return;
            }
            request.respond(Response::new(
                StatusCode(206),
                headers(vec![header(
                    "Content-Range",
                    &format!("bytes {first}-{last}/{len}"),
                )]),
                file.take(size),
                usize::try_from(size).ok(),
                None,
            ))
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
