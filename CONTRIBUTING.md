# Contributing to Dictate

Thanks for helping. Bug reports, ideas and pull requests are all welcome. For a bigger change, open an issue first so we can agree on the approach before you write the code.

Everyone taking part follows the [Code of Conduct](CODE_OF_CONDUCT.md). Security problems go by email, not in an issue: see [SECURITY.md](SECURITY.md).

## Reporting a bug

Open an [issue](https://github.com/henryonai/dictate/issues/new/choose) with the bug report form. It asks for:

- what you did, what you expected and what happened;
- your Dictate version (the number in the name of the installer, or the commit you built), your operating system and how you installed Dictate (`.dmg`, `.exe`, `.deb` or a build from source);
- for a recording, its log: in the library, ⋯ → **View log** → **Copy** (remove anything private first). For anything else, start Dictate from a terminal with `RUST_LOG=dictate=debug` and paste what it prints.

## Building and running

The app is a [Tauri 2](https://tauri.app) program: a Rust shell in `app/src-tauri/` and a Svelte 5 interface in `app/ui/`. You need:

- **Rust 1.85 or newer** (rustup; `rust-toolchain.toml` picks the stable toolchain, and 1.85 is the oldest that CI checks), **Node.js 22**, **pnpm 12** (or Corepack, which fetches the version that `app/package.json` names), **CMake** and **libclang** (whisper.cpp is built from source with the app);
- **macOS:** Xcode 16 or newer, or the Command Line Tools with the macOS 15 SDK or newer (`xcode-select --install`), and `brew install cmake`;
- **Windows 10 or 11:** the [Microsoft C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) ("Desktop development with C++"), and `winget install Kitware.CMake LLVM.LLVM`;
- **Linux** (Ubuntu 24.04; the same libraries have other names on other distributions):

  ```sh
  sudo apt-get install -y build-essential cmake libclang-dev libasound2-dev \
    libpipewire-0.3-dev libspa-0.2-dev \
    libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev
  ```

On **Windows and Linux** the speaker separation library (sherpa-onnx) has to be the build without text to speech, because the one that a build would download has espeak-ng (GPL-3.0) in it, and you name it with `SHERPA_ONNX_LIB_DIR`. [app/README.md](app/README.md) has the steps for both systems, the details of each system's prerequisites, and the variables that the tests use (`DICTATE_MEETINGS_DIR`, `DICTATE_FFMPEG`, `DICTATE_TEST_SCREEN` and the others). On macOS the build downloads the library by itself.

Then, from the `app` folder:

```sh
pnpm install
pnpm tauri dev
```

The first Rust build takes a few minutes. The app lives in the tray; the Settings window opens at the first start. With `pnpm tauri dev` the permissions of a Mac go to the terminal that you started it from.

## Checks

CI runs these on macOS, Windows and Linux for every pull request, and so should you before you push (from the root of the repository):

```sh
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all --locked
pnpm --dir app/ui check
pnpm --dir app/ui test
```

and, on Linux, that the crates still build with the oldest Rust that they support: `cargo +1.85 check --workspace --all-targets --locked`. CI also builds the app and checks that no espeak-ng is in it. Tests that need ffmpeg skip when it is not installed, and fail when the `CI` environment variable is set, so `CI=true cargo test --all --locked` is what CI does.

Some tests read files of the repository as text, so that a change in one place cannot leave another one wrong: `app/ui/src/ci-workflow.test.ts` (the CI workflow and the packages the README lists), `release-workflow.test.ts` and `release-config.test.ts` (the release), and `docs.test.ts` (the READMEs, the notices and the templates). If one fails after you changed a document or a workflow, the message says what has to change with it.

## Code layout

| Folder | Contents |
|---|---|
| `crates/dictate-core/` | Plain logic shared by the app and the MCP server: the library's file format (`meetings/`), summaries, transcripts, speaker names, timestamps, diacritic-insensitive search, the MCP protocol and its tools |
| `crates/dictate-services/` | Everything that talks to the system or to a service: audio capture, Whisper and speaker separation, the AI and transcription clients, the pipeline of a meeting, recording (`record/`), finding ffmpeg and yt-dlp |
| `crates/dictate-mcp/` | The `dictate-mcp` program |
| `app/src-tauri/` | The Tauri shell: the tray, the windows, the commands, the dictation controller, the recorder, the settings of the system |
| `app/ui/` | The interface (Svelte 5, Vite, Vitest): `settings/`, `meetings/`, `panel/` (the floating dictation windows), `recorder/` (the recording bar), `locales/` |
| `scripts/` | What CI and the release use: the ffmpeg build, the checks of an installer, fetching sherpa-onnx, the golden comparison with the Swift MCP server |
| `tests/golden/` | The outputs of the Swift MCP server that `dictate-mcp` has to match byte for byte |
| `docs/` | The design spec and the plans |

## Translating the interface

The interface is in English and Vietnamese, in `app/ui/src/locales/en.json` and `vi.json`, and English is the source. To add or change a text, give it a key and write it in **both** files, in the same place:

1. Use the key in the interface with `t("area.name")` (a text with a value takes `{name}` in it: `t("area.name", { name })`).
2. Add `"area.name": "…"` to `en.json` and the same key to `vi.json`, in natural Vietnamese (the files hold the way the app already talks; look at the keys around yours).
3. Run `pnpm --dir app/ui test`. Two tests guard the two files:
   - `lib/i18n.test.ts` fails when one file has a key that the other does not.
   - `locales/catalog.test.ts` fails when a key that the code names has no text in either file. It reads the Rust and the Svelte sources for the keys they use: the title and the note of every Whisper model of the catalog, the title of every edit preset, the `dictation.*` and `audio.*` error keys of the dictation controller, the `meetings.*` keys of the Meetings window, its store and its formats, and the `recording.*` keys of the picker, the bar and the recorder (why a start failed, why a recording ended). A text that you add for one of those areas is found by it; a key that is used somewhere else is not, so the parity test is what keeps those two files in step.

The tray menu is written in Rust, with the two languages side by side (`app/src-tauri/src/tray.rs`), and the banners of the meetings are built in Rust from the same words as the `meetings.notify.*` keys (`app/src-tauri/src/meetings/notify.rs`, which has a test that keeps the two in step): change both together.

## Pull requests

- Keep one change per pull request, and make sure the checks above pass. The pull request template has a short checklist.
- When a change is visible to users, update both `README.md` and `README.vi.md`.
- A new dependency or a new component that ships in the installers needs its license in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md), and the file of license texts that the installers carry has to be written again: run `python3 scripts/generate-licenses.py` (it needs `cargo` and `pnpm --dir app install`) and commit `THIRD_PARTY_LICENSES.txt`. A test fails until you do, whenever `Cargo.lock` has changed. Code under the GPL or the AGPL cannot go into an installer, and the LGPL one only the way that FFmpeg does (a separate program that can be replaced).
- Pull requests are merged with **rebase** or **squash**, never with a merge commit: a merge commit repeats the change in the release notes.

## Commits and releases

Releases are made by [release-please](https://github.com/googleapis/release-please) from commit messages written as [Conventional Commits](https://www.conventionalcommits.org):

| Commit message | Next version |
|---|---|
| `fix: …`, `perf: …` | patch: 2.0.0 → 2.0.1 |
| `feat: …` | minor: 2.0.0 → 2.1.0 |
| `feat!: …`, or a `BREAKING CHANGE:` footer | major: 2.0.0 → 3.0.0 |
| `docs:`, `refactor:`, `test:`, `ci:`, `chore:` | no release on their own |

Commits that don't follow the format are left out of the release notes and don't trigger a release. **Don't put AI attribution in a commit message**: no `Co-Authored-By` trailer that names an AI tool, and no line that says a tool wrote it.

After each push to `main`, release-please opens or updates a pull request named "chore(main): release x.y.z" that bumps the version everywhere (the workspace `Cargo.toml`, `Cargo.lock`, `app/ui/package.json` and `version.txt`) and adds the release notes to `CHANGELOG.md`. Merging it (squash) tags `vx.y.z`, publishes the GitHub Release and builds and attaches the installers. [app/README.md](app/README.md#cut-a-release) says what the maintainer does, and what the workflow attaches.

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).
