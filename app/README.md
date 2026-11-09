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

## Build it

```sh
pnpm tauri build
```

This builds the interface and the app in release mode and bundles it for the system you are on: the folder `target/release/bundle/` at the root of the repository gets the `.app` and `.dmg` on macOS, the `.msi` and `.exe` installers on Windows and the `.deb`, `.rpm` and AppImage on Linux. The bundles are not signed. Only the macOS `.app` has been built and run so far; the installers are untested, and CI builds the app but not its bundles.

For a quicker build of just the macOS app, without installers:

```sh
pnpm tauri build --debug --bundles app
```

That makes `target/debug/bundle/macos/Dictate.app`.

## Check it

```sh
pnpm --dir ui check   # types and Svelte warnings
pnpm --dir ui test    # the interface's tests
cargo test --all      # the Rust tests, from anywhere in the repository
```

CI runs these on macOS, Windows and Linux for every pull request, together with a build of the app and a check of the Rust crates with the oldest Rust they support (1.85).
