# Dictate app

The desktop app, for macOS, Windows and Linux: a tray icon, a Settings window and a Meetings window. It is built with [Tauri 2](https://tauri.app): a Rust shell in `src-tauri/` and a Svelte 5 + TypeScript interface in `ui/`.

Dictation works: press the global shortcut (Ctrl+Alt+Space by default, ⌃⌥Space on the Mac; it can be changed in Settings), talk, and press it again (or hold it, in hold-to-talk mode): the text is written into the text field you were in, and a small chip by it lets you write it again in another style. With no text field to write into, or with the setting that writes into fields off, a card opens to edit and copy the text. Local Whisper (a model downloaded in Settings) turns the speech into text and shows it live while you talk; the transcription API and the AI edit are optional. Settings (language, AI provider, transcription service), API keys in the system's key store, the Whisper model downloads and the connection checks work. `dictate --toggle` (also when the app isn't running yet) does what pressing the shortcut once does, and so does the first item of the tray menu. The audio and the text of each dictation are kept in `Dictate/Dictations` in the system's local data folder (the tray menu opens it) when "Save recordings + transcripts" is on; a dictation that isn't saved leaves nothing behind. Audio goes to the transcription API only when that is the chosen engine. On a Wayland desktop the shortcut comes from the desktop's GlobalShortcuts portal (KDE Plasma, GNOME 48 and later, Hyprland); where there is none (Sway and other wlroots desktops), bind a shortcut in the desktop's own settings to run `dictate --toggle`.

The Meetings window (tray menu → **Meetings…**) imports audio and video files and links, and turns each into a transcript with the speakers told apart and, when an AI is set up, a summary: see [Meetings](#meetings). It also records the screen, a window or just the sound of a call, and the recording goes through the same steps: see [Recording](#recording). The installers (a `.dmg` for each kind of Mac, an `.exe` for Windows and a `.deb`) are built and checked automatically by CI, and each carries the MCP server and an LGPL ffmpeg: see [Installers](#installers). What is still to do is the manual install matrix, running each installer on a clean machine of its system and trying dictation, import, recording and the MCP server by hand. The recording tests use scripted sources, and CI records the screen of the Windows runner for three seconds.

```
app/
  src-tauri/   the Rust side: tray, windows, commands (crate `dictate-app`, the binary is `dictate`)
  ui/          the Settings window and the page of the floating dictation windows (`panel.html`): Svelte, Vite, Vitest
  package.json the Tauri command line tool; pnpm-workspace.yaml makes `ui/` part of the workspace
```

## Prerequisites

Every system needs:

- **Rust 1.85 or newer** with `rustup` (the repository picks the stable toolchain in `rust-toolchain.toml`).
- **Node.js 22**.
- **pnpm 12 or Corepack.** `app/package.json` names the exact pnpm version. Either install pnpm 12, or use [Corepack](https://nodejs.org/api/corepack.html) (`corepack enable`), which fetches that version by itself. Corepack should be 0.34.5 or newer: if `pnpm` fails to run, update it with `npm install -g corepack@latest`.
- **CMake and libclang.** Whisper (whisper.cpp) is built from source with the app.
- **A network connection for the first build.** The speaker separation library (sherpa-onnx, with onnxruntime inside) is not built from source: a prebuilt one is downloaded. Its speech synthesis has espeak-ng in it, which is GPL-3.0 and which Dictate has no use for, so on Windows and Linux you fetch the build of the release that has no synthesis yourself and point `SHERPA_ONNX_LIB_DIR` at it ([Windows](#on-windows-the-sherpa-onnx-library), [Linux](#on-linux-the-sherpa-onnx-library-is-a-shared-one-without-text-to-speech)); on Linux the build stops and says how if the variable is not set. On macOS the build script of the `sherpa-onnx-sys` crate downloads the static library (about 20 MB) into `target/sherpa-onnx-prebuilt` the first time: a static link keeps out what the program does not use. To build offline there, download `sherpa-onnx-v1.13.8-osx-arm64-static-lib.tar.bz2` (`osx-x64` on Intel) from the [v1.13.8 release](https://github.com/k2-fsa/sherpa-onnx/releases/tag/v1.13.8), put it in a folder and set `SHERPA_ONNX_ARCHIVE_DIR` to it, or set `SHERPA_ONNX_LIB_DIR` to the `lib` folder of the unpacked archive.

On top of that:

- **macOS:** Xcode 16 or newer, or the Command Line Tools (`xcode-select --install`) with the macOS 15 SDK or newer (`xcrun --sdk macosx --show-sdk-version` says which one you have), and CMake (`brew install cmake`). The SDK has to be that new to build, because the `screencapturekit` crate is built with its `macos_15_0` feature; the app itself runs on macOS 13 or later (on a system older than 15 a call that only macOS 15 has says so at run time). The screen and the system's audio of a recording come from ScreenCaptureKit, through a Swift bridge that the `screencapturekit` crate builds with `swift build` (Xcode and the Command Line Tools have it; the first build takes a minute or so longer for it).
- **Windows 10 or 11:** the [Microsoft C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with the "Desktop development with C++" workload, CMake and LLVM (for libclang), and the WebView2 runtime, which Windows 11 has already. CMake and LLVM can come from winget, one command each:

  ```sh
  winget install Kitware.CMake
  winget install LLVM.LLVM
  ```

  Open a new terminal afterwards, so that it finds the new tools. If the build can't find libclang, set `LIBCLANG_PATH` to the `bin` folder of the LLVM install.
- **Linux:** the web view, tray, audio (ALSA, for the microphone), PipeWire (the screen of a recording, see below) and build libraries. On Debian and Ubuntu:

  ```sh
  sudo apt-get install -y build-essential cmake libclang-dev libasound2-dev \
    libpipewire-0.3-dev libspa-0.2-dev \
    libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev
  ```

  Other distributions have the same libraries under their own names (`pipewire-devel` on Fedora; the [Tauri prerequisites](https://tauri.app/start/prerequisites/#linux) list the others). The tray icon needs a desktop that shows app indicators (GNOME needs the AppIndicator extension).

  The screen of a recording comes from the desktop's ScreenCast portal (`xdg-desktop-portal` and the backend of your desktop: GNOME, KDE Plasma and Hyprland have one, and `xdg-desktop-portal-wlr` serves Sway and other wlroots desktops) as a PipeWire stream, so a desktop without one, such as a bare window manager, cannot record its screen. The desktop's own dialog asks what to share. Dictate asks the portal to remember the choice, and a desktop that can (it is up to the desktop; GNOME and KDE Plasma can) records the same kind of thing later without the dialog; one that cannot asks every time. The app links `libpipewire-0.3`, so a machine that runs it needs the library itself (`libpipewire-0.3-0` on Debian and Ubuntu, which a desktop with PipeWire has).

## Install

From this folder:

```sh
pnpm install
```

This installs the Tauri command line tool and the dependencies of `ui/`. The first Rust build, which comes with the next step, takes a few minutes because of whisper.cpp.

## Run it

```sh
pnpm tauri dev
```

This starts the Vite dev server for the interface and the app, which reloads when the interface changes. The first time the app starts, with no settings saved yet, the Settings window opens. After that the app lives in the tray (menu bar on macOS, with no Dock icon): click the icon for the menu, **Settings** to open the window and **Quit** to leave. Closing the window doesn't quit the app.

Dictation has two small floating windows, made at startup and hidden: the bubble, which sits next to the text field and never takes the focus (the live text, and the result chip that stays by what was written, with its ⋯ menu to write it again in another style), and the card, which does take the focus (the pill that shows while dictating with no field to write into, and the review card to edit the text, with ⌘1–5 / Ctrl+1–5 for the styles). The controller shows them as a dictation goes on. In a debug build, `DICTATE_PANEL_DEMO=1 target/debug/dictate` cycles through example states to look at them (`DICTATE_PANEL_DEMO_ANCHOR=x,y` puts them next to a point of the screen, `DICTATE_PANEL_DEMO_STEP_MS` and `DICTATE_PANEL_DEMO_HOLD=n` set how long a state stays and which one to stay on, and `DICTATE_PANEL_DEMO_MONITOR=1` also logs that a key or a click was heard in another app, which is what hides the chip). A debug build can also listen to a WAV file instead of the microphone, to try dictation without talking: `DICTATE_DEBUG_MIC_WAV=speech.wav target/debug/dictate`; each recording then hears the file from its start, in real time. `RUST_LOG=dictate=debug` logs what the panels show (how many characters, never the text) and what the shortcut says. A debug build made with `cargo build` loads the interface from the Vite dev server, so start `pnpm --dir ui dev` first, or add `--features tauri/custom-protocol` to build the interface in.

Settings are kept in `Dictate/settings.json` in the system's config folder, the Whisper models in `Dictate/whisper-models` in its local data folder (on macOS both are in `~/Library/Application Support`; on Windows the settings are in `%APPDATA%` and the models in `%LOCALAPPDATA%`, so that they don't roam with the profile), and API keys in the system's key store (Keychain, Credential Manager, Secret Service).

## Dictation on each system

What the system lets dictation do differs, and what to do about it:

**macOS.** Dictation needs two permissions. The *Microphone* is asked for by the system the first time you dictate. *Accessibility* is what lets Dictate find the text field you are in and paste the text there; without it the text still comes out, in the card, where you can edit and copy it. Settings → General → Permissions shows both, with **Grant access…** (the system's own prompt) and **Open System Settings** (the Privacy & Security pane), and refreshes by itself while you turn them on. Settings also opens by itself when the app starts while the microphone has not been asked about yet, and once when Accessibility is off and the setting that writes into text fields is on; it does not open at every login for a person who skipped Accessibility or who only uses the card.

- The app is not signed (ad-hoc at most), so macOS takes every rebuilt or updated copy for a different app: Accessibility may need granting again after an update. Open System Settings → Privacy & Security → Accessibility, remove Dictate from the list with the minus button, and add it again.
- With `pnpm tauri dev`, or `target/debug/dictate` started from a terminal, the permissions go to the terminal app (Terminal, iTerm, the editor's terminal) and not to Dictate: that is the app to turn on in the Accessibility and Microphone lists. The bundle `Dictate.app` has the permissions of its own.
- `src-tauri/Info.plist` holds the reason the system shows with the microphone prompt, and `src-tauri/Entitlements.plist` the right to use the microphone that a signed build (with the hardened runtime, which notarisation needs) must declare; `tauri.conf.json` points the bundler to both.

**Windows.** Dictation pastes with Ctrl+V into the app that had the focus. An app that runs as administrator has more rights than Dictate, and Windows blocks input from a normal app into it: the text is copied and the card says so, then press Ctrl+V yourself. If the microphone only records silence, check Settings → Privacy & security → Microphone and let desktop apps use it.

**Linux.** On X11 everything works. On Wayland no app may press keys in another one, so the paste needs a helper: install `wtype` (wlroots desktops such as Sway, and Hyprland) or `ydotool` (with its `ydotoold` daemon running); without either, the text is copied and the card says to press Ctrl+V. The shortcut comes from the desktop's GlobalShortcuts portal (KDE Plasma, GNOME 48 and later, Hyprland). When the desktop has none (Sway and other wlroots desktops), bind a key in the desktop's own settings to `dictate --toggle`, which does what the shortcut does.

## Meetings

The tray menu's **Meetings…** opens the library of recordings. Add one and Dictate transcribes it (local Whisper by default), separates the speakers (Speaker 1, 2…), and, when an AI is set up in Settings, writes a summary with key points, decisions, action items that you can tick and chapters that jump to that moment of the recording. You can ask a question about a recording, rename the speakers (in the transcript and the summary, without AI, or with a fresh summary), see the log of a job, export the Markdown and move a recording to the Trash. A job can be cancelled and tried again; a recording that the app closed on shows "Not finished processing" the next time.

**Importing files and links.** Use the **Add file or link** menu (a file picker, or a link), drag files onto the window, or paste a link with ⌘V / Ctrl+V anywhere in the window. Audio and video of the usual formats work (mp4, mov, mkv, webm, m4a, mp3, wav, flac, ogg and more). **The files you add are not copied or moved**: the recording keeps the path of the original, so keep the file where it is. If it goes away the transcript and the summary stay, and the window says that the original file was moved or deleted.

**yt-dlp and ffmpeg.** Links are downloaded with yt-dlp, and ffmpeg reads the formats that Dictate can't decode itself. ffmpeg comes with the installers, as `dictate-ffmpeg` next to the program (a build of FFmpeg under the LGPL: see [Installers](#installers)); yt-dlp does not, and the user installs it. They are looked for in this order. For ffmpeg: the program that the `DICTATE_FFMPEG` environment variable names (a path, or a name on the search path; it is how someone replaces the ffmpeg that ships, which the LGPL allows, and a value that is empty or that names no program that can run is ignored), then `dictate-ffmpeg` next to the executable, then `ffmpeg` on the search path. For yt-dlp: next to the executable, then the search path, which is your `PATH` with the usual package-manager folders added (Homebrew, `/usr/local/bin`, `~/.local/bin` where pipx puts yt-dlp); yt-dlp is told where ffmpeg is with `--ffmpeg-location`. A build in the source tree (`pnpm tauri dev`) has no `dictate-ffmpeg` beside it, so it uses `DICTATE_FFMPEG` or the ffmpeg of your system. Settings → Speech → **Tools for links** shows which were found, with their versions, and the command for the ones that are missing, which is the command of the system you run on:

| | yt-dlp (also installs ffmpeg) | ffmpeg only |
|---|---|---|
| macOS | `brew install yt-dlp ffmpeg` | `brew install ffmpeg` |
| Windows | `winget install yt-dlp.yt-dlp Gyan.FFmpeg` | `winget install Gyan.FFmpeg` |
| Linux | `pipx install yt-dlp && sudo apt install ffmpeg` | `sudo apt install ffmpeg` |

The error card of a recording that needs a missing tool has the same command with a **Copy install command** button.

**Playback on Linux.** The window plays recordings with the system's web view, which has no AAC decoder without GStreamer's libav plugin: install `gstreamer1.0-libav` (and `gstreamer1.0-plugins-good`; the `.deb` that `pnpm tauri build` makes lists both as recommended packages, so `apt` installs them with it):

```sh
sudo apt install gstreamer1.0-libav gstreamer1.0-plugins-good
```

Without it the window says "Install gstreamer1.0-libav to play recordings." The transcript and the summary work all the same.

**The library folder.** `Dictate Meetings` in the Documents folder of the system (Settings → General → **Open meetings folder**, and the same item of the tray menu, open it). Each recording is a folder named by its start time (`yyyy-MM-dd HH-mm-ss`, with ` (2)` if two share a second) that holds `meta.json`, `transcript.txt` (`[mm:ss] Speaker: text` paragraphs), `summary.md`, `tasks.json` (the action items that are ticked), `log.txt` and `audio.m4a`. This is the library that `dictate-mcp` serves to other apps. `DICTATE_MEETINGS_DIR` replaces the folder (a leading `~` is your home folder), for the app and for `dictate-mcp`: set it before starting either, for instance to try the app out without touching your own recordings:

```sh
DICTATE_MEETINGS_DIR=~/dictate-try pnpm tauri dev
```

**Audio and privacy.** Audio is sent to a service only when **File engine** (Settings → Speech) is **Transcription API** and one is set up. With local Whisper, the default, nothing of the audio leaves the computer. The AI that writes summaries and answers questions is sent text only (the transcript), never audio.

**Speaker models and credits.** The first time speakers are separated, the app downloads two models (about 46 MB, once) into `Dictate/diarization-models` in the system's local data folder, and checks them by SHA-256. They run through [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) (Apache-2.0), which the app links:

- **pyannote segmentation 3.0**, MIT license, © CNRS ([model](https://huggingface.co/pyannote/segmentation-3.0)). It finds who talks when.
- **NVIDIA NeMo TitaNet-small**, [CC-BY-4.0](https://creativecommons.org/licenses/by/4.0/), © NVIDIA ([the converted model](https://github.com/k2-fsa/sherpa-onnx/releases/tag/speaker-recongition-models)). It tells voices apart.

## Recording

The tray menu's **Record & summarize…** (and the **Record** button of the Meetings window) opens a picker; with the Meetings window never opened, the item makes the window and shows the picker in it. A recording that you stop becomes a recording of the library and is processed like an imported file: transcript, speakers and, when an AI is set up, a summary. While one goes on, the same tray item reads **Stop recording & summarize**, and on macOS the menu bar shows a dot and the time (`● 00:12`, and `⏸ 00:12` while paused).

**What to record.** The picker has three modes:

- **Full screen:** a display, with the sound of the computer and the microphone. The main display is the default; with several, the picker has a list. On Linux the desktop's own dialog asks what to share (the picker says so), and so it does for a window.
- **Single window:** one window, chosen in the picker (on macOS the list has the windows that are on screen, those of Dictate not among them), until you stop or close it.
- **Audio only:** no picture, the system's audio and the microphone, either or both.

In the two modes with a picture the system's audio and the microphone can be turned off as well (a video with no sound is allowed). *Video quality* has three presets: **Small** (10 frames a second, up to 1280 px on the long side, about 1 Mbit/s), **Standard** (15 frames, up to 1920 px, 2.5 Mbit/s; the default) and **Sharp** (30 frames, up to 3840 px, 6 Mbit/s). A screen larger than the limit is scaled down and never up. The picker keeps the mode, the quality and the two sounds in the settings (`record_mode`, `record_quality`, `record_mic`, `record_system_audio`) and offers them the next time. It does not keep the display or the window: a window may be gone, and the main display is the default.

**The recording bar.** A small floating bar at the top and in the middle of the main screen, 12 points below the top of its work area, made the first time a recording begins. It shows a dot, the time (without the time spent paused), a meter for each sound that is on, and the buttons **Pause** / **Resume**, **Stop & summarize** and **Discard recording**. Discard asks "Discard this recording?" (**Discard** or **Keep recording**; Esc keeps it) and then moves the recording's folder to the Trash (Recycle Bin), with no job and no summary. While it is being put together the bar says "Saving…". A note shows in the bar when a sound has a problem: "Mic: silent" (nothing but silence has come from it; on Windows the privacy switch for desktop apps keeps a microphone silent), "Mic: reconnected" (see below) or "Mic is off". Drag it by its capsule; it never takes the focus, and each time it is shown it goes back to the top.

**The shortcut and `dictate --record`.** Ctrl+Alt+R (⌃⌥R on the Mac) starts and stops a recording from anywhere. It is fixed (Settings → General shows it, with the line to run from a script). It records with the choices of the last time; when the last mode was *Single window* it opens the picker instead, since the window is not kept. A recording that cannot start from the shortcut opens the picker with the reason. `dictate --record` does the same as pressing it once, also when the app isn't running yet, as `dictate --toggle` does for dictation. Where the shortcut comes from the desktop's GlobalShortcuts portal (see Dictation on Linux) the same goes: with no portal (Sway and other wlroots desktops), bind a shortcut in the desktop's settings to run `dictate --record`.

**ffmpeg.** The video is encoded by ffmpeg, which the installers bundle (where it comes from: [Meetings](#meetings)), so the two modes with a picture work out of the box. The encoder is the first of a list for the system that this ffmpeg has and that works on this machine: on macOS `h264_videotoolbox`, then `libopenh264`, `libx264` and `mpeg4`; on Windows `h264_mf` hardware, `h264_mf` software, `h264_nvenc`, `h264_amf`, `libopenh264`, `libx264` and `mpeg4` (Media Foundation uses the encoder of whatever graphics card there is, Intel, AMD or NVIDIA, so the list needs no other entry for them); on Linux `libvpx-vp9`, then `libopenh264`, `libx264` and `mpeg4`. The ffmpeg that ships has `h264_videotoolbox` and `mpeg4` on macOS, `h264_mf`, `h264_nvenc`, `h264_amf` and `mpeg4` on Windows, and `libvpx_vp9` and `mpeg4` on Linux; the other names are for an ffmpeg that you point `DICTATE_FFMPEG` at. The picker turns the two modes off and says so when ffmpeg is missing (a build in the source tree with none on the system, or a `DICTATE_FFMPEG` that names nothing) or has no encoder that works here, with the command that installs it and a **Copy install command** button: the table under [Meetings](#meetings). **Audio only** works without ffmpeg on macOS and Windows, which have their own AAC encoders; on Linux the sound is then kept as an uncompressed `recording.wav`. The sound of the recording (the system's audio and the microphone) is mixed into one `audio.m4a`, which the transcript is made from, and a recording with a picture also has the video with that sound in it: `recording.mp4` (`recording.webm` when the encoder is VP9). Unlike an imported file, a recording lives in the library folder: it is not a pointer to a file somewhere else.

**When something goes wrong.**

- A device that is lost (the headphones are unplugged, the microphone goes away) does not end the recording: it goes on with the device that the system has now, what came between is silence, and the bar says "reconnected". When no device can be taken up again, that sound is off for the rest of the recording and the bar says so.
- A recording that ends by itself (the window that was recorded is closed, the display is unplugged, Screen Recording is turned off, the desktop's sharing is stopped) is put together as a stop would, and a notification says why: "The window you were recording was closed. What was recorded is kept and is being processed." and the like.
- An app that is ended in the middle of a recording (a crash, `kill -9`) leaves the recording's folder with a `recording.json` in it. The next start finishes it from what is on the disk, before it looks at anything else, and starts its job; the sound is there up to about the last second and the video up to its last keyframe (every 2 s), so a crash loses at most about the last second, and `log.txt` says "The recording was interrupted; what was recorded is kept." A power cut can lose more: the files are not forced to the disk with `fsync`, so what the system had not written yet is gone, and a header that says more sound than the file holds is corrected to what the file holds (with a line in the log). A stem that cannot be read (a crash can cut one inside its header) does not stop that: it is kept as `<name>.unreadable` and the rest is put together, with a line in the log; and a video that ffmpeg cannot read (a long one on a slow disk) is kept as the recording when there is no sound to go with it.
- **Quit** (tray) while a recording goes on puts it together first and then quits; it waits for up to 60 seconds and quits after that anyway (the next start finishes what is left).
- A recording that has nothing in it ("Nothing was recorded") is removed, and the person is told so. Only the library's own files (`meta.json`, the log) are removed with its folder: a folder that holds anything else (a stem that could not be read) is moved to the Trash, never deleted.
- When the sound cannot be put under the video (ffmpeg fails, or is gone at the next start), the recording is the sound (`audio.m4a`: that is what the transcript is made from) and the video is kept beside it as `video.mp4` (`video.webm`), with a line in the log.

**Recording on each system.**

**macOS** 13 or later, Apple silicon and Intel. The screen and the system's audio come from ScreenCaptureKit, and that needs the **Screen Recording** permission, for the sound of the computer as well, even when you record only audio. Settings → General → Permissions has a *Screen Recording* row (with **Open System Settings**, which goes to Privacy & Security → Screen & System Audio Recording, called Screen Recording before macOS 15), and so has the picker when it is off; turn Dictate on there, then reopen the app. macOS 15 asks again every so often (about once a month) whether the app may keep recording the screen: allow it. Without the permission the picker says so, with a button to the pane, and a recording that needs it does not start. `src-tauri/Info.plist` has the reason that the system shows (`NSScreenCaptureUsageDescription`). As with Accessibility, the app is not signed (ad hoc at most), so macOS takes every rebuilt or updated copy for a different app and the permission is lost: remove Dictate from the list with the minus button and add it again. With `pnpm tauri dev` or `target/debug/dictate` started from a terminal, the permission goes to the terminal app. The recording leaves out all of the app's own windows (the bar, Settings, the Meetings window). A recording of a single window is a ScreenCaptureKit stream of that window alone (a desktop-independent window filter), and such a stream carries only the sound of the app that owns the window, so in *Single window* mode the system's audio is that app's and not what other apps play; a full-screen recording has all of it.

**Windows** 10 or 11. The screen and the windows come from the Windows Graphics Capture API, the system's audio from the loopback of the default output device and the microphone from the default input. Dictate's windows (the bar, Settings, the Meetings window) are kept out of the video while a recording goes on, which needs Windows 10 version 2004 or later; the bar also keeps itself out of any capture from the start. The windows of dictation (the bubble and the card) are not kept out, so they can appear in a video that is made while you dictate. A window that is captured has a yellow border drawn by Windows: Dictate asks for none where Windows lets an app do that. The microphone is silent when Settings → Privacy & security → Microphone does not let desktop apps use it, and the bar says "silent".

**Linux.** The screen comes from the desktop's ScreenCast portal (`xdg-desktop-portal` and the backend of your desktop: GNOME, KDE Plasma and Hyprland have one, `xdg-desktop-portal-wlr` serves Sway and other wlroots desktops), as a PipeWire stream, on Wayland and on X11. A desktop that has none (a bare window manager) can't record its screen, and the picker says so. The desktop's own dialog asks what to share; Dictate asks the portal to remember the choice (`screencast_restore_token` in the settings, which has the choice of the last recording, a screen or a window), and a desktop that can (GNOME and KDE Plasma can) records the same kind of thing again without the dialog; going from a screen to a window, or back, asks again. If you stop the sharing from the desktop's own indicator, the recording ends and is kept. The system's audio is recorded by running `parec`, which is in the `pulseaudio-utils` package (it works with the PulseAudio server that PipeWire comes with, as well as with PulseAudio itself): `sudo apt install pulseaudio-utils`. Without it a recording with the system's audio on does not start: the picker says "Recording the system's audio needs parec. Turn system audio off, or install it: sudo apt install pulseaudio-utils", with a button that copies the command (the system's audio is on by default, so this is what a first recording on a system without `parec` meets); turn that sound off or install it. The microphone does not need it. Linux has no way to keep Dictate's own windows out of a capture, so they can appear in the video (the picker says so), the bar too.

**Where the code is.** `crates/dictate-services/src/record/` has what does not depend on the system (the session with its pause, its ends and its recovery, the stems and their mix, the quality presets, the video through ffmpeg, scripted sources for the tests) and `record/capture/` has one source per system (`macos.rs`, `windows.rs`, `linux.rs`, `parec_linux.rs`, `mic.rs`). `src-tauri/src/recording/` has the recorder of the app (one recording at a time, quitting, recovery), the bar (`hud.rs`) and the commands; the picker is `ui/src/meetings/RecordPicker.svelte` and the page of the bar `ui/src/recorder/RecorderApp.svelte`.

## Build it

```sh
pnpm tauri build
```

This builds the interface and the app in release mode and bundles it for the system you are on: the folder `target/release/bundle/` at the root of the repository gets the bundles. The installers that Dictate ships are the `.dmg` on macOS, the NSIS `.exe` on Windows and the `.deb` on Linux, and they are not signed. A plain `pnpm tauri build` has no sidecars and no libraries in them: the installers that go to people are made with the release configuration, by the release workflow, and [Installers](#installers) says how to make one by hand. CI (`ci.yml`) builds the app, and on macOS also its debug `.app`, signed ad hoc to check that Info.plist and Entitlements.plist end up in it.

For a quicker build of just the macOS app, without installers:

```sh
pnpm tauri build --debug --bundles app
```

That makes `target/debug/bundle/macos/Dictate.app`. The bundler signs only when it is told with whom: `APPLE_SIGNING_IDENTITY=- pnpm tauri build --debug --bundles app` signs it ad hoc (this is what CI does), with the right to use the microphone of `src-tauri/Entitlements.plist`; check it with `codesign -d --entitlements - target/debug/bundle/macos/Dictate.app`.

### On Windows: the sherpa-onnx library

Windows takes the static `/MD` build of the release that has no text to speech, from a folder that you name in `SHERPA_ONNX_LIB_DIR`. Two things are wrong with what the build script of `sherpa-onnx-sys` would download for Windows: it is built with the static C runtime (`/MT`), and whisper.cpp and Rust use the DLL one (`/MD`), so the linker stops with `LNK2038` ("mismatch detected for 'RuntimeLibrary'") or `LNK2005`; and it has the speech synthesis in it. The synthesis has espeak-ng (GPL-3.0), and the link takes it into the program even from the `/MD` build that has it, `/OPT:REF` or not (`scripts/check-no-espeak.sh` found it in `dictate.exe` in CI; a link of the same libraries with `lld-link` and `/OPT:REF` kept 237 KB of espeak-ng, 282 of its functions among them, because the one object of the C API that the program needs also holds the synthesis, which refers to espeak-ng). The build without the synthesis, `sherpa-onnx-v1.13.8-win-x64-static-MD-Release-no-tts-lib.tar.bz2` (90 MB, SHA-256 `542348e56e827b59c6d249fd0dfd38dc34b7bd0c521a1ebe9eb6e2db154fa15d`), has none of it. It also lacks three libraries (`espeak-ng.lib`, `piper_phonemize.lib`, `ucd.lib`) that the build script names for every static build, so copies of `kissfft-float.lib` stand in for them (the linker takes nothing from a copy, since the real `kissfft-float.lib` comes first in the list and has every symbol). In PowerShell at the root of the repository:

```powershell
mkdir target\sherpa-onnx-md-no-tts
curl.exe -fL -o sherpa-onnx-md-no-tts.tar.bz2 https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.8/sherpa-onnx-v1.13.8-win-x64-static-MD-Release-no-tts-lib.tar.bz2
if ((Get-FileHash -Algorithm SHA256 sherpa-onnx-md-no-tts.tar.bz2).Hash -ne '542348e56e827b59c6d249fd0dfd38dc34b7bd0c521a1ebe9eb6e2db154fa15d') { throw 'not the archive that CI uses' }
& "$env:SystemRoot\System32\tar.exe" -xjf sherpa-onnx-md-no-tts.tar.bz2 -C target\sherpa-onnx-md-no-tts --strip-components=1
foreach ($name in 'espeak-ng', 'piper_phonemize', 'ucd') { Copy-Item target\sherpa-onnx-md-no-tts\lib\kissfft-float.lib "target\sherpa-onnx-md-no-tts\lib\$name.lib" }
$env:SHERPA_ONNX_LIB_DIR = "$PWD\target\sherpa-onnx-md-no-tts\lib"
```

Then run, build or test as described here, from the same terminal (the variable is only set there; set it for your user in the system settings to keep it). With it set, the build script uses that folder and downloads nothing. CI does the same on Windows, and stops if the SHA-256 of the archive is not the one it pins (`SHERPA_ONNX_MD_SHA256` in `.github/workflows/ci.yml`, the digest GitHub lists for the release asset). The folder is cached under a key that has the start of the pin in it, so a new pin is always downloaded and checked.

### On Linux: the sherpa-onnx library is a shared one, without text to speech

On Linux the speaker separation library is linked as shared libraries (`libsherpa-onnx-c-api.so` and `libonnxruntime.so`) and not statically as on macOS. On x86_64 the static build and whisper.cpp each carry their own copy of libstdc++'s `std::regex`, compiled by different GCC versions, and a program with both aborts with `free(): invalid pointer` the first time it loads the speaker models; the ARM64 static build does not abort, but Linux uses the shared one on every processor so that there is one setup to test.

**The library has to be the build without text to speech, and `SHERPA_ONNX_LIB_DIR` has to name it.** A shared library has all of its own code in it, whatever the program uses, and the ordinary one has the speech synthesis of sherpa-onnx in it, with espeak-ng (GPL-3.0), which Dictate has no use for and must not ship. The release has a build without the synthesis for x86_64, `sherpa-onnx-v1.13.8-linux-x64-shared-no-tts-lib.tar.bz2` (9 MB, SHA-256 `bf2d998c8b07012cd5098f3b92673bc1333fd9b927767d7cb664be8190d8bc0b`). From the root of the repository:

```sh
export SHERPA_ONNX_LIB_DIR="$(scripts/fetch-sherpa-onnx-no-tts.sh)"
```

That downloads the archive into `target/sherpa-onnx-no-tts`, checks its SHA-256 before it unpacks anything (it stops if it is not the pinned one), and prints the `lib` folder, which the variable then names (it does nothing but print when the folder is there already). Run the build, `cargo test` or `pnpm tauri dev` from the same shell, or set the variable for your user. Without it the build stops with a message that says this: `crates/dictate-services/build.rs` requires it on Linux, because the library that the build script of `sherpa-onnx-sys` would download has espeak-ng in it. By hand: download the archive from the [v1.13.8 release](https://github.com/k2-fsa/sherpa-onnx/releases/tag/v1.13.8), check the SHA-256, unpack it and set the variable to its `lib` folder. CI does the same in every Linux job, and `scripts/check-no-espeak.sh` checks the libraries that the build makes.

There is no build without the synthesis for ARM64 Linux yet: the release has only `linux-aarch64-shared-cpu-lib`, which has espeak-ng in it. Build sherpa-onnx v1.13.8 from source with `-DSHERPA_ONNX_ENABLE_TTS=OFF -DBUILD_SHARED_LIBS=ON` and set `SHERPA_ONNX_LIB_DIR` to the `lib` folder of its install, or wait until one is published. For a build of your own that you give to no one, the `lib` folder of the `linux-aarch64-shared-cpu-lib` archive works as well; what is built with it has espeak-ng in it, so it must not be distributed.

The build copies every `.so` file of the folder (the two above and `libsherpa-onnx-cxx-api.so`) into `target/debug` or `target/release`, and the program and the tests look for them there (`$ORIGIN` and `$ORIGIN/..` in their run path). The build script of `dictate-services` copies them again whenever they are missing, for example after a CI cache restore, which keeps only part of `target`. So `pnpm tauri dev`, `cargo run` and `cargo test` need nothing more once the variable is set. **A package of the app has to put `libsherpa-onnx-c-api.so` and `libonnxruntime.so` (the ones without the synthesis) in a folder that the run path of the program names.** The `.deb` does: `tauri.release.conf.json` puts the two libraries in `/usr/lib/dictate` (`bundle.linux.deb.files`), and `build.rs` gives the program the run path `$ORIGIN/../lib/dictate`, which is `/usr/lib/dictate` for the program in `/usr/bin`. A build in the source tree has the other run paths as well (above); the build of an installer sets `DICTATE_RPATH_INSTALLED=1`, which leaves only the installed one, so that the program does not look for libraries in `/usr` or `/`. A bare `pnpm tauri build` (without the release configuration) makes a package that does not start, since the libraries are not in it.

## Check it

```sh
pnpm --dir ui check   # types and Svelte warnings
pnpm --dir ui test    # the interface's tests
cargo test --all      # the Rust tests, from anywhere in the repository
```

CI runs these on macOS, Windows and Linux for every pull request, together with a build of the app and a check of the Rust crates with the oldest Rust they support (1.85), and a recording of three seconds of the primary monitor on the Windows runner.

The tests that record this machine are ignored, since they use its screen, its sound or its microphone; all but the microphone test stop with a message unless you ask for them with an environment variable. Run them on a desktop session of your own. The ones that record the screen need ffmpeg: without it they print "skipped: ffmpeg is not installed" and pass, having recorded nothing. On Linux and Windows set `SHERPA_ONNX_LIB_DIR` first, as for any build.

```sh
# macOS (the terminal needs Screen Recording; the microphone test needs Microphone). The screen tests share the one stream that
# ScreenCaptureKit gives the process, so they run one at a time
DICTATE_TEST_SCREEN=1 cargo test -p dictate-services --lib --locked -- --ignored --nocapture --test-threads=1 macos_
cargo test -p dictate-services --lib --locked -- --ignored --nocapture the_real_microphone_delivers_stamped_audio
# Linux (a desktop session with the ScreenCast portal: you choose what to share in its dialog; the second one plays a tone)
DICTATE_TEST_SCREEN=1 cargo test -p dictate-services --lib --locked -- --ignored --nocapture linux_records_three_seconds_through_the_portal
DICTATE_TEST_REAL_AUDIO=1 cargo test -p dictate-services --lib --locked -- --ignored --nocapture linux_parec_reads_the_monitor
# Windows, in PowerShell (the first runs two tests: the primary monitor, and a window of the classic notepad.exe until it is closed; the second plays a tone)
$env:DICTATE_TEST_SCREEN = "1"; cargo test -p dictate-services --lib --locked -- --ignored --nocapture windows_records
$env:DICTATE_TEST_REAL_AUDIO = "1"; cargo test -p dictate-services --lib --locked -- --ignored --nocapture windows_loopback_delivers_audio_while_something_plays
```

## Installers

What an installer has in it beyond the app, and the scripts that put it there. An ordinary `cargo build`, `cargo test` or `pnpm tauri build` needs none of this: the sidecars and the libraries are named only by the **release overlay**, `src-tauri/tauri.release.conf.json` (with `tauri.release.windows.conf.json` for the Windows runtime), which is added to `tauri.conf.json` with `--config`.

- **`dictate-mcp`**, the MCP server, is a sidecar (`bundle.externalBin`): the bundler takes `binaries/dictate-mcp-<target triple>` and puts it next to the program under the name `dictate-mcp`: `Dictate.app/Contents/MacOS/` on macOS, the install folder on Windows (`%LOCALAPPDATA%\Dictate`), `/usr/bin` with the `.deb`.
- **`dictate-ffmpeg`**, an LGPL build of FFmpeg 9.0.2, is the other sidecar. It is not called `ffmpeg` because the `.deb` puts its programs in `/usr/bin`, where the `ffmpeg` of the distribution is. `scripts/build-ffmpeg.sh <target> <folder>` builds it from the pinned source (the SHA-256 of the tarball is checked before anything is unpacked), with every demuxer, decoder, parser and bitstream filter and only the encoders, muxers, filters and protocols that the app uses, under the LGPL v2.1 or later (`--disable-gpl --disable-nonfree --disable-version3`); it writes the program, `BUILDCONF-<target>.txt` (the configure line) and the source tarball. macOS is built on a Mac (the Intel one is cross-built on Apple silicon), Windows with mingw-w64 on Linux, Linux fully static on Ubuntu 24.04. `scripts/check-ffmpeg-lgpl.sh` fails on anything that is not a plain LGPL build.
- **The Linux libraries**: `libsherpa-onnx-c-api.so` and `libonnxruntime.so` of the build without text to speech, in `/usr/lib/dictate` (`bundle.linux.deb.files`), with the run path `$ORIGIN/../lib/dictate` on the program (see [On Linux](#on-linux-the-sherpa-onnx-library-is-a-shared-one-without-text-to-speech)). The package `depends` on ALSA and PipeWire (`libasound2t64 | libasound2`, `libpipewire-0.3-0t64 | libpipewire-0.3-0`) and `recommends` GStreamer's libav and good plugins, `xdg-desktop-portal` and `pulseaudio-utils`.
- **The Windows runtime**: `vcruntime140.dll`, `vcruntime140_1.dll` and `msvcp140.dll`, next to `dictate.exe`, so that a Windows without the Visual C++ runtime starts the app (Microsoft allows this deployment). The installer is per user (`installMode: currentUser`) and downloads WebView2 when the system has none.
- **No signing.** macOS gets the ad hoc identity `-` (Tauri signs the sidecars with it, inside out) and the Windows installer is not signed.

To make one by hand, from the root of the repository, with the prerequisites above and `<target>` one of `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-pc-windows-msvc` and `x86_64-unknown-linux-gnu`:

```sh
scripts/build-ffmpeg.sh <target> target/ffmpeg/<target>
scripts/stage-sidecars.sh <target>
DICTATE_RPATH_INSTALLED=1 pnpm --dir app tauri build --target <target> --bundles <dmg, nsis or deb> --config src-tauri/tauri.release.conf.json
scripts/check-artifact.sh target/<target>/release/bundle/<kind>/<the installer>
```

`stage-sidecars.sh` builds `dictate-mcp` for the target and puts everything in `app/src-tauri/binaries/` and `app/src-tauri/libs/` (both ignored by git); on Windows add `--config src-tauri/tauri.release.windows.conf.json` too. `check-artifact.sh` looks inside the installer: both sidecars next to the program (and executable), no espeak-ng (`scripts/check-no-espeak.sh`), a plain LGPL ffmpeg, the libraries and the run path of the `.deb`, the three DLLs of the Windows runtime, and on macOS the usage descriptions, the signature and the architecture. The release workflow runs the same, and writes `<installer>.sha256` (the line `<hash>  <name>`) only for an installer that passes.

The installers are named `Dictate_<version>_aarch64.dmg`, `Dictate_<version>_x64.dmg`, `Dictate_<version>_x64-setup.exe` and `Dictate_<version>_amd64.deb`; the version is the one of the workspace `Cargo.toml`.

## Cut a release

Versions come from [Conventional Commits](https://www.conventionalcommits.org) through [release-please](https://github.com/googleapis/release-please): `release-please.yml` runs on every push to `main`, `release.yml` builds the installers, and the maintainer decides when by merging one pull request.

1. **Once, in the repository settings:** Settings → Actions → General → Workflow permissions → tick **Allow GitHub Actions to create and approve pull requests**. release-please opens its pull request with the workflow token, and without this setting the job fails.
2. **Try the installers first.** `gh workflow run release.yml --ref <branch>` with no tag is a dry run: it builds and checks all four installers and keeps them (with their `.sha256` files, the FFmpeg source, the `BUILDCONF` files and the notices) as artifacts of the run for a week. It uploads nothing to any release.
3. **Keep the notices true.** `THIRD_PARTY_NOTICES.md` must be at the root of the repository: the release fails at the start, in its first job, when it is missing at the tag. Update it with the change that needs it: a new native component, a new FFmpeg version or flags (the version, the SHA-256 and the lists of encoders and muxers in the file are pinned to `scripts/build-ffmpeg.sh` by `app/ui/src/docs.test.ts`), or new Rust crates (its section 4 says how that list is made).
4. **Merge the release pull request.** release-please keeps a pull request named "chore(main): release x.y.z" up to date: it bumps the version in `Cargo.toml` (the workspace), the four crates in `Cargo.lock`, `app/ui/package.json` and `version.txt` (`release-please-config.json` lists them) and writes `CHANGELOG.md`. For the first release, `release-as` in `release-please-config.json` makes it 2.0.0; **remove that line once 2.0.0 is out**, or every later release pull request proposes 2.0.0 again. Merge the pull request with a squash.
5. **What happens then.** release-please tags `vx.y.z` and publishes the GitHub Release with the notes of `CHANGELOG.md`, and calls `release.yml` with the tag, which builds the tag (not the tip of `main`) on four runners (macOS Apple silicon, macOS Intel cross-built, Windows, Linux), checks each installer with `scripts/check-artifact.sh` and **uploads to the release** with `gh release upload --clobber`: the four installers and their four `.sha256` files, then `ffmpeg-9.0.2.tar.xz` (the source of the FFmpeg inside them), the four `BUILDCONF-<target>.txt` files and `THIRD_PARTY_NOTICES.md`, which the written offer of the source in the notices points at. The workflow never creates or edits the release itself, so the notes release-please wrote stay. A row whose installer fails a check is not uploaded, and nothing of the second group goes up when a row has failed.
6. **After the upload.** Check that the release page has all of those files, try the installers by hand on a clean machine of each system, and, if a row failed, run the failed jobs again (`--clobber` replaces what is there).
