# Dictate app

The desktop app, for macOS, Windows and Linux: a tray icon and a Settings window. It is built with [Tauri 2](https://tauri.app): a Rust shell in `src-tauri/` and a Svelte 5 + TypeScript interface in `ui/`.

Dictation works: press the global shortcut (Ctrl+Alt+Space by default, ⌃⌥Space on the Mac; it can be changed in Settings), talk, and press it again (or hold it, in hold-to-talk mode): the text is written into the text field you were in, and a small chip by it lets you write it again in another style. With no text field to write into, or with the setting that writes into fields off, a card opens to edit and copy the text. Local Whisper (a model downloaded in Settings) turns the speech into text and shows it live while you talk; the transcription API and the AI edit are optional. Settings (language, AI provider, transcription service), API keys in the system's key store, the Whisper model downloads and the connection checks work. `dictate --toggle` (also when the app isn't running yet) does what pressing the shortcut once does, and so does the first item of the tray menu. The audio and the text of each dictation are kept in `Dictate/Dictations` in the system's local data folder (the tray menu opens it) when "Save recordings + transcripts" is on; a dictation that isn't saved leaves nothing behind. Audio goes to the transcription API only when that is the chosen engine. On a Wayland desktop the shortcut comes from the desktop's GlobalShortcuts portal (KDE Plasma, GNOME 48 and later, Hyprland); where there is none (Sway and other wlroots desktops), bind a shortcut in the desktop's own settings to run `dictate --toggle`.

So far the app has only been run on macOS. Windows and Linux are compiled and tested by CI, but nobody has run the app there yet.

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
- **A network connection for the first build.** The speaker separation library (sherpa-onnx, with onnxruntime inside) is not built from source: its build script downloads a prebuilt one (about 20 MB on macOS, a shared library of about 10 MB on Linux) into `target/sherpa-onnx-prebuilt` the first time. To build offline, put that archive in a folder and set `SHERPA_ONNX_ARCHIVE_DIR` to it, or set `SHERPA_ONNX_LIB_DIR` to the `lib` folder of an unpacked one. On Windows it comes from somewhere else, see [Build it](#build-it).

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

The speaker separation library that the build script downloads for Windows is built with the static C runtime (`/MT`), and whisper.cpp and Rust use the DLL one (`/MD`): the linker stops with `LNK2038` ("mismatch detected for 'RuntimeLibrary'") or `LNK2005` when it has both. The same release has a build for `/MD`. Download `sherpa-onnx-v1.13.8-win-x64-static-MD-Release-lib.tar.bz2` (92 MB) from the [v1.13.8 release](https://github.com/k2-fsa/sherpa-onnx/releases/tag/v1.13.8) (it is not the `no-tts` one) and unpack it, in PowerShell at the root of the repository:

```powershell
mkdir target\sherpa-onnx-md
curl.exe -fL -o sherpa-onnx-md.tar.bz2 https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.8/sherpa-onnx-v1.13.8-win-x64-static-MD-Release-lib.tar.bz2
& "$env:SystemRoot\System32\tar.exe" -xjf sherpa-onnx-md.tar.bz2 -C target\sherpa-onnx-md --strip-components=1
$env:SHERPA_ONNX_LIB_DIR = "$PWD\target\sherpa-onnx-md\lib"
```

Then run, build or test as described here, from the same terminal (the variable is only set there; set it for your user in the system settings to keep it). With it set, the build script uses that folder and downloads nothing. CI does the same on Windows.

### On Linux: the sherpa-onnx library is a shared one

On Linux the speaker separation library is linked as shared libraries (`libsherpa-onnx-c-api.so` and `libonnxruntime.so`) and not statically as on macOS and Windows: the static build and whisper.cpp each carry their own copy of libstdc++'s `std::regex`, compiled by different GCC versions, and a program with both aborts with `free(): invalid pointer` the first time it loads the speaker models. The build copies the two `.so` files into `target/debug` or `target/release`, and the program and the tests look for them there (`$ORIGIN` and `$ORIGIN/..` in their run path), so `pnpm tauri dev`, `cargo run` and `cargo test` need nothing more. **A package of the app has to put the two `.so` files next to the program** (or in a folder that its run path names); the bundles that `pnpm tauri build` makes do not do that yet, so a `.deb` or an AppImage made from them does not start until they do.

## Check it

```sh
pnpm --dir ui check   # types and Svelte warnings
pnpm --dir ui test    # the interface's tests
cargo test --all      # the Rust tests, from anywhere in the repository
```

CI runs these on macOS, Windows and Linux for every pull request, together with a build of the app and a check of the Rust crates with the oldest Rust they support (1.85).
