# Third-party notices

Dictate is © 2026 Henry Hoang, under the [MIT License](LICENSE). It includes, uses or downloads the software below, each under its own license. This file is attached to every [release](https://github.com/henryonai/dictate/releases), and Settings → General → **Licenses** opens it.

Three more files go with it, in the repository and **in every installer**:

- `THIRD_PARTY_LICENSES.txt` has the full license text, with the copyright lines and the notices, of each native component and of every Rust crate and npm package that the program is built from (the Apache `NOTICE` files that exist are in it). `scripts/generate-licenses.py` writes it.
- `COPYING.LGPLv2.1` is the text of the GNU Lesser General Public License version 2.1, for FFmpeg and glibc.
- `LICENSE` is the MIT License of Dictate.

Where they are after an installation: macOS, in `Dictate.app/Contents/Resources/Licenses/`; Windows, in the `Licenses` folder of the install folder (next to `dictate.exe`); the `.deb`, in `/usr/share/doc/dictate/` (with a `copyright` file there, for the tools of Debian).

What is in this file:

1. [FFmpeg](#1-ffmpeg-lgpl-21-or-later): the license, the written offer of its source, how it is built and how to replace it.
2. [What is linked into the FFmpeg that ships](#2-what-is-linked-into-the-ffmpeg-that-ships): glibc, libgcc, libvpx, libopus, zlib, mingw-w64, winpthreads and the encoder headers.
3. [The other components of the app](#3-the-other-components-of-the-app): whisper.cpp, the models, sherpa-onnx, ONNX Runtime, Tauri and the rest.
4. [Rust crates](#4-rust-crates) and [npm packages](#5-npm-packages): the licenses of the libraries that the program is built from, and how this list was made.

## 1. FFmpeg (LGPL 2.1 or later)

The installers of Dictate carry a build of [FFmpeg](https://ffmpeg.org) 9.0.2. Dictate starts it as a program of its own, `dictate-ffmpeg`, next to the main program (`Dictate.app/Contents/MacOS/dictate-ffmpeg` on macOS, `dictate-ffmpeg.exe` in the install folder on Windows, `/usr/bin/dictate-ffmpeg` with the `.deb`), and does not link its libraries.

- **License:** the GNU Lesser General Public License, version 2.1 or later (LGPL v2.1+). The text is `COPYING.LGPLv2.1` (next to this file, and in the installers), the same file as in the source of FFmpeg; it is also at <https://www.gnu.org/licenses/old-licenses/lgpl-2.1.html>. The build is made with `--disable-gpl --disable-nonfree --disable-version3`, and its `configure` has to print `License: LGPL version 2.1 or later` (`scripts/build-ffmpeg.sh` stops otherwise); CI checks every `dictate-ffmpeg` that goes into an installer (`scripts/check-ffmpeg-lgpl.sh`). FFmpeg is a trademark of Fabrice Bellard, originator of the FFmpeg project.
- **What it is for:** reading the audio and video formats that Dictate does not decode itself, encoding the video and the sound of a recording, and giving yt-dlp a program to join and convert what it downloads.

### Written offer of the source

Dictate includes FFmpeg (<https://ffmpeg.org>), licensed under the GNU LGPL v2.1 or later. The corresponding source of the FFmpeg that is in a release, the build script and the configure lines are at <https://github.com/henryonai/dictate/releases>:

- **The source:** the exact, unmodified `ffmpeg-9.0.2.tar.xz` (we apply no patches), attached to every release. Its SHA-256 is `8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e`, and it is the same file as <https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz>.
- **The configure lines:** `BUILDCONF-aarch64-apple-darwin.txt`, `BUILDCONF-x86_64-apple-darwin.txt`, `BUILDCONF-x86_64-pc-windows-msvc.txt` and `BUILDCONF-x86_64-unknown-linux-gnu.txt`, attached to every release: each is the configure line of the program of that installer (the output of `ffmpeg -buildconf`). The lines are copied below.
- **The build script:** `scripts/build-ffmpeg.sh` in this repository, at the tag of the release. It downloads the source, checks the SHA-256 above before it unpacks anything, runs `configure` with the lines below and builds the program for one target. Anyone can run it to make the same program, or change it to link another library.

This offer is valid for at least three years after the release that it comes with, and for as long as that release stays on the page. If something above is missing there, write to <henryhoang.tech@gmail.com> or open an [issue](https://github.com/henryonai/dictate/issues), and the source of FFmpeg, or of any other library named in section 2 that is inside `dictate-ffmpeg`, will be sent to you.

### How it is configured

The line that `configure` of each build got, as `ffmpeg -buildconf` of the program prints it (the `BUILDCONF` files of the release are these lines; `<build folder>` stands for the temporary folder in which the Windows build put the header files of the encoders). Every line starts with the same flags, which turn everything off (`--disable-everything`) and then turn on every demuxer, decoder, parser and bitstream filter (an import can need any of them) and only what the app asks of the rest:

- **macOS, Apple silicon (aarch64-apple-darwin):**

  ```
  --disable-gpl --disable-nonfree --disable-version3 --disable-autodetect --disable-everything --disable-doc --disable-debug --disable-network --disable-ffplay --disable-ffprobe --disable-avdevice --enable-demuxers --enable-decoders --enable-parsers --enable-bsfs --enable-protocol='file,pipe' --enable-filter='buffer,buffersink,abuffer,abuffersink,null,anull,format,aformat,aresample,scale,fps,atrim,trim,asetpts,setpts,volume' --enable-encoder=wrapped_avframe --cc='clang -arch arm64' --arch=arm64 --target-os=darwin --extra-cflags='-mmacosx-version-min=13.0' --extra-ldflags='-mmacosx-version-min=13.0' --enable-zlib --enable-videotoolbox --enable-audiotoolbox --enable-encoder='aac,h264_videotoolbox,mpeg4,pcm_f32le,pcm_s16le' --enable-muxer='ipod,mp4,null,pcm_f32le'
  ```

- **macOS, Intel (x86_64-apple-darwin, cross-built on an Apple silicon Mac):**

  ```
  --disable-gpl --disable-nonfree --disable-version3 --disable-autodetect --disable-everything --disable-doc --disable-debug --disable-network --disable-ffplay --disable-ffprobe --disable-avdevice --enable-demuxers --enable-decoders --enable-parsers --enable-bsfs --enable-protocol='file,pipe' --enable-filter='buffer,buffersink,abuffer,abuffersink,null,anull,format,aformat,aresample,scale,fps,atrim,trim,asetpts,setpts,volume' --enable-encoder=wrapped_avframe --cc='clang -arch x86_64' --arch=x86_64 --target-os=darwin --extra-cflags='-mmacosx-version-min=13.0' --extra-ldflags='-mmacosx-version-min=13.0' --enable-zlib --enable-videotoolbox --enable-audiotoolbox --enable-encoder='aac,h264_videotoolbox,mpeg4,pcm_f32le,pcm_s16le' --enable-muxer='ipod,mp4,null,pcm_f32le' --enable-cross-compile
  ```

- **Windows (x86_64-pc-windows-msvc, built on Linux with mingw-w64):**

  ```
  --disable-gpl --disable-nonfree --disable-version3 --disable-autodetect --disable-everything --disable-doc --disable-debug --disable-network --disable-ffplay --disable-ffprobe --disable-avdevice --enable-demuxers --enable-decoders --enable-parsers --enable-bsfs --enable-protocol='file,pipe' --enable-filter='buffer,buffersink,abuffer,abuffersink,null,anull,format,aformat,aresample,scale,fps,atrim,trim,asetpts,setpts,volume' --enable-encoder=wrapped_avframe --arch=x86_64 --target-os=mingw32 --extra-cflags=-I<build folder>/deps/include --extra-ldflags='-static -static-libgcc' --enable-mediafoundation --enable-d3d11va --enable-ffnvcodec --enable-nvenc --enable-amf --pkg-config=pkg-config --enable-encoder='aac,h264_mf,h264_nvenc,h264_amf,mpeg4,pcm_f32le,pcm_s16le' --enable-muxer='ipod,mp4,null,pcm_f32le' --cross-prefix=x86_64-w64-mingw32- --enable-cross-compile
  ```

- **Linux (x86_64-unknown-linux-gnu, built on Ubuntu 24.04, fully static):**

  ```
  --disable-gpl --disable-nonfree --disable-version3 --disable-autodetect --disable-everything --disable-doc --disable-debug --disable-network --disable-ffplay --disable-ffprobe --disable-avdevice --enable-demuxers --enable-decoders --enable-parsers --enable-bsfs --enable-protocol='file,pipe' --enable-filter='buffer,buffersink,abuffer,abuffersink,null,anull,format,aformat,aresample,scale,fps,atrim,trim,asetpts,setpts,volume' --enable-encoder=wrapped_avframe --enable-zlib --enable-libvpx --enable-libopus --enable-encoder='aac,libvpx_vp9,libopus,mpeg4,pcm_f32le,pcm_s16le' --enable-muxer='ipod,mp4,webm,null,pcm_f32le' --pkg-config-flags=--static --extra-ldflags=-static
  ```

### Replacing it

The LGPL lets you use a different build of FFmpeg with Dictate, and the file is separate so that you can. Dictate looks for ffmpeg in this order: the program that the `DICTATE_FFMPEG` environment variable names (a path, or a name on the search path; a value that is empty or that names a program that cannot run is ignored), then `dictate-ffmpeg` next to the main program, then `ffmpeg` on the search path. The variable leaves the installed files alone, which matters on a Mac, where a changed file inside a signed app breaks its signature. How to set it so that an app started from the desktop sees it (`launchctl setenv` on macOS, a user environment variable on Windows, the session environment on Linux) is in the [README](README.md#yt-dlp-and-ffmpeg). Nothing in the MIT license of Dictate forbids reverse engineering, for a change of this kind or any other.

## 2. What is linked into the FFmpeg that ships

The FFmpeg of each installer is made from the source above together with the libraries below. They are inside `dictate-ffmpeg` (linked statically), not separate files, so their notices and their source offers are here too.

| System | Inside the program | License |
|---|---|---|
| Linux | **glibc** 2.39 (the C library of Ubuntu 24.04, linked statically) | LGPL-2.1-or-later |
| Linux, Windows | **libgcc**, the runtime library of GCC (GCC 13 of Ubuntu 24.04; on Windows `-static-libgcc`) | GPL-3.0-or-later **with the GCC Runtime Library Exception 3.1** |
| Linux | **libvpx** (VP9 encoding; the Ubuntu 24.04 package, 1.14.0 when this was written) | BSD-3-Clause, with the WebM patent grant |
| Linux | **libopus** (Opus encoding; the Ubuntu 24.04 package, 1.4 when this was written) | BSD-3-Clause |
| macOS, Linux | **zlib** (macOS: the system's own `libz`, not shipped; Linux: the static library of Ubuntu 24.04, 1.3 when this was written) | zlib license |
| Windows | **mingw-w64** runtime (the C runtime that the Windows program is linked with) | public domain and permissive licenses (ZPL-2.1, BSD-style); the notices that its license asks for are in `THIRD_PARTY_LICENSES.txt` |
| Windows | **winpthreads** (the POSIX threads of mingw-w64) | MIT-style, with BSD-style notices for parts; its `COPYING` is in `THIRD_PARTY_LICENSES.txt` |
| Windows | **nv-codec-headers** n12.1.14.0 and the **AMF** headers v1.5.3 (header files, used to compile the hardware encoders; the program loads `nvEncodeAPI64.dll` and `amfrt64.dll` of the graphics driver at run time, and ships none of it) | MIT |
| macOS | the frameworks of the system (VideoToolbox, AudioToolbox, CoreMedia and others) | the system's, not shipped |
| Windows | Media Foundation and the other DLLs of Windows, loaded at run time | the system's, not shipped |

- **glibc and libgcc.** The Linux `dictate-ffmpeg` is fully static, so the code of glibc (LGPL-2.1-or-later) and of libgcc (GPL-3.0-or-later with the GCC Runtime Library Exception, which allows linking it into a program under another license) is in the file. The source is that of the Ubuntu 24.04 packages `glibc` (2.39) and `gcc-13` (<https://archive.ubuntu.com/ubuntu/pool/main/g/glibc/> and <https://archive.ubuntu.com/ubuntu/pool/main/g/gcc-13/>; `apt source glibc` fetches it) and, upstream, <https://ftp.gnu.org/gnu/glibc/> and <https://gcc.gnu.org/>. Because glibc is linked statically, the way to use another one is to rebuild the program: `scripts/build-ffmpeg.sh` does that from the source of FFmpeg, on Ubuntu 24.04 or any system that has a static glibc.
- **mingw-w64 and winpthreads.** The Windows `dictate-ffmpeg.exe` has the runtime of mingw-w64 and winpthreads in it (`-static -static-libgcc`). Their source is the one of the Ubuntu 24.04 packages `mingw-w64` and `gcc-mingw-w64` (<https://archive.ubuntu.com/ubuntu/pool/universe/m/mingw-w64/>) and, upstream, <https://www.mingw-w64.org>.
- **libvpx, libopus and zlib** are under BSD-style licenses that ask that their notices stay with copies of the binary: libvpx, Copyright (c) 2010, The WebM Project authors (BSD-3-Clause, with the additional IP rights grant of the WebM project); libopus, Copyright 2001-2011 Xiph.Org, Skype Limited, Octasic, Jean-Marc Valin, Timothy B. Terriberry, CSIRO, Gregory Maxwell, Mark Borgerding, Erik de Castro Lopo (BSD-3-Clause; Opus is covered by royalty-free patent licenses from the Xiph.Org Foundation, Microsoft and Broadcom, which the text points to); zlib, (C) 1995-2022 Jean-loup Gailly and Mark Adler (the zlib license). The full texts are in `THIRD_PARTY_LICENSES.txt` (part 1), and the sources are at <https://chromium.googlesource.com/webm/libvpx/>, <https://gitlab.xiph.org/xiph/opus> and <https://zlib.net>.
- **The header files** of the Windows encoders are MIT: nv-codec-headers n12.1.14.0 (Copyright (c) 2010-2023 NVIDIA Corporation, in the header files) and the AMF headers v1.5.3 (Copyright (c) 2016 Advanced Micro Devices, Inc.; its license adds that AMD gives no license to any codec standard). Their texts are in `THIRD_PARTY_LICENSES.txt` too.
- **Patents.** The AAC encoder of FFmpeg, and its decoders for formats such as MP3, AAC and AC-3, are technologies of patent pools in some countries; VP9 and Opus are royalty-free. The H.264 video is encoded by the system (VideoToolbox on macOS, Media Foundation or the driver of the graphics card on Windows), under the license of the system or of the vendor. This is not legal advice.

## 3. The other components of the app

| Component | License | What it does in Dictate, and where it is |
|---|---|---|
| [whisper.cpp](https://github.com/ggml-org/whisper.cpp) and ggml, Copyright (c) 2023-2024 The ggml authors | MIT | Recognizes speech on your computer. Built into the program through [whisper-rs](https://codeberg.org/tazz4843/whisper-rs) (Unlicense), which has whisper.cpp in its source |
| [Whisper](https://github.com/openai/whisper) models, Copyright (c) 2022 OpenAI, in the ggml format of whisper.cpp | MIT | The models that you download in Settings, from [huggingface.co/ggerganov/whisper.cpp](https://huggingface.co/ggerganov/whisper.cpp). They are not in the installers |
| [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) 1.13.8, the k2-fsa authors | Apache-2.0 (the project has no `NOTICE` file) | Separates the speakers. The prebuilt library **without text to speech** is linked into the program (a static one on macOS and Windows; `libsherpa-onnx-c-api.so` in `/usr/lib/dictate` with the `.deb`). It also contains small libraries, whose licenses its own repository lists |
| [ONNX Runtime](https://github.com/microsoft/onnxruntime) 1.28.2, Copyright (c) Microsoft Corporation | MIT (no `NOTICE` file; its `ThirdPartyNotices.txt` is in `THIRD_PARTY_LICENSES.txt`) | Runs the models. It is inside the sherpa-onnx libraries (`libonnxruntime.so` with the `.deb`) |
| pyannote segmentation 3.0 ([model](https://huggingface.co/pyannote/segmentation-3.0)), Copyright (c) 2022 CNRS | MIT | Finds who talks when. Downloaded the first time that speakers are separated (in its ONNX form from [csukuangfj/sherpa-onnx-pyannote-segmentation-3-0](https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0)); not in the installers |
| NVIDIA NeMo TitaNet-small, © NVIDIA, converted to ONNX by the sherpa-onnx project ([download](https://github.com/k2-fsa/sherpa-onnx/releases/tag/speaker-recongition-models)) | [CC-BY-4.0](https://creativecommons.org/licenses/by/4.0/) | Tells voices apart. Downloaded with the model above; not in the installers. Its attribution is below |
| [Tauri](https://tauri.app) 2, with wry, tao and its plugins, © the Tauri Programme within The Commons Conservancy | Apache-2.0 or MIT (tao: Apache-2.0) | The application framework: windows, tray, the bundler that makes the installers |
| [Svelte](https://svelte.dev) 5 and its runtime packages (clsx, esm-env), `@tauri-apps/api`, `@tauri-apps/plugin-opener` | MIT; the two Tauri packages Apache-2.0 or MIT | The interface of the windows |
| [Symphonia](https://github.com/pdeljanov/Symphonia) | **MPL-2.0** | Decodes MP3, AAC, FLAC, Vorbis and the other audio formats that Dictate reads itself. Linked into the program; see section 4 |
| Microsoft Visual C++ runtime (`vcruntime140.dll`, `vcruntime140_1.dll`, `msvcp140.dll`) | The Microsoft license terms for the Visual C++ Redistributable, which allow deployment next to the program | Windows only, next to `dictate.exe`, so that a Windows without the runtime starts the app |

**Attribution for NVIDIA NeMo TitaNet-small (CC BY 4.0).** The speaker model "TitaNet-small" (`titanet_small` of [NVIDIA NeMo](https://github.com/NVIDIA/NeMo)) is © NVIDIA and is licensed under the [Creative Commons Attribution 4.0 International license](https://creativecommons.org/licenses/by/4.0/). The change that was made to it: it was converted to the ONNX format by the sherpa-onnx project, and Dictate downloads that file (`nemo_en_titanet_small.onnx`, from <https://github.com/k2-fsa/sherpa-onnx/releases/tag/speaker-recongition-models>) and uses it as it is. NVIDIA does not endorse Dictate.

**Copyright lines of the MIT components** (their full texts are in `THIRD_PARTY_LICENSES.txt`): whisper.cpp and ggml, "Copyright (c) 2023-2024 The ggml authors"; the Whisper models and code, "Copyright (c) 2022 OpenAI"; pyannote segmentation 3.0, "Copyright (c) 2022 CNRS"; ONNX Runtime, "Copyright (c) Microsoft Corporation". The Apache-2.0 sherpa-onnx has no `NOTICE` file and neither does ONNX Runtime; where a Rust crate ships one, it is in `THIRD_PARTY_LICENSES.txt` under that crate.

Not in the installers, and so not covered by this file: the web view that the windows use (WKWebView on macOS, Microsoft Edge WebView2 on Windows, which the installer downloads from Microsoft when the system has none, and WebKitGTK on Linux, which `apt` installs with the package), the libraries of the system that the `.deb` depends on (GTK, ALSA, PipeWire and others), and [yt-dlp](https://github.com/yt-dlp/yt-dlp), which you install yourself.

**espeak-ng is not in Dictate.** The ordinary builds of sherpa-onnx have a text to speech in them, which uses espeak-ng (GPL-3.0). Dictate has no use for it, and shipping it would make the program GPL. The builds of sherpa-onnx that Dictate uses are the ones without text to speech, and CI scans every installer for it (`scripts/check-no-espeak.sh`) and stops the release if it finds a trace.

## 4. Rust crates

The program, `dictate-mcp` and the libraries they share are written in Rust and use third-party crates, linked statically.

**How this list was made.** `scripts/generate-licenses.py` asks `cargo tree -e normal --locked` for the dependencies of `dictate-app` and `dictate-mcp` on each of the four systems of the installers (`aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`): the normal dependencies, so not the ones of build scripts or of tests, and the union of the four. That is 572 crates at the `Cargo.lock` of this release. For each it takes the license files that the crate ships (`LICENSE*`, `COPYING*`, `NOTICE*`, with their copyright lines) from `cargo metadata`, and writes them to `THIRD_PARTY_LICENSES.txt`, which says in front of every text which crates use it. A crate that ships no license file (42 of them) gets the standard text of the license that it declares, with its authors. `cargo about` and `cargo deny` were tried and not used, because `cargo about` leaves out the crates that only a feature of a target brings in (the keyring backend of macOS, for one). The `license` field of each crate's manifest gives the summary below, and `app/ui/src/docs.test.ts` fails when `Cargo.lock` has changed and the file has not been written again.

| License expression (as the crate declares it) | Crates |
|---|---|
| MIT OR Apache-2.0 | 250 |
| MIT | 139 |
| Apache-2.0 OR MIT | 54 |
| MPL-2.0 | 20 |
| Zlib OR Apache-2.0 OR MIT | 20 |
| Unicode-3.0 | 18 |
| MIT/Apache-2.0 | 17 |
| Unlicense OR MIT | 7 |
| Apache-2.0 | 6 |
| Apache-2.0/MIT | 3 |
| BSD-3-Clause | 3 |
| ISC | 3 |
| (MIT OR Apache-2.0) AND Unicode-3.0 | 2 |
| Apache-2.0 OR ISC OR MIT | 2 |
| Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | 2 |
| BSD-3-Clause OR Apache-2.0 | 2 |
| BSL-1.0 | 2 |
| MIT OR Apache-2.0 OR Zlib | 2 |
| MIT OR Zlib OR Apache-2.0 | 2 |
| Unlicense | 2 |
| Unlicense/MIT | 2 |
| Zlib | 2 |
| 0BSD OR MIT OR Apache-2.0 | 1 |
| Apache-2.0 / MIT | 1 |
| Apache-2.0 AND ISC | 1 |
| Apache-2.0 AND MIT | 1 |
| Apache-2.0 OR BSL-1.0 | 1 |
| BSD-2-Clause OR Apache-2.0 OR MIT | 1 |
| BSD-2-Clause OR MIT OR Apache-2.0 | 1 |
| BSD-3-Clause AND MIT | 1 |
| BSD-3-Clause/MIT | 1 |
| CC0-1.0 OR MIT-0 OR Apache-2.0 | 1 |
| CDLA-Permissive-2.0 | 1 |
| Zlib OR MIT OR Apache-2.0 | 1 |

What the list shows:

- **No GPL, LGPL or AGPL crate** is among them.
- **Almost all are permissive:** MIT, Apache-2.0 (many as "MIT OR Apache-2.0", so either may be chosen), BSD-2-Clause and BSD-3-Clause, ISC, Zlib, Unlicense, 0BSD, BSL-1.0, CC0-1.0 and Unicode-3.0 (the `icu_*` crates and their data, which ask that the [Unicode license](https://www.unicode.org/license.txt) goes with copies) and CDLA-Permissive-2.0 (`webpki-roots`, the list of root certificates of Mozilla).
- **Apache-2.0 only** (no MIT choice): `cpal` (audio input), `gethostname`, `hound` (WAV files), `sherpa-onnx-sys` (the binding to sherpa-onnx), `sync_wrapper` and `tao`.
- **Unlicense** (public domain): `whisper-rs` and `whisper-rs-sys`, the Rust binding to whisper.cpp, which is MIT.
- **MPL-2.0** (file-level copyleft): 20 crates, with the source that the license asks to be available at their repositories, [github.com/pdeljanov/Symphonia](https://github.com/pdeljanov/Symphonia) (Symphonia: `symphonia` and its 14 codec, format and other crates), [github.com/servo/rust-cssparser](https://github.com/servo/rust-cssparser) (`cssparser`, `cssparser-macros`), [github.com/servo/stylo](https://github.com/servo/stylo) (`selectors`), [github.com/upsuper/dtoa-short](https://github.com/upsuper/dtoa-short) and [github.com/soc/option-ext](https://github.com/soc/option-ext). Dictate links them unmodified, and the exact source of each version is on <https://crates.io> (`cargo vendor` fetches it from the `Cargo.lock` of this repository). The text of the MPL 2.0 is in `THIRD_PARTY_LICENSES.txt` and at <https://www.mozilla.org/MPL/2.0/>. The part of the program that is Dictate's own stays MIT.
- **Mixed licenses:** `ring` (declared "Apache-2.0 AND ISC": its own license file is in `THIRD_PARTY_LICENSES.txt`), `brotli` ("BSD-3-Clause AND MIT"), `unicode-ident` and `unicode-normalization` ("(MIT OR Apache-2.0) AND Unicode-3.0").

## 5. npm packages

The interface in `app/ui` is built with Vite from Svelte and two packages of Tauri. What ends up in the program is the runtime of Svelte 5 (MIT) with the two small packages it uses (`clsx` and `esm-env`, both MIT), `@tauri-apps/api` (Apache-2.0 OR MIT) and `@tauri-apps/plugin-opener` (MIT OR Apache-2.0; it ships only an SPDX file, so `THIRD_PARTY_LICENSES.txt` has the license files of `@tauri-apps/api`, the same project, for it). Their license files are in `THIRD_PARTY_LICENSES.txt` (part 3), written by the same script.

`pnpm licenses list --json`, run in `app/ui`, lists the 116 packages of the workspace that the lock file has, tools for building and testing included: MIT (99), Apache-2.0 (6), BSD-2-Clause (2), BSD-3-Clause (2), ISC (2), MPL-2.0 (2), "Apache-2.0 OR MIT" (1), "MIT OR Apache-2.0" (1) and BlueOak-1.0.0 (1). The tools (Vite, Vitest, TypeScript, jsdom and the like) are not in the installers. The two packages under MPL-2.0 (`lightningcss` and its platform file), which Vite uses to process CSS, and the one under BlueOak-1.0.0 (`lru-cache`, a permissive license) are among them.
