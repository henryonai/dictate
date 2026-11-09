# Dictate

Dictate is a cross-platform (macOS, Windows, Linux) take on the Dictate macOS app: dictation, meeting recording, transcripts and AI summaries. This repository starts with a Rust port of the shared core library and the `dictate-mcp` server. It is a work in progress; see the [design spec](docs/specs/2026-11-08-cross-platform-dictate-design.md) for the plan. The app so far is a tray icon with a Settings window, and dictation with local Whisper: [Run it](app/README.md) to try it.

## Dictation on each system

- **macOS:** dictation needs the Microphone and Accessibility permissions (Accessibility lets Dictate find the text field and paste into it; without it the text comes out in a card to edit and copy). Settings → General → Permissions shows both and takes you to the right pane of System Settings. The app is not signed, so after a rebuild or an update Accessibility may need granting again: remove Dictate from the list in System Settings → Privacy & Security → Accessibility and add it again. With `pnpm tauri dev` the permissions go to the terminal app you started it from.
- **Windows:** an app that runs as administrator can't be pasted into by a normal app: the text is copied and you press Ctrl+V.
- **Linux:** X11 works. On Wayland, install `wtype` (wlroots desktops such as Sway, and Hyprland) or `ydotool` so that Dictate can paste, and where the desktop has no global-shortcut portal, bind a key to `dictate --toggle`.

[app/README.md](app/README.md) has the details.
