# Dictate

Dictate is a cross-platform (macOS, Windows, Linux) take on the Dictate macOS app: dictation, meeting recording, transcripts and AI summaries. This repository starts with a Rust port of the shared core library and the `dictate-mcp` server. It is a work in progress; see the [design spec](docs/specs/2026-11-08-cross-platform-dictate-design.md) for the plan. The app so far is a tray icon with a Settings window, dictation with local Whisper, and a Meetings window that turns audio and video files and links into transcripts and summaries: [Run it](app/README.md) to try it.

## Dictation on each system

- **macOS:** dictation needs the Microphone and Accessibility permissions (Accessibility lets Dictate find the text field and paste into it; without it the text comes out in a card to edit and copy). Settings → General → Permissions shows both and takes you to the right pane of System Settings. The app is not signed, so after a rebuild or an update Accessibility may need granting again: remove Dictate from the list in System Settings → Privacy & Security → Accessibility and add it again. With `pnpm tauri dev` the permissions go to the terminal app you started it from.
- **Windows:** an app that runs as administrator can't be pasted into by a normal app: the text is copied and you press Ctrl+V.
- **Linux:** X11 works. On Wayland, install `wtype` (wlroots desktops such as Sway, and Hyprland) or `ydotool` so that Dictate can paste, and where the desktop has no global-shortcut portal, bind a key to `dictate --toggle`.

[app/README.md](app/README.md) has the details.

## Meetings

The tray menu's **Meetings…** opens the library: add a recording and Dictate makes a transcript, tells the speakers apart (Speaker 1, 2…) and, when an AI is set up, writes a summary with decisions, action items to tick and chapters that jump to that moment of the recording. Ask a question about a recording, rename the speakers, and export the Markdown.

- **Importing.** Choose audio or video files, drag them onto the window, or paste a link (YouTube or any site yt-dlp supports) with Ctrl+V / ⌘V anywhere in the window. **The files you add are not copied or moved:** the recording keeps pointing at the original, so keep it where it is. If it is moved or deleted the transcript and summary stay, and playback says the file is gone.
- **yt-dlp and ffmpeg.** A link needs yt-dlp (it downloads the audio), and ffmpeg reads the formats that Dictate can't open by itself. Settings → Speech shows which of them Dictate found, with the command to install a missing one:

  | | yt-dlp (and ffmpeg) | ffmpeg only |
  |---|---|---|
  | macOS | `brew install yt-dlp ffmpeg` | `brew install ffmpeg` |
  | Windows | `winget install yt-dlp.yt-dlp Gyan.FFmpeg` | `winget install Gyan.FFmpeg` |
  | Linux | `pipx install yt-dlp && sudo apt install ffmpeg` | `sudo apt install ffmpeg` |

- **Playback on Linux** needs the GStreamer libav plugin: `sudo apt install gstreamer1.0-libav gstreamer1.0-plugins-good` (the `.deb` recommends both). Without it the window says which package to install.
- **The library folder** is `Dictate Meetings` in your Documents folder, one folder per recording, named by its start time, with `transcript.txt`, `summary.md`, `tasks.json`, `log.txt` and `meta.json` in it. It is the same library that `dictate-mcp` reads. Set `DICTATE_MEETINGS_DIR` (a leading `~` means your home folder) before starting the app to use another folder, for example to try it out without touching your real library; `dictate-mcp` reads the same variable. A deleted recording goes to the Trash (Recycle Bin).
- **Audio leaves your computer only when you choose the Transcription API** as the file engine (Settings → Speech) and have set one up. The default is local Whisper. The AI that writes summaries and answers questions gets the text of the transcript, never the audio.
- **Third-party models.** The first time speakers are separated, Dictate downloads two models (about 46 MB) and runs them with [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) (Apache-2.0): the pyannote segmentation 3.0 model (MIT license, © CNRS) and the NVIDIA NeMo TitaNet-small speaker model (CC-BY-4.0, © NVIDIA).
