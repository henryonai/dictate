# Cross-platform Dictate — Plan 1: Core library and MCP server

**Goal:** Start the new cross-platform repo with a Rust port of `DictateCore` (meetings library, transcript/summary parsing, search, speaker names, Whisper/segment helpers) and a `dictate-mcp` binary whose output matches the Swift one byte for byte, building and testing on macOS, Windows and Linux.

**Architecture:** A Cargo workspace with two crates: `dictate-core` (library, no UI, no OS-specific code) and `dictate-mcp` (stdio binary). The Tauri app in later plans depends on `dictate-core`. The Swift sources in `henryonai/dictate-mac` are the behavior reference; every Swift file named below is read before porting it.

**Tech Stack:** Rust stable (2021 edition), `serde`/`serde_json`, `chrono` (local-time folder names), `unicode-normalization` (diacritic folding), `fancy-regex` (lookbehind in speaker renaming), `dirs` (Documents folder), GitHub Actions.

**Spec:** `docs/specs/2026-11-08-cross-platform-dictate-design.md`

**Swift reference:** the Swift app, `henryonai/dictate-mac` (below: `dictate-mac/`). **This repo:** the new cross-platform repo; paths below without a prefix are relative to its root. Creating the GitHub repo `henryonai/dictate` and pushing is **not** part of this plan; it happens once the plan is complete.

### The plan series

This spec is split into plans that each produce working, tested software:

1. **Core library and MCP server** (this plan).
2. Tauri app shell: tray, settings, keychain, AI providers, localization (en/vi), local Whisper + transcription APIs.
3. Dictation: hotkey, mic capture, live bubble, cleanup, paste per OS, chip/rewrite, review card, hold-to-talk.
4. Meetings: import file/link, pipeline (mix → transcript + diarization → summary), library UI, Q&A, speaker rename, export.
5. Recording: screen/window/audio capture per OS, HUD, then packaging, CI release artifacts, README.

Each later plan is written when the previous one is done.

## Global Constraints

- License: MIT (copy `dictate-mac/LICENSE`, keep the copyright line).
- Library folder: OS Documents folder + `Dictate Meetings`; `DICTATE_MEETINGS_DIR` overrides it (with `~` expanded).
- Recording folder names `yyyy-MM-dd HH-mm-ss` (optionally followed by ` (2)`…), parsed in **local** time.
- Files: `meta.json`, `transcript.txt`, `live_transcript.txt`, `summary.md`, `tasks.json`, `log.txt` — names exactly as in `dictate-mac/Sources/DictateCore/Meetings/MeetingLibrary.swift`.
- Screen-recording `source` value: `"Quay màn hình"`.
- Summary section names in both languages, exactly as in `dictate-mac/Sources/DictateCore/Meetings/SummaryDoc.swift:38-43`.
- MCP: same tool names, argument schemas, descriptions, instructions text, protocol versions (`2026-07-28` modern; `2025-11-25`, `2025-06-18`, `2025-03-26`, `2024-11-05` legacy) and **identical output text** to the Swift `dictate-mcp`.
- Commits: Conventional Commits (`feat:`, `fix:`, `test:`, `ci:`, `chore:`, `docs:`). No `Co-Authored-By` trailer and no tool attributions in messages.
- No OS-specific code in `dictate-core`; it must compile and pass tests on `macos-latest`, `windows-latest`, `ubuntu-latest`.
- No `unwrap()`/`expect()`/byte-index slicing of user text in library code; text is sliced by `char`.

## Review Focus

1. **Snippets around Vietnamese text** — `TextSearch::snippet` on a long transcript with multi-byte characters must not panic and must return the same window as Swift. Test: Task 1 `snippet_multibyte_window`.
2. **Files edited on Windows (CRLF line endings)** — transcripts and summaries with `\r\n` parse the same as `\n`. Tests: Task 2 `transcript_crlf`, `summary_crlf`.
3. **Broken or missing `meta.json`** (invalid JSON, wrong types, empty file) — the recording still lists, using folder name as title and folder date as created. Test: Task 4 `broken_meta_falls_back`.
4. **No library folder yet** (fresh install) — `meetings()` returns empty, MCP tools say there are no recordings rather than erroring. Test: Task 4 `missing_library_dir_is_empty`; Task 6 golden case `list_empty_library`.
5. **Garbage on the MCP stdin** (non-JSON line, unknown method, batch array, notification without id) — server replies with the right JSON-RPC error or nothing, and keeps serving the next line. Test: Task 5 `malformed_lines_keep_serving`.

---

### Task 1: Workspace, toolchain, CI, text utilities (Timestamp, TextSearch)

**Files:**
- Create: `Cargo.toml` (workspace, members `crates/*`), `rust-toolchain.toml` (`channel = "stable"`, components `rustfmt`, `clippy`), `.gitignore` (`target/`), `LICENSE`, `README.md` (one paragraph: what this is, "work in progress", link to the spec), `docs/specs/2026-11-08-cross-platform-dictate-design.md` (copy of the spec), `docs/plans/` (copy of this plan)
- Create: `.github/workflows/ci.yml`
- Create: `crates/dictate-core/Cargo.toml`, `crates/dictate-core/src/lib.rs`, `src/text/mod.rs`, `src/text/timestamp.rs`, `src/text/search.rs`
- Test: inline `#[cfg(test)]` modules in each file (this convention holds for every task)
- Reference: `dictate-mac/Sources/DictateCore/Text/Timestamp.swift`, `dictate-mac/Sources/DictateCore/Text/TextSearch.swift`

**Interfaces:**
- Produces:
  - `dictate_core::text::timestamp::{format(seconds: f64) -> String, parse(text: &str) -> Option<f64>}`
  - `dictate_core::text::search::TextSearch { pub words: Vec<String> }` with `TextSearch::new(query: &str) -> Self`, `is_empty(&self) -> bool`, `matches(&self, text: &str) -> bool`, `snippet(&self, text: &str, length: usize) -> String` (callers pass 280 where Swift uses the default), and `pub fn fold(s: &str) -> String`

- [ ] **Step 1: Install the toolchain.** If `cargo --version` fails, install rustup (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y`, then `source "$HOME/.cargo/env"`). Expected: `cargo --version` prints a version.
- [ ] **Step 2: `git init` in the repo root, create the workspace files listed above.** `cargo build` succeeds on an empty `lib.rs`.
- [ ] **Step 3: Write failing tests.**

```rust
// timestamp.rs
#[test] fn formats() { assert_eq!(format(65.0), "01:05"); assert_eq!(format(3725.9), "1:02:05"); assert_eq!(format(0.0), "00:00"); }
#[test] fn parses() {
    assert_eq!(parse("[12:34]"), Some(754.0)); assert_eq!(parse("1:02:03"), Some(3723.0));
    assert_eq!(parse("754"), Some(754.0)); assert_eq!(parse("12:x"), None); assert_eq!(parse("1:2:3:4"), None);
}
// search.rs
#[test] fn fold_vietnamese() { assert_eq!(fold("Họp Đội đẹp"), "hop doi dep"); }
#[test] fn matches_all_words() { let s = TextSearch::new("hop doi"); assert!(s.matches("Họp đội tuần")); assert!(!s.matches("Họp tuần")); assert!(TextSearch::new("  ").matches("anything")); }
#[test] fn snippet_short_text_unchanged() { assert_eq!(TextSearch::new("x").snippet("a\nb", 280), "a b"); }
#[test] fn snippet_multibyte_window() {
    let text = format!("{}refactor{}", "Chào mọi người ".repeat(40), " đồng ý ".repeat(40));
    let out = TextSearch::new("refactor").snippet(&text, 280);
    assert!(out.starts_with('…') && out.ends_with('…'));
    assert!(out.contains("refactor"));
    assert_eq!(out.trim_matches('…').chars().count(), 280);
}
```

- [ ] **Step 4: Run `cargo test -p dictate-core`.** Expected: FAIL (functions missing).
- [ ] **Step 5: Implement.** `fold`: NFD-decompose, drop combining marks (`\u{0300}`–`\u{036F}` and other `Mn`), lowercase, then `đ`/`Đ` → `d`. `snippet`: port Swift exactly, counting and slicing by `char` (NFC-normalize input first so one Vietnamese letter is one char); centre on the first word only when folded and original have equal char counts.
- [ ] **Step 6: Run `cargo test -p dictate-core`.** Expected: PASS.
- [ ] **Step 7: Write `ci.yml`.** Matrix over `macos-latest`, `windows-latest`, `ubuntu-latest`; steps: checkout, `dtolnay/rust-toolchain@stable` with rustfmt+clippy, `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all`. Locally run those three commands. Expected: all pass.
- [ ] **Step 8: Commit** `feat: start the cross-platform workspace with timestamp and search helpers`.

### Task 2: Transcript lines, summary document, speaker names

**Files:**
- Create: `crates/dictate-core/src/meetings/mod.rs`, `src/meetings/transcript.rs`, `src/meetings/summary.rs`, `src/meetings/speakers.rs`
- Reference: `dictate-mac/Sources/DictateCore/Meetings/{TranscriptLine,SummaryDoc,SpeakerNames}.swift`, `dictate-mac/Tests/DictateCoreTests/{ParsingTests,SpeakerNamesTests,Fixture}.swift`

**Interfaces:**
- Consumes: `timestamp::parse`
- Produces:
  - `TranscriptLine { id: usize, start: f64, stamp: String, speaker: Option<String>, text: String }`, `TranscriptLine::parse(transcript: &str) -> Vec<TranscriptLine>`, `formatted(&self) -> String`
  - `SummaryDoc { title: Option<String>, sections: Vec<Section> }`, `Section { heading, body }`, `ActionItem { owner: Option<String>, text: String }`, `Mark { seconds: u32, label: String, text: String }`; `SummaryDoc::parse(md: &str) -> SummaryDoc`, `section(&self, names: &[&str]) -> Option<&Section>`, `decisions()`, `questions() -> Vec<String>`, `action_items() -> Vec<ActionItem>`, `timeline() -> Vec<Mark>`; consts `SUMMARY_NAMES`, `KEY_POINT_NAMES`, `DECISION_NAMES`, `ACTION_NAMES`, `QUESTION_NAMES`, `TIMELINE_NAMES`
  - `speakers::{Speaker { name, lines: usize, sample: TranscriptLine }, speakers_in(transcript: &str) -> Vec<Speaker>, MAX_LENGTH: usize = 24, clean(name: &str) -> Option<String>, rename_transcript(transcript: &str, names: &HashMap<String,String>) -> String, rename_summary(summary: &str, names: &HashMap<String,String>) -> String}`
  - A test-only fixture module `crates/dictate-core/src/test_fixture.rs` (`#[cfg(test)]`) holding `SUMMARY` and `TRANSCRIPT` copied verbatim from `dictate-mac/Tests/DictateCoreTests/Fixture.swift`; Task 4 extends it to write the 3-recording library.

- [ ] **Step 1: Port every test in `ParsingTests.swift` (the transcript/summary ones) and `SpeakerNamesTests.swift` to Rust**, one `#[test]` per Swift test, same names in snake_case, same input strings and expected values. Add:

```rust
#[test] fn transcript_crlf() { assert_eq!(TranscriptLine::parse(&TRANSCRIPT.replace('\n', "\r\n")).len(), 3); }
#[test] fn summary_crlf() {
    let d = SummaryDoc::parse(&SUMMARY.replace('\n', "\r\n"));
    assert_eq!(d.title.as_deref(), Some("Họp kế hoạch refactor"));
    assert_eq!(d.action_items().len(), 2); assert_eq!(d.timeline()[1].seconds, 65);
}
```

- [ ] **Step 2: Run `cargo test -p dictate-core`.** Expected: FAIL.
- [ ] **Step 3: Implement the three files by porting the Swift.** Normalize `\r\n` to `\n` at the top of each parse. Speaker renaming: use `fancy-regex` for the `(?<!word)` / `(?!word)` boundaries with `word = [\p{L}\p{M}\p{N}_]`, keep Swift's generic-vs-named ordering.
- [ ] **Step 4: Run `cargo test -p dictate-core`.** Expected: PASS.
- [ ] **Step 5: Commit** `feat: parse transcripts, summaries and speaker names`.

### Task 3: Whisper text and segment replies

**Files:**
- Create: `src/text/whisper.rs`, `src/text/segments.rs`
- Reference: `dictate-mac/Sources/DictateCore/Text/{WhisperText,SegmentsReply}.swift`, `dictate-mac/Tests/DictateCoreTests/{WhisperTextTests,SegmentsReplyTests}.swift`

**Interfaces:**
- Produces:
  - `whisper::{clean(text: &str) -> String, dropping_padding_lines<T>(items: Vec<T>, duration: f64, start: impl Fn(&T)->f64, text: impl Fn(&T)->&str) -> Vec<T>` (argument set as the Swift signature), `Timed { start: f64, end: f64, text: String, speaker: Option<u32> }`, `Turn { start: f64, end: f64, speaker: u32 }`, `splitting_by_speaker(lines: &[(Timed, Vec<Timed>)], turns: &[Turn]) -> Vec<Timed>}`
  - `segments::{Segment { start: f64, speaker: Option<String>, text: String }` (serde `Serialize`/`Deserialize`), `parse(content: &str, duration: Option<f64>) -> Option<(Vec<Segment>, bool)>`, `dropping_loops(segments: Vec<Segment>) -> Vec<Segment>`, `collapsing_repeats(text: &str) -> String}`

- [ ] **Step 1: Port every test from `WhisperTextTests.swift` and `SegmentsReplyTests.swift`**, same inputs and expected values.
- [ ] **Step 2: Run `cargo test -p dictate-core`.** Expected: FAIL.
- [ ] **Step 3: Implement by porting.** `segments::parse` keeps Swift's two paths: full JSON object with `segments`, else the brace-depth scanner that recovers complete objects from a truncated reply (`complete = false`).
- [ ] **Step 4: Run `cargo test -p dictate-core`.** Expected: PASS.
- [ ] **Step 5: Commit** `feat: clean Whisper output and parse segment replies`.

### Task 4: Meetings library

**Files:**
- Create: `src/meetings/library.rs`; extend `src/test_fixture.rs`
- Reference: `dictate-mac/Sources/DictateCore/Meetings/MeetingLibrary.swift`, `dictate-mac/Tests/DictateCoreTests/{Fixture,ParsingTests}.swift`

**Interfaces:**
- Consumes: `search::fold`
- Produces:
  - `files` module consts `META = "meta.json"`, `TRANSCRIPT = "transcript.txt"`, `LIVE_TRANSCRIPT = "live_transcript.txt"`, `SUMMARY = "summary.md"`, `TASKS = "tasks.json"`, `LOG = "log.txt"`; `SCREEN_SOURCE = "Quay màn hình"`
  - `enum Kind { Screen, File, Link }`
  - `MeetingRecord { id: String, folder: PathBuf, title: String, source: String, created: chrono::DateTime<Utc>, duration: Option<f64> }` with `from_folder(path: &Path) -> Option<Self>`, `kind()`, `read(&self, file: &str) -> Option<String>`, `summary() -> Option<String>`, `transcript() -> Option<(String, bool /*is_live*/)>`, `done_tasks() -> BTreeSet<usize>`
  - `MeetingLibrary { directory: PathBuf }` with `default_directory() -> PathBuf`, `new(directory: PathBuf)`, `meetings() -> Vec<MeetingRecord>` (newest first), `find(id_or_title: &str) -> Option<MeetingRecord>`
  - `test_fixture::Fixture::new() -> Fixture` (temp dir with the three recordings from `Fixture.swift`, removed on drop) and `Fixture::library()`

- [ ] **Step 1: Port library tests from `ParsingTests.swift` and write:**

```rust
#[test] fn lists_newest_first() { let f = Fixture::new(); let ids: Vec<_> = f.library().meetings().into_iter().map(|m| m.id).collect();
    assert_eq!(ids, ["2026-10-19 22-20-58", "2026-10-18 11-50-17", "2026-10-17 09-00-00"]); }
#[test] fn kinds_and_live() { let f = Fixture::new(); let m = f.library().meetings();
    assert_eq!(m[0].kind(), Kind::Screen); assert_eq!(m[1].kind(), Kind::Link);
    assert_eq!(m[2].transcript().map(|t| t.1), Some(true)); assert_eq!(m[0].done_tasks(), BTreeSet::from([0])); }
#[test] fn find_by_title_folded() { let f = Fixture::new(); assert_eq!(f.library().find("hop ke hoach").map(|m| m.id).as_deref(), Some("2026-10-19 22-20-58"));
    assert!(f.library().find("\"Video về Swift\"").is_some()); }
#[test] fn broken_meta_falls_back() { /* write "{not json" to meta.json of a folder "2026-01-02 03-04-05" */ /* title == folder name, created == that local time */ }
#[test] fn missing_library_dir_is_empty() { assert!(MeetingLibrary::new("/nonexistent/dictate".into()).meetings().is_empty()); }
```

- [ ] **Step 2: Run `cargo test -p dictate-core`.** Expected: FAIL.
- [ ] **Step 3: Implement.** `default_directory`: `DICTATE_MEETINGS_DIR` (expand leading `~` with `dirs::home_dir`) else `dirs::document_dir()` (fallback `home_dir()/Documents`) joined with `Dictate Meetings`. Folder-name date: first 19 chars, `%Y-%m-%d %H-%M-%S`, `chrono::Local`. `find` trims `" \"'/"` as Swift does.
- [ ] **Step 4: Run `cargo test -p dictate-core`.** Expected: PASS.
- [ ] **Step 5: Commit** `feat: read the meetings library`.

### Task 5: MCP server protocol

**Files:**
- Create: `src/mcp/mod.rs`, `src/mcp/server.rs`
- Reference: `dictate-mac/Sources/DictateCore/MCP/MCPServer.swift`, protocol tests in `dictate-mac/Tests/DictateCoreTests/MCPServerTests.swift`

**Interfaces:**
- Produces:
  - `Tool { name: String, title: String, description: String, input_schema: serde_json::Value, run: Box<dyn Fn(&serde_json::Map<String, Value>) -> Result<String, ToolError> + Send + Sync> }`
  - `ToolError { message: String }`
  - `McpServer::new(name: &str, version: &str, instructions: &str, tools: Vec<Tool>) -> Self`, `handle_line(&self, line: &str) -> Option<String>`, `run(&self)` (reads stdin line by line, writes each reply + `\n` to stdout, flushes)
  - consts `MODERN_VERSIONS`, `LEGACY_VERSIONS` with the values in Global Constraints

- [ ] **Step 1: Port the protocol-level tests from `MCPServerTests.swift`** (initialize/version negotiation, `server/discover`, `tools/list`, `ping`, unknown method, tool error → `isError`), comparing parsed JSON values, not raw strings. Add:

```rust
#[test] fn malformed_lines_keep_serving() {
    let s = test_server();
    let r: Value = serde_json::from_str(&s.handle_line("{nope").unwrap()).unwrap();
    assert_eq!(r["error"]["code"], -32700);
    assert!(s.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
    let b: Value = serde_json::from_str(&s.handle_line(r#"[{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","id":2,"method":"nope"}]"#).unwrap()).unwrap();
    assert_eq!(b.as_array().unwrap().len(), 2); assert_eq!(b[1]["error"]["code"], -32601);
    assert!(s.handle_line(r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#).is_some());
}
```

- [ ] **Step 2: Run `cargo test -p dictate-core`.** Expected: FAIL.
- [ ] **Step 3: Implement by porting `MCPServer.swift`**, keeping its error codes, result shapes and version-negotiation rules.
- [ ] **Step 4: Run `cargo test -p dictate-core`.** Expected: PASS.
- [ ] **Step 5: Commit** `feat: speak MCP over stdio`.

### Task 6: Meeting tools and the `dictate-mcp` binary

**Files:**
- Create: `src/mcp/tools.rs`; `crates/dictate-mcp/Cargo.toml`, `crates/dictate-mcp/src/main.rs`
- Reference: `dictate-mac/Sources/DictateCore/MCP/MeetingTools.swift`, `dictate-mac/Sources/DictateMCP/main.swift`, tool tests in `dictate-mac/Tests/DictateCoreTests/MCPServerTests.swift`

**Interfaces:**
- Consumes: `McpServer`, `Tool`, `ToolError`, `MeetingLibrary`, `SummaryDoc`, `TranscriptLine`, `TextSearch`, `timestamp`
- Produces: `tools::INSTRUCTIONS: &str` (verbatim from Swift), `tools::all(library: MeetingLibrary) -> Vec<Tool>` with `list_meetings`, `get_meeting`, `get_transcript`, `search_meetings`, `list_action_items`; binary `dictate-mcp`

- [ ] **Step 1: Port every tool-level test from `MCPServerTests.swift`** against `Fixture`, asserting the exact output strings the Swift tests assert.
- [ ] **Step 2: Run `cargo test -p dictate-core`.** Expected: FAIL.
- [ ] **Step 3: Implement `tools.rs` by porting `MeetingTools.swift`**: descriptions, schemas, argument validation and error messages verbatim; dates in `from`/`to` as local `YYYY-MM-DD`.
- [ ] **Step 4: Implement `main.rs`.** If stdin is a terminal (`std::io::IsTerminal`), print the Swift help text to stderr with the executable path substituted. Version from `env!("CARGO_PKG_VERSION")`. Then `McpServer::new("dictate", version, INSTRUCTIONS, tools::all(MeetingLibrary::new(MeetingLibrary::default_directory()))).run()`.
- [ ] **Step 5: Run `cargo test --all` and `echo '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | cargo run -q -p dictate-mcp`.** Expected: tests PASS; one JSON line listing the five tools.
- [ ] **Step 6: Commit** `feat: add dictate-mcp with the meeting tools`.

### Task 7: Golden parity check against the Swift `dictate-mcp`

**Files:**
- Create: `tests/golden/requests.jsonl` (one JSON-RPC request per line), `tests/golden/fixture/` (the three-recording library from `Fixture.swift` as real files, plus a fourth recording with CRLF transcript and a long Vietnamese transcript for snippets), `tests/golden/expected.jsonl`, `scripts/golden.sh`, `crates/dictate-mcp/tests/golden.rs`

**Interfaces:**
- Consumes: the `dictate-mcp` binary

- [ ] **Step 1: Write `requests.jsonl`** covering: `initialize` (modern and each legacy version), `tools/list`, every tool with no args, with every argument (including `from`/`to`, `limit`, `start`/`end`, `offset`, `owner`, `status` values), an unknown recording id, a search with no matches, a search with diacritics (`"hop"`), plus case `list_empty_library` (run with `DICTATE_MEETINGS_DIR` pointing to an empty dir).
- [ ] **Step 2: Write `scripts/golden.sh`**: builds the Swift `dictate-mcp` in `dictate-mac/` (`swift build -c release --product dictate-mcp`), pipes `requests.jsonl` into it with `DICTATE_MEETINGS_DIR=tests/golden/fixture` and `TZ=Asia/Ho_Chi_Minh`, writes `expected.jsonl`. Run it once and commit the output (the Swift binary is only needed to regenerate).
- [ ] **Step 3: Write `golden.rs`**: runs the Rust binary (`env!("CARGO_BIN_EXE_dictate-mcp")`) with the same env and input; compares line by line as parsed JSON, except the `serverInfo.version` field. Run `cargo test -p dictate-mcp --test golden`. Expected: FAIL only where output differs; fix `dictate-core` until PASS.
- [ ] **Step 4: Run the full CI command set** (`cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --all`). Expected: PASS.
- [ ] **Step 5: Commit** `test: check dictate-mcp output against the Swift version`.
