//! Checks `dictate-mcp` against the macOS (Swift) server: the same requests must get the same replies.
//!
//! `tests/golden/expected*.jsonl` hold the Swift server's replies to `tests/golden/requests*.jsonl`, made by
//! `scripts/golden.sh`. Here the Rust binary reads the same requests and its replies are compared with them
//! line by line as parsed JSON, ignoring only the server's version number (each build has its own).
//!
//! One reply is deliberately not the Swift server's: a transcript saved with Windows line ends (CRLF) is read
//! paragraph by paragraph here, while the Swift server takes it for one long paragraph with "\r\n" inside. The
//! fixture keeps one such recording (`2026-10-16 23-59-59`); no golden request prints its transcript (a
//! `list_meetings` query reads it), and `crlf_transcript_is_read_paragraph_by_paragraph` pins the Rust behaviour.
//!
//! The two tests that replay the golden requests run on Unix only: the dates in the replies are local times, and
//! the dates in `expected*.jsonl` are those of `Asia/Ho_Chi_Minh`. chrono follows the `TZ` variable on Unix only,
//! so on Windows the server would show the machine's own zone. The comparison self-test and the CRLF test run on
//! every OS.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

const ZONE: &str = "Asia/Ho_Chi_Minh";

/// Where each server states its version: the legacy `initialize` reply and every modern reply.
const LEGACY_INFO: &[&str] = &["result", "serverInfo"];
const MODERN_INFO: &[&str] = &["result", "_meta", "io.modelcontextprotocol/serverInfo"];

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden")
}

#[cfg(unix)]
fn read(file: &str) -> String {
    let path = golden_dir().join(file);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

/// The replies (stdout) of `dictate-mcp` to the requests in the file `input`, reading the library in `library`.
fn serve_file(library: &Path, input: &Path) -> String {
    let input = File::open(input).expect("open the requests");
    let output = Command::new(env!("CARGO_BIN_EXE_dictate-mcp"))
        .env("DICTATE_MEETINGS_DIR", library)
        .env("TZ", ZONE)
        .stdin(Stdio::from(input))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run dictate-mcp");
    assert!(
        output.status.success(),
        "dictate-mcp failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Stdin was a file, not a terminal: no help text.
    assert!(output.stderr.is_empty(), "dictate-mcp wrote to stderr");
    String::from_utf8(output.stdout).expect("replies are UTF-8")
}

/// The replies of `dictate-mcp` to the requests in the golden file `requests`.
#[cfg(unix)]
fn serve(library: &Path, requests: &str) -> String {
    serve_file(library, &golden_dir().join(requests))
}

fn parse(text: &str) -> Vec<Value> {
    text.lines()
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("not a JSON line ({error}): {line}"))
        })
        .collect()
}

/// `reply` without the server's version number. A batch is a list of replies.
fn without_server_version(mut reply: Value) -> Value {
    if let Value::Array(replies) = reply {
        return Value::Array(replies.into_iter().map(without_server_version).collect());
    }
    for path in [LEGACY_INFO, MODERN_INFO] {
        let mut place = Some(&mut reply);
        for key in path {
            place = place.and_then(|value| value.get_mut(*key));
        }
        if let Some(info) = place.and_then(Value::as_object_mut) {
            info.remove("version");
        }
    }
    reply
}

/// Compares the replies line by line; the error lists the lines that differ.
fn compare(expected: &str, actual: &str) -> Result<(), String> {
    let (expected, actual) = (parse(expected), parse(actual));
    let mut problems = Vec::new();
    if expected.len() != actual.len() {
        problems.push(format!(
            "expected {} replies, got {}",
            expected.len(),
            actual.len()
        ));
    }
    for (index, (want, got)) in expected.into_iter().zip(actual).enumerate() {
        let (want, got) = (without_server_version(want), without_server_version(got));
        if want != got {
            problems.push(format!(
                "line {} differs\n  expected: {want}\n  actual:   {got}",
                index + 1
            ));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        let count = problems.len();
        let shown: Vec<String> = problems.into_iter().take(5).collect();
        Err(format!(
            "{count} difference(s), first {}:\n{}",
            shown.len(),
            shown.join("\n")
        ))
    }
}

#[cfg(unix)]
#[test]
fn replies_match_the_swift_server_for_the_fixture_library() {
    let actual = serve(&golden_dir().join("fixture"), "requests.jsonl");
    if let Err(problems) = compare(&read("expected.jsonl"), &actual) {
        panic!("{problems}");
    }
}

#[cfg(unix)]
#[test]
fn replies_match_the_swift_server_for_an_empty_library() {
    let library = tempfile::tempdir().expect("an empty library folder");
    let actual = serve(library.path(), "requests-empty.jsonl");
    if let Err(problems) = compare(&read("expected-empty.jsonl"), &actual) {
        panic!("{problems}");
    }
}

#[test]
fn the_comparison_ignores_the_server_version_and_nothing_else() {
    let legacy = |name: &str, version: &str| {
        format!(
            r#"{{"jsonrpc":"2.0","id":1,"result":{{"serverInfo":{{"name":"{name}","version":"{version}"}},"instructions":"v"}}}}"#
        )
    };
    let modern = |version: &str, text: &str| {
        format!(
            r#"{{"jsonrpc":"2.0","id":2,"result":{{"content":[{{"type":"text","text":"{text}"}}],"_meta":{{"io.modelcontextprotocol/serverInfo":{{"name":"dictate","version":"{version}"}}}}}}}}"#
        )
    };
    // Other versions: the legacy and the modern place, alone and inside a batch.
    assert_eq!(
        compare(&legacy("dictate", "dev"), &legacy("dictate", "1.2.3")),
        Ok(())
    );
    assert_eq!(compare(&modern("dev", "x"), &modern("1.2.3", "x")), Ok(()));
    assert_eq!(
        compare(
            &format!("[{}]", modern("dev", "x")),
            &format!("[{}]", modern("1.2.3", "x"))
        ),
        Ok(())
    );
    // Anything else counts: the server's name, a reply's text, the number of replies.
    assert!(compare(&legacy("dictate", "dev"), &legacy("other", "dev")).is_err());
    assert!(compare(&modern("dev", "x"), &modern("dev", "y")).is_err());
    assert!(compare(&modern("dev", "x"), "").is_err());
    // A "version" elsewhere in a reply is data, not the server's.
    let protocol = |version: &str| {
        format!(
            r#"{{"jsonrpc":"2.0","id":3,"result":{{"protocolVersion":"{version}","version":"{version}"}}}}"#
        )
    };
    assert!(compare(&protocol("2025-06-18"), &protocol("2025-03-26")).is_err());
}

#[test]
fn crlf_transcript_is_read_paragraph_by_paragraph() {
    let library = golden_dir().join("fixture");
    let id = "2026-10-16 23-59-59";
    let file = std::fs::read(library.join(id).join("transcript.txt")).expect("the CRLF transcript");
    assert!(
        file.windows(2).any(|pair| pair == b"\r\n"),
        "the fixture transcript lost its CRLF line ends"
    );

    let scratch = tempfile::tempdir().expect("a folder for the request");
    let request = scratch.path().join("request.jsonl");
    let call = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "get_transcript", "arguments": {"id": id}}});
    std::fs::write(&request, format!("{call}\n")).expect("write the request");

    let replies = parse(&serve_file(&library, &request));
    assert_eq!(
        replies[0]["result"]["content"][0]["text"],
        "Transcript of \"Gọi khách hàng về đường truyền\" (id: 2026-10-16 23-59-59) — paragraphs 1–3 of 3.\n\n\
         [00:00] Khách: Băng thông của máy chủ đang bị nghẽn từ sáng nay.\n\n\
         [00:40] Kỹ thuật: Chúng tôi sẽ nâng dung lượng đường truyền ngay trong ngày.\n\n\
         [01:30] Khách: Cảm ơn, mong sớm có kết quả."
    );
}
