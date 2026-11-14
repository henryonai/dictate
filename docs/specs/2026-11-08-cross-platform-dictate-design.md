# Cross-platform Dictate — design

Status: draft for approval · 2026-11-08

## Intent

Dictate is a macOS-only Swift app (repo `henryonai/dictate-mac`). Friends on Windows and Linux have asked to try it, and the project is meant to be public, so strangers must be able to download, install and use it without help.

The goal is **one app, one codebase, running on macOS, Windows and Linux**, with the same two halves Dictate has today:

1. **Dictation** — press a global shortcut, talk, see the words appear near the cursor, AI cleans them up, the result is written into whatever text field had focus. Optional rewrite of what was just said (shorter, email, bullets, translation…).
2. **Meetings** — record the screen or audio, or import a file or a link; get a transcript with speakers, a summary, decisions and action items; ask questions about a recording; expose the library to AI assistants through an MCP server.

The Swift Mac app keeps working for now and is **retired once the new app is good enough on Mac**. From then on there is one codebase to maintain. That single-codebase outcome is the main reason for the project's shape: the maintainer is one person.

## Goals

- Feature parity with today's Swift Dictate at the **first public release** (no staged feature releases; internal build order is the builder's call, but nothing ships until everything below works).
- Runs on macOS (Apple silicon and Intel), Windows 10/11 (x64), and Linux (x64; at least Ubuntu/Debian-family, X11 and Wayland — see Constraints).
- Works **free and offline out of the box** using local Whisper; an API key is optional and improves speed/accuracy.
- Installable by a non-developer from a GitHub Releases page.
- Interface in English and Vietnamese.
- Existing Mac users can switch from the Swift app without losing their meetings library or breaking their MCP setup.

## Non-goals

- Accounts, sign-in, licensing, payments, telemetry, analytics, or a project-run backend server. Dictate talks only to services the user configures (as today).
- A marketing/landing website.
- Mobile platforms.
- Porting the Swift code line by line. The Swift code is the reference for behavior, not for structure.

## Anti-goals (would count as failure even if it technically works)

- Copying voice-to-text's heavyweight pieces: its multi-layer "Clean Architecture"/DDD folder scheme, OAuth, Paddle licensing, its backend. We borrow its **choice of tools**, not its code or structure.
- An app that only works with a paid API key, or that silently sends audio to the cloud when the user chose local.
- Dictation that copies to the clipboard and leaves the user to paste on platforms where writing into the field is achievable. (Falling back to clipboard only where the OS forbids it is fine — see below.)
- Two codebases kept alive indefinitely.

## Decided by the user

- One cross-platform app; retire the Swift Mac app later.
- Both dictation and meetings, full parity, all at the first release.
- Speech recognition: **local Whisper by default**, **optional API key** for cloud transcription.
- A **fresh repository** (working name `henryonai/dictate`); `dictate-mac` gets archived after the Mac version is retired.
- **Reliability over feature count in the first release.** Features that need reaching back into another app's text (replace the dictated text in place, "Delete what was written") are **not** in the first release on any platform; the ⋯ rewrite writes the new version as a fresh paste instead. *(To confirm.)*
- **No live captions during meeting recording in the first release.** Meetings show only the final transcript after recording; `live_transcript.txt` is still read if present (old recordings) but not written.

## Decisions (open to change)

- **Stack: Tauri 2** — Rust core plus a web UI — because it ships on all three platforms from one codebase and stays small. This is the same tool family as https://github.com/777genius/voice-to-text (Tauri 2 + Rust + Vue, Apache-2.0), which is useful prior art for audio capture (`cpal`), tray, global hotkey and packaging on all three OSes.
- **Frontend framework:** builder's choice; Vue or Svelte both fine. Keep it simple and flat.
- **Local Whisper via whisper.cpp** (e.g. through the `whisper-rs` bindings), with GPU acceleration where cheaply available (Metal on macOS; CPU elsewhere is acceptable, Vulkan/CUDA optional). Models downloaded once from Hugging Face on first use, as today.
- **Live text while talking** comes from re-running Whisper on short rolling chunks of audio. It will lag more than Apple Speech; when the user stops, the final transcript (and AI cleanup) replaces the live text. Live text quality is "good enough to see it's hearing you", not perfect.
- **Keep the MCP server and the meetings library format identical** (see "Compatibility") so Mac users and existing MCP client configs keep working.
- **License: MIT**, same as today.
- Spec lives in `dictate-mac` for now and is copied into the new repo as its first commit.
- **Parity scope = README.md, minus the exclusions under "Decided by the user".** That includes: the review card when no field is focused (with numbered style shortcuts, undo and custom requests), hold-to-talk, "press again to skip AI", chip dismissal on typing or app switch, terminal handling, saving dictation audio/text to the app's data folder; for meetings, the full-screen / single-window / audio-only picker with quality presets, the recording HUD with pause and levels, excluding the app's own windows from video where the OS allows, speaker rename with a sample line, Markdown export, grouping by day.
- **Mac identity during transition:** the new app has its own bundle identifier so testers can run it next to the Swift app. Both write to the same library folder; one folder per recording means no clashes. The MCP binary moves to a new path; the README gives the one-line config change.

## Feature list (parity target)

Reference behavior is in this repo's `README.md` and `Sources/`. In short:

**Dictation**
- Global shortcut (default `Ctrl+Alt+Space` on Windows/Linux, `⌃⌥Space` on macOS), configurable.
- A floating bubble with live text, positioned near the text cursor when the OS lets us find it, otherwise near the mouse or bottom-centre of the screen.
- Language picker in the panel, switchable while talking; English is the default.
- On stop: optional cloud re-transcription (if an API is configured and chosen), then AI cleanup (drop ums, fix misheard words and punctuation) with the user's default style, then write into the focused field and restore the user's clipboard afterwards.
- A chip next to the result with "⋯" rewrite options (shorter, email, bullets, translate, …); the rewrite is pasted as new text, not swapped in place (first release).
- If the AI fails, the raw transcript is written instead.

**Meetings**
- Record screen + system audio + microphone, or audio only.
- Import a local audio/video file; import a link via `yt-dlp` (+ `ffmpeg`), which remain optional user-installed tools, detected at runtime with a clear message if missing.
- Pipeline per recording: mix audio tracks to `audio.m4a` → transcript (local Whisper, or a transcription API) with speaker separation → summary from the AI provider. Each step shows progress and time left; failures say why, keep finished steps, and offer a fix (open settings, retry).
- Library view: list, transcript with clickable timestamps that seek the media, summary, decisions, action items with tick boxes, Q&A about the recording.
- Speaker separation: today uses SpeakerKit (pyannote). Builder picks a cross-platform equivalent (e.g. pyannote/ONNX segmentation + embedding models run via ONNX Runtime, or sherpa-onnx diarization). Must work offline after a one-time model download.

**Providers and settings**
- AI: OpenRouter with `deepseek/deepseek-v4.1-flash` by default (an OpenAI-compatible endpoint, with its base URL and model already filled in), or Anthropic, or any other OpenAI-compatible endpoint (OpenAI, CLIProxyAPI, Ollama…), each with a "Check" button.
- Transcription API: OpenRouter (Gemini via audio input), OpenAI, Groq, or a custom `/audio/transcriptions` endpoint; separate model choice for dictation vs files.
- API keys stored in the OS credential store (macOS Keychain, Windows Credential Manager, Linux Secret Service), never in plain files. A Rust crate like `keyring` covers all three.
- Launch at login, interface language (follows the OS by default).

**MCP server**
- A separate small command-line binary (today `dictate-mcp`) speaking MCP over stdio with the same tools: `list_meetings`, `get_meeting`, `get_transcript`, `search_meetings`, `list_action_items`, same arguments and same output text. Shipped with the app on all platforms, with a documented path for MCP client config.

## Compatibility (must not break)

The meetings library format is a contract shared by the app and the MCP server, and with existing Mac users' data:

- Library folder: `~/Documents/Dictate Meetings` (use the OS's Documents folder on Windows/Linux), overridable with `DICTATE_MEETINGS_DIR`.
- One folder per recording named `yyyy-MM-dd HH-mm-ss` (plus ` (2)`… on collisions).
- Files inside: `meta.json` (title, source, media, created as Unix seconds, duration), `transcript.txt` (`[mm:ss] Speaker: text` paragraphs), `live_transcript.txt`, `summary.md` (format per `SummaryDoc.swift`), `tasks.json` (array of indices of ticked action items), `log.txt`, plus the media/`audio.m4a`.
- Screen recordings currently store `source = "Quay màn hình"`; keep reading that value. New app may keep writing it for compatibility (builder's call, documented).
- Summaries in either Vietnamese or English section headings must parse (see `SummaryDoc.swift`).
- `DictateCore` tests (`Tests/DictateCoreTests`) describe parsing, search (case- and diacritic-insensitive), speaker names, segment replies and MCP behavior; port them as the acceptance tests for the Rust equivalents.

Settings and API keys do **not** need to migrate from the Swift app; users re-enter them.

## Constraints and known hard parts

- **Writing text into the focused field.**
  - macOS: Accessibility permission, then paste via synthesized ⌘V (as today) with clipboard restore.
  - Windows: synthesized Ctrl+V via `SendInput` with clipboard restore.
  - Linux X11: synthesized Ctrl+V (xdotool-style / XTest).
  - Linux Wayland: compositors block synthetic input to other apps. Try available paths (e.g. `wtype`/`ydotool` if installed, or the XDG RemoteDesktop portal); if none works, **fall back to copying to the clipboard and showing a "Copied — press Ctrl+V" notice**. This fallback is acceptable only on Wayland.
- **Global shortcut on Wayland**: use the XDG GlobalShortcuts portal where supported; otherwise document binding a desktop shortcut to a command (e.g. `dictate --toggle`). The app must expose such a CLI toggle on all platforms.
- **Cursor position for the bubble**: macOS via Accessibility; Windows via UI Automation/caret APIs where available; Linux best-effort. Fallback position is fine.
- **System audio capture**: macOS ScreenCaptureKit (needs Screen Recording permission); Windows WASAPI loopback; Linux PipeWire/PulseAudio monitor source. Screen video capture: macOS ScreenCaptureKit, Windows Graphics Capture, Linux PipeWire screencast portal. Bundling ffmpeg for muxing/encoding is acceptable if licensing is respected (LGPL build).
- **Permissions UX**: on macOS, the app must guide the user through Microphone, Accessibility and Screen Recording, as the Swift app does today.
- **Packaging**: GitHub Releases with `.dmg` (macOS), `.msi` or NSIS `.exe` (Windows), `.AppImage` and `.deb` (Linux). Unsigned builds are acceptable for the first release, with README instructions for the macOS Gatekeeper and Windows SmartScreen prompts (as the current README does for macOS). Include `.sha256` files.
- **Updates**: Tauri's updater with signed update manifests is desirable but may come after the first release.
- **Model sizes**: Whisper models are large; show size before download and let the user pick (recommend one by default based on the machine).

## Repository and process

- New repo `henryonai/dictate`, MIT, with README (English + Vietnamese), CONTRIBUTING, CODE_OF_CONDUCT, SECURITY carried over and rewritten for the new stack.
- Conventional Commits with release-please, as in `dictate-mac`. No `Co-Authored-By` trailers or tool attributions in commit messages.
- CI on GitHub Actions builds and tests on all three OSes for every PR, and produces release artifacts on release.
- Tests: Rust unit tests for the library/parsing/search/MCP (ported from `DictateCoreTests`), plus per-platform smoke checks where practical.

## Left to the builder

- Internal build order (suggested: core library + MCP → dictation on one OS → dictation on all three → meetings pipeline → recording/capture per OS → packaging/CI).
- Frontend framework and UI component choices; visual design should follow the current app's look where sensible.
- Exact crates for audio, hotkeys, input synthesis, diarization, and screen capture.
- Default Whisper model choice per platform.
- How the Rust core is split into crates/modules (keep it flat and readable; no layered DDD scheme).
