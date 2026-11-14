//! Runs the `loah-mcp` binary the way a client does: JSON-RPC lines on stdin, replies on stdout.

use std::io::Write;
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

/// The two variables that name the library folder: the one of Loah, and the one Dictate had, which is still read.
const NEW_VARIABLE: &str = "LOAH_MEETINGS_DIR";
const OLD_VARIABLE: &str = "DICTATE_MEETINGS_DIR";

/// Runs the server on `input` (one message per line) against the library in `directory`, until stdin closes.
fn run(directory: &std::path::Path, input: &str) -> Output {
    run_with(NEW_VARIABLE, directory, input)
}

/// `run`, with `directory` named by `variable` only: the other one is removed from what the server is given, so that a
/// variable of the person who runs the tests never decides which library is read.
fn run_with(variable: &str, directory: &std::path::Path, input: &str) -> Output {
    let other = if variable == NEW_VARIABLE {
        OLD_VARIABLE
    } else {
        NEW_VARIABLE
    };
    let mut child = Command::new(env!("CARGO_BIN_EXE_loah-mcp"))
        .env_remove(other)
        .env(variable, directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start loah-mcp");
    child
        .stdin
        .take()
        .expect("a stdin pipe")
        .write_all(input.as_bytes())
        .expect("write the requests");
    child.wait_with_output().expect("wait for loah-mcp")
}

fn replies(output: &Output) -> Vec<Value> {
    let text = String::from_utf8(output.stdout.clone()).expect("replies are UTF-8");
    text.lines()
        .map(|line| serde_json::from_str(line).expect("a reply is one JSON line"))
        .collect()
}

#[test]
fn lists_the_five_tools_in_one_line() {
    let library = tempfile::tempdir().expect("a library folder");
    let request = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"});
    let output = run(library.path(), &format!("{request}\n"));
    assert!(output.status.success());
    // Stdin was a pipe, not a terminal: no help text.
    assert!(output.stderr.is_empty());
    let replies = replies(&output);
    assert_eq!(replies.len(), 1);
    let names: Vec<&str> = replies[0]["result"]["tools"]
        .as_array()
        .expect("a tools array")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert_eq!(
        names,
        [
            "list_meetings",
            "get_meeting",
            "get_transcript",
            "search_meetings",
            "list_action_items"
        ]
    );
}

/// A library folder with one recording, and the requests that read its transcript.
fn library_and_requests() -> (tempfile::TempDir, String) {
    let library = tempfile::tempdir().expect("a library folder");
    let folder = library.path().join("2026-10-19 22-20-58");
    std::fs::create_dir(&folder).expect("a recording folder");
    std::fs::write(
        folder.join("meta.json"),
        json!({"title": "Họp kế hoạch", "source": "Quay màn hình", "created": 1_792_423_258.0, "duration": 373.0}).to_string(),
    )
    .expect("meta.json");
    std::fs::write(folder.join("transcript.txt"), "[00:00] A: xin chào").expect("transcript");
    let initialize = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}});
    let call = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "get_transcript", "arguments": {"id": "Họp kế hoạch"}}});
    let initialized = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    (library, format!("{initialize}\n{initialized}\n{call}\n"))
}

#[test]
fn reads_the_library_named_by_the_environment() {
    let (library, input) = library_and_requests();
    let output = run(library.path(), &input);
    let replies = replies(&output);
    assert_eq!(replies.len(), 2, "a notification gets no reply");
    let info = &replies[0]["result"]["serverInfo"];
    assert_eq!(info["name"], "loah");
    assert_eq!(info["version"], env!("CARGO_PKG_VERSION"));
    assert!(replies[0]["result"]["instructions"]
        .as_str()
        .is_some_and(|text| text.starts_with("Loah is an app for macOS, Windows and Linux")));
    assert_eq!(replies[1]["result"]["isError"], false);
    assert_eq!(
        replies[1]["result"]["content"][0]["text"],
        "Transcript of \"Họp kế hoạch\" (id: 2026-10-19 22-20-58) — paragraphs 1–1 of 1.\n\n[00:00] A: xin chào"
    );
}

#[test]
fn the_old_meetings_variable_still_works() {
    // An MCP client that was set up for Dictate has DICTATE_MEETINGS_DIR in its configuration and has never heard of the
    // new variable: the server must read that library and not an empty one.
    let (library, input) = library_and_requests();
    let output = run_with(OLD_VARIABLE, library.path(), &input);
    let replies = replies(&output);
    assert_eq!(replies.len(), 2, "a notification gets no reply");
    assert_eq!(replies[1]["result"]["isError"], false);
    assert_eq!(
        replies[1]["result"]["content"][0]["text"],
        "Transcript of \"Họp kế hoạch\" (id: 2026-10-19 22-20-58) — paragraphs 1–1 of 1.\n\n[00:00] A: xin chào"
    );
}
