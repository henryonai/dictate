# Dictate app

The desktop app, for macOS, Windows and Linux: a tray icon, a Settings window and a Meetings window. It is built with [Tauri 2](https://tauri.app): a Rust shell in `src-tauri/` and a Svelte 5 + TypeScript interface in `ui/`.

Dictation works: press the global shortcut (Ctrl+Alt+Space by default, ⌃⌥Space on the Mac; it can be changed in Settings), talk, and press it again (or hold it, in hold-to-talk mode): the text is written into the text field you were in, and a small chip by it lets you write it again in another style. With no text field to write into, or with the setting that writes into fields off, a card opens to edit and copy the text. Local Whisper (a model downloaded in Settings) turns the speech into text and shows it live while you talk; the transcription API and the AI edit are optional. Settings (language, AI provider, transcription service), API keys in the system's key store, the Whisper model downloads and the connection checks work. `dictate --toggle` (also when the app isn't running yet) does what pressing the shortcut once does, and so does the first item of the tray menu. The audio and the text of each dictation are kept in `Dictate/Dictations` in the system's local data folder (the tray menu opens it) when "Save recordings + transcripts" is on; a dictation that isn't saved leaves nothing behind. Audio goes to the transcription API only when that is the chosen engine. On a Wayland desktop the shortcut comes from the desktop's GlobalShortcuts portal (KDE Plasma, GNOME 48 and later, Hyprland); where there is none (Sway and other wlroots desktops), bind a shortcut in the desktop's own settings to run `dictate --toggle`.

The Meetings window (tray menu → **Meetings…**) imports audio and video files and links, and turns each into a transcript with the speakers told apart and, when an AI is set up, a summary: see [Meetings](#meetings). So far the app has only been run on macOS. Windows and Linux are compiled and tested by CI, but nobody has run the app there yet.

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

- **macOS:** the Xcode Command Line Tools (`xcode-select --install`) and CMake (`brew install cmake`).
- **Windows 10 or 11:** the [Microsoft C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with the "Desktop development with C++" workload, CMake and LLVM (for libclang), and the WebView2 runtime, which Windows 11 has already. CMake and LLVM can come from winget, one command each:

  ```sh
  winget install Kitware.CMake
  winget install LLVM.LLVM
  ```

  Open a new terminal afterwards, so that it finds the new tools. If the build can't find libclang, set `LIBCLANG_PATH` to the `bin` folder of the LLVM install.
- **Linux:** the web view, tray, audio (ALSA, for the microphone) and build libraries. On Debian and Ubuntu:

  ```sh
  sudo apt-get install -y build-essential cmake libclang-dev libasound2-dev \
    libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev
  ```

  Other distributions have the same libraries under their own names; the [Tauri prerequisites](https://tauri.app/start/prerequisites/#linux) list them. The tray icon needs a desktop that shows app indicators (GNOME needs the AppIndicator extension).

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

**yt-dlp and ffmpeg.** Links are downloaded with yt-dlp, and ffmpeg reads the formats that Dictate can't decode itself. Both are looked for on your `PATH` and in the usual package-manager folders (Homebrew, `/usr/local/bin`, `~/.local/bin` where pipx puts yt-dlp). Settings → Speech → **Tools for links** shows which were found, with their versions, and the command for the ones that are missing, which is the command of the system you run on:

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

## Build it

```sh
pnpm tauri build
```

This builds the interface and the app in release mode and bundles it for the system you are on: the folder `target/release/bundle/` at the root of the repository gets the `.app` and `.dmg` on macOS, the `.msi` and `.exe` installers on Windows and the `.deb`, `.rpm` and AppImage on Linux. The bundles are not signed. Only the macOS `.app` has been built and run so far; the installers are untested. CI builds the app, and on macOS also its debug `.app`, signed ad hoc to check that Info.plist and Entitlements.plist end up in it, but not the installers.

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

The build copies every `.so` file of the folder (the two above and `libsherpa-onnx-cxx-api.so`) into `target/debug` or `target/release`, and the program and the tests look for them there (`$ORIGIN` and `$ORIGIN/..` in their run path). The build script of `dictate-services` copies them again whenever they are missing, for example after a CI cache restore, which keeps only part of `target`. So `pnpm tauri dev`, `cargo run` and `cargo test` need nothing more once the variable is set. **A package of the app has to put `libsherpa-onnx-c-api.so` and `libonnxruntime.so` (the ones without the synthesis) next to the program** (or in a folder that its run path names); the bundles that `pnpm tauri build` makes do not do that yet, so a `.deb` or an AppImage made from them does not start until they do.

## Check it

```sh
pnpm --dir ui check   # types and Svelte warnings
pnpm --dir ui test    # the interface's tests
cargo test --all      # the Rust tests, from anywhere in the repository
```

CI runs these on macOS, Windows and Linux for every pull request, together with a build of the app and a check of the Rust crates with the oldest Rust they support (1.85).
