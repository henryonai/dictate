# Third-party notices

Dictate is © 2026 Henry Hoang, under the [MIT License](LICENSE). It includes, uses or downloads the software below, each under its own license. This file is attached to every [release](https://github.com/henryonai/dictate/releases), and Settings → General → **Licenses** opens it.

What is in this file:

1. [FFmpeg](#1-ffmpeg-lgpl-21-or-later): the license, the written offer of its source, how it is built and how to replace it.
2. [What is linked into the FFmpeg that ships](#2-what-is-linked-into-the-ffmpeg-that-ships): glibc, libgcc, libvpx, libopus, zlib, mingw-w64, winpthreads and the encoder headers.
3. [The other components of the app](#3-the-other-components-of-the-app): whisper.cpp, the models, sherpa-onnx, ONNX Runtime, Tauri and the rest.
4. [Rust crates](#4-rust-crates) and [npm packages](#5-npm-packages): the licenses of the libraries that the program is built from, and how this list was made.

## 1. FFmpeg (LGPL 2.1 or later)

The installers of Dictate carry a build of [FFmpeg](https://ffmpeg.org) 9.0.2. Dictate starts it as a program of its own, `dictate-ffmpeg`, next to the main program (`Dictate.app/Contents/MacOS/dictate-ffmpeg` on macOS, `dictate-ffmpeg.exe` in the install folder on Windows, `/usr/bin/dictate-ffmpeg` with the `.deb`), and does not link its libraries.

- **License:** the GNU Lesser General Public License, version 2.1 or later (LGPL v2.1+). The text is at <https://www.gnu.org/licenses/old-licenses/lgpl-2.1.html> and in the source of FFmpeg as `COPYING.LGPLv2.1`. The build is made with `--disable-gpl --disable-nonfree --disable-version3`, and its `configure` has to print `License: LGPL version 2.1 or later` (`scripts/build-ffmpeg.sh` stops otherwise); CI checks every `dictate-ffmpeg` that goes into an installer (`scripts/check-ffmpeg-lgpl.sh`). FFmpeg is a trademark of Fabrice Bellard, originator of the FFmpeg project.
- **What it is for:** reading the audio and video formats that Dictate does not decode itself, encoding the video and the sound of a recording, and giving yt-dlp a program to join and convert what it downloads.

### Written offer of the source

Dictate includes FFmpeg (<https://ffmpeg.org>), licensed under the GNU LGPL v2.1 or later. The corresponding source of the FFmpeg that is in a release, the build script and the configure lines are at <https://github.com/henryonai/dictate/releases>:

- **The source:** the exact, unmodified `ffmpeg-9.0.2.tar.xz` (we apply no patches), attached to every release. Its SHA-256 is `8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e`, and it is the same file as <https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz>.
- **The configure lines:** `BUILDCONF-aarch64-apple-darwin.txt`, `BUILDCONF-x86_64-apple-darwin.txt`, `BUILDCONF-x86_64-pc-windows-msvc.txt` and `BUILDCONF-x86_64-unknown-linux-gnu.txt`, attached to every release: each is the configure line of the program of that installer (the output of `ffmpeg -buildconf`). The lines are copied below.
- **The build script:** `scripts/build-ffmpeg.sh` in this repository, at the tag of the release. It downloads the source, checks the SHA-256 above before it unpacks anything, runs `configure` with the lines below and builds the program for one target. Anyone can run it to make the same program, or change it to link another library.

This offer is valid for at least three years after the release that it comes with, and for as long as that release stays on the page. If something above is missing there, write to <henryhoang.tech@gmail.com> or open an [issue](https://github.com/henryonai/dictate/issues), and the source of FFmpeg, or of any other library named in section 2 that is inside `dictate-ffmpeg`, will be sent to you.

### How it is configured

Every target starts from the same lines, which turn everything off (`--disable-everything`) and then turn on every demuxer, decoder, parser and bitstream filter (an import can need any of them) and only what the app asks of the rest:

```
--disable-gpl --disable-nonfree --disable-version3 --disable-autodetect --disable-everything --disable-doc --disable-debug --disable-network --disable-ffplay --disable-ffprobe --disable-avdevice --enable-demuxers --enable-decoders --enable-parsers --enable-bsfs --enable-protocol='file,pipe' --enable-filter='buffer,buffersink,abuffer,abuffersink,null,anull,format,aformat,aresample,scale,fps,atrim,trim,asetpts,setpts,volume' --enable-encoder=wrapped_avframe
```

And then, for each target (the `BUILDCONF` files have the whole line):

- **macOS, Apple silicon** (`aarch64-apple-darwin`): `--cc='clang -arch arm64' --arch=arm64 --target-os=darwin --extra-cflags='-mmacosx-version-min=13.0' --extra-ldflags='-mmacosx-version-min=13.0' --enable-zlib --enable-videotoolbox --enable-audiotoolbox --enable-encoder='aac,h264_videotoolbox,mpeg4,pcm_f32le,pcm_s16le' --enable-muxer='ipod,mp4,null,pcm_f32le'`
- **macOS, Intel** (`x86_64-apple-darwin`, cross-built on an Apple silicon Mac): the same, with `--cc='clang -arch x86_64' --arch=x86_64` and `--enable-cross-compile`.
- **Windows** (`x86_64-pc-windows-msvc`, built on Linux with mingw-w64): `--arch=x86_64 --target-os=mingw32 --extra-ldflags='-static -static-libgcc' --enable-mediafoundation --enable-d3d11va --enable-ffnvcodec --enable-nvenc --enable-amf --pkg-config=pkg-config --enable-encoder='aac,h264_mf,h264_nvenc,h264_amf,mpeg4,pcm_f32le,pcm_s16le' --enable-muxer='ipod,mp4,null,pcm_f32le' --cross-prefix=x86_64-w64-mingw32- --enable-cross-compile`
- **Linux** (`x86_64-unknown-linux-gnu`, built on Ubuntu 24.04, fully static): `--enable-zlib --enable-libvpx --enable-libopus --enable-encoder='aac,libvpx_vp9,libopus,mpeg4,pcm_f32le,pcm_s16le' --enable-muxer='ipod,mp4,webm,null,pcm_f32le' --pkg-config-flags=--static --extra-ldflags=-static`

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
| Windows | **mingw-w64** runtime (the C runtime that the Windows program is linked with) | public domain and permissive licenses (ZPL-2.1, BSD-style); its `COPYING` files are in the mingw-w64 source |
| Windows | **winpthreads** (the POSIX threads of mingw-w64) | MIT-style, with BSD-style notices for parts; its `COPYING` is in the mingw-w64 source |
| Windows | **nv-codec-headers** n12.1.14.0 and the **AMF** headers v1.5.3 (header files, used to compile the hardware encoders; the program loads `nvEncodeAPI64.dll` and `amfrt64.dll` of the graphics driver at run time, and ships none of it) | MIT |
| macOS | the frameworks of the system (VideoToolbox, AudioToolbox, CoreMedia and others) | the system's, not shipped |
| Windows | Media Foundation and the other DLLs of Windows, loaded at run time | the system's, not shipped |

- **glibc and libgcc.** The Linux `dictate-ffmpeg` is fully static, so the code of glibc (LGPL-2.1-or-later) and of libgcc (GPL-3.0-or-later with the GCC Runtime Library Exception, which allows linking it into a program under another license) is in the file. The source is that of the Ubuntu 24.04 packages `glibc` (2.39) and `gcc-13` (<https://archive.ubuntu.com/ubuntu/pool/main/g/glibc/> and <https://archive.ubuntu.com/ubuntu/pool/main/g/gcc-13/>; `apt source glibc` fetches it) and, upstream, <https://ftp.gnu.org/gnu/glibc/> and <https://gcc.gnu.org/>. Because glibc is linked statically, the way to use another one is to rebuild the program: `scripts/build-ffmpeg.sh` does that from the source of FFmpeg, on Ubuntu 24.04 or any system that has a static glibc.
- **mingw-w64 and winpthreads.** The Windows `dictate-ffmpeg.exe` has the runtime of mingw-w64 and winpthreads in it (`-static -static-libgcc`). Their source is the one of the Ubuntu 24.04 packages `mingw-w64` and `gcc-mingw-w64` (<https://archive.ubuntu.com/ubuntu/pool/universe/m/mingw-w64/>) and, upstream, <https://www.mingw-w64.org>.
- **libvpx, libopus and zlib** are under BSD-style licenses that ask that their notices stay with copies of the binary: libvpx © The WebM Project authors; libopus © the Xiph.Org Foundation and the other contributors of the Opus project (its `COPYING` names them); zlib © Jean-loup Gailly and Mark Adler. The full texts are in their sources: <https://chromium.googlesource.com/webm/libvpx/>, <https://gitlab.xiph.org/xiph/opus> and <https://zlib.net>. Opus is covered by royalty-free patent licenses from the Xiph.Org Foundation, Microsoft and Broadcom, which its `COPYING` points to.
- **Patents.** The AAC encoder of FFmpeg, and its decoders for formats such as MP3, AAC and AC-3, are technologies of patent pools in some countries; VP9 and Opus are royalty-free. The H.264 video is encoded by the system (VideoToolbox on macOS, Media Foundation or the driver of the graphics card on Windows), under the license of the system or of the vendor. This is not legal advice.

## 3. The other components of the app

| Component | License | What it does in Dictate, and where it is |
|---|---|---|
| [whisper.cpp](https://github.com/ggml-org/whisper.cpp) and ggml, © The ggml authors | MIT | Recognizes speech on your computer. Built into the program through [whisper-rs](https://codeberg.org/tazz4843/whisper-rs) (Unlicense) |
| [Whisper](https://github.com/openai/whisper) models, © 2022 OpenAI, in the ggml format of whisper.cpp | MIT | The models that you download in Settings, from [huggingface.co/ggerganov/whisper.cpp](https://huggingface.co/ggerganov/whisper.cpp). They are not in the installers |
| [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) 1.13.8, © the k2-fsa authors | Apache-2.0 | Separates the speakers. The prebuilt library **without text to speech** is linked into the program (a static one on macOS and Windows; `libsherpa-onnx-c-api.so` in `/usr/lib/dictate` with the `.deb`). It also contains small libraries, whose licenses its own repository lists |
| [ONNX Runtime](https://github.com/microsoft/onnxruntime), © Microsoft | MIT | Runs the models. It is inside the sherpa-onnx libraries (`libonnxruntime.so` with the `.deb`); its own repository lists the notices of what it contains |
| pyannote segmentation 3.0 ([model](https://huggingface.co/pyannote/segmentation-3.0)), © CNRS | MIT | Finds who talks when. Downloaded the first time that speakers are separated (in its ONNX form from [csukuangfj/sherpa-onnx-pyannote-segmentation-3-0](https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0)); not in the installers |
| NVIDIA NeMo TitaNet-small, © NVIDIA, converted to ONNX by the sherpa-onnx project ([download](https://github.com/k2-fsa/sherpa-onnx/releases/tag/speaker-recongition-models)) | [CC-BY-4.0](https://creativecommons.org/licenses/by/4.0/) | Tells voices apart. Downloaded with the model above; not in the installers. It was changed from its original format to ONNX |
| [Tauri](https://tauri.app) 2, with wry, tao and its plugins, © the Tauri Programme within The Commons Conservancy | Apache-2.0 or MIT (tao: Apache-2.0) | The application framework: windows, tray, the bundler that makes the installers |
| [Svelte](https://svelte.dev) 5 and its runtime packages (clsx, esm-env), `@tauri-apps/api`, `@tauri-apps/plugin-opener` | MIT; the two Tauri packages Apache-2.0 or MIT | The interface of the windows |
| [Symphonia](https://github.com/pdeljanov/Symphonia) | **MPL-2.0** | Decodes MP3, AAC, FLAC, Vorbis and the other audio formats that Dictate reads itself. Linked into the program; see section 4 |
| Microsoft Visual C++ runtime (`vcruntime140.dll`, `vcruntime140_1.dll`, `msvcp140.dll`) | The Microsoft license terms for the Visual C++ Redistributable, which allow deployment next to the program | Windows only, next to `dictate.exe`, so that a Windows without the runtime starts the app |

Not in the installers, and so not covered by this file: the web view that the windows use (WKWebView on macOS, Microsoft Edge WebView2 on Windows, which the installer downloads from Microsoft when the system has none, and WebKitGTK on Linux, which `apt` installs with the package), the libraries of the system that the `.deb` depends on (GTK, ALSA, PipeWire and others), and [yt-dlp](https://github.com/yt-dlp/yt-dlp), which you install yourself.

**espeak-ng is not in Dictate.** The ordinary builds of sherpa-onnx have a text to speech in them, which uses espeak-ng (GPL-3.0). Dictate has no use for it, and shipping it would make the program GPL. The builds of sherpa-onnx that Dictate uses are the ones without text to speech, and CI scans every installer for it (`scripts/check-no-espeak.sh`) and stops the release if it finds a trace.

## 4. Rust crates

The program, `dictate-mcp` and the libraries they share are written in Rust and use third-party crates, linked statically.

**How this list was made.** No license tool (`cargo about`, `cargo deny`) was used. The list is the `license` field of each crate's manifest, read with `cargo metadata --format-version 1 --locked --filter-platform <target>` for each of the four targets of the release (`aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`), following only the normal dependencies (not the ones for build scripts or for tests) of `dictate-app` and `dictate-mcp`, and taking the union of the four. That is 602 crates at the `Cargo.lock` of this release; the appendix at the end has all of them with their versions. The licenses are the ones the crates declare, and `Cargo.lock` has the exact versions, so run the same command to make the list again for a later version.

| License expression (as the crate declares it) | Crates |
|---|---|
| MIT OR Apache-2.0 | 263 |
| MIT | 144 |
| Apache-2.0 OR MIT | 57 |
| Zlib OR Apache-2.0 OR MIT | 23 |
| MPL-2.0 | 20 |
| MIT/Apache-2.0 | 18 |
| Unicode-3.0 | 18 |
| Unlicense OR MIT | 7 |
| Apache-2.0 | 6 |
| MIT OR Apache-2.0 OR Zlib | 5 |
| Apache-2.0/MIT | 3 |
| BSD-3-Clause | 3 |
| ISC | 3 |
| Zlib | 3 |
| (MIT OR Apache-2.0) AND Unicode-3.0 | 2 |
| Apache-2.0 OR ISC OR MIT | 2 |
| Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | 2 |
| BSD-2-Clause OR Apache-2.0 OR MIT | 2 |
| BSD-3-Clause OR Apache-2.0 | 2 |
| BSL-1.0 | 2 |
| MIT OR Zlib OR Apache-2.0 | 2 |
| Unlicense | 2 |
| Unlicense/MIT | 2 |
| 0BSD OR MIT OR Apache-2.0 | 1 |
| Apache-2.0 / MIT | 1 |
| Apache-2.0 AND ISC | 1 |
| Apache-2.0 AND MIT | 1 |
| Apache-2.0 OR BSL-1.0 | 1 |
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
- **MPL-2.0** (file-level copyleft): 20 crates, with the source that the license asks to be available at their repositories, [github.com/pdeljanov/Symphonia](https://github.com/pdeljanov/Symphonia) (Symphonia: `symphonia` and its 14 codec, format and other crates), [github.com/servo/rust-cssparser](https://github.com/servo/rust-cssparser) (`cssparser`, `cssparser-macros`), [github.com/servo/stylo](https://github.com/servo/stylo) (`selectors`), [github.com/upsuper/dtoa-short](https://github.com/upsuper/dtoa-short) and [github.com/soc/option-ext](https://github.com/soc/option-ext). Dictate links them unmodified, and the exact source of each version is on <https://crates.io> (`cargo vendor` fetches it from the `Cargo.lock` of this repository). The text of the MPL 2.0 is at <https://www.mozilla.org/MPL/2.0/>. The part of the program that is Dictate's own stays MIT.
- **Mixed licenses:** `ring` (declared "Apache-2.0 AND ISC": its own license file, <https://github.com/briansmith/ring/blob/main/LICENSE>, has the details), `brotli` ("BSD-3-Clause AND MIT"), `unicode-ident` and `unicode-normalization` ("(MIT OR Apache-2.0) AND Unicode-3.0").

## 5. npm packages

The interface in `app/ui` is built with Vite from Svelte and two packages of Tauri. What ends up in the program is the runtime of Svelte 5 (MIT) with the two small packages it uses (`clsx` and `esm-env`, both MIT), `@tauri-apps/api` (Apache-2.0 OR MIT) and `@tauri-apps/plugin-opener` (MIT OR Apache-2.0).

**How this list was made.** `pnpm licenses list --json`, run in `app/ui`, lists the 116 packages of the workspace that the lock file has, tools for building and testing included: MIT (99), Apache-2.0 (6), BSD-2-Clause (2), BSD-3-Clause (2), ISC (2), MPL-2.0 (2), "Apache-2.0 OR MIT" (1), "MIT OR Apache-2.0" (1) and BlueOak-1.0.0 (1). The tools (Vite, Vitest, TypeScript, jsdom and the like) are not in the installers. The two packages under MPL-2.0 (`lightningcss` and its platform file), which Vite uses to process CSS, and the one under BlueOak-1.0.0 (`lru-cache`, a permissive license) are among them.

## Appendix: the Rust crates, by license

The 602 crates of section 4, with their versions, grouped by the license that each declares.

**MIT OR Apache-2.0** (263): aes 0.8.4, anstream 1.0.0, anstyle 1.0.14, anstyle-parse 1.0.0, anstyle-query 1.1.5, anstyle-wincon 3.0.11, anyhow 1.0.104, apple-cf 0.11.7, apple-metal 0.10.4, arboard 3.7.0, async-broadcast 0.7.2, async-trait 0.1.92, audio-core 0.2.1, audioadapter 4.0.0, audioadapter-buffers 4.0.0, audioadapter-sample 4.0.0, base64 0.21.7, base64 0.22.1, base64 0.23.1, bitflags 2.13.2, block-buffer 0.10.4, block-padding 0.3.3, camino 1.2.6, cargo-platform 0.1.9, cbc 0.1.2, cfg-if 1.0.5, chacha20 0.10.2, chrono 0.4.45, chunked_transfer 1.5.0, cipher 0.4.4, colorchoice 1.0.5, cookie 0.18.2, core-foundation 0.10.1, core-foundation-sys 0.8.7, core-graphics 0.25.0, core-graphics-types 0.2.0, cpufeatures 0.2.17, cpufeatures 0.3.1, crc32fast 1.5.4, crossbeam-channel 0.5.17, crossbeam-deque 0.8.8, crossbeam-epoch 0.9.21, crossbeam-utils 0.8.23, crypto-common 0.1.7, dasp_sample 0.11.0, dbus-secret-service 4.1.0, deranged 0.5.9, digest 0.10.7, dirs 4.0.0, dirs 6.0.0, dirs 7.0.0, dirs-sys 0.3.7, dirs-sys 0.5.0, displaydoc 0.2.7, doom-fish-utils 0.4.3, dtoa 1.0.11, dyn-clone 1.0.20, either 1.19.0, embed_plist 1.2.2, enumflags2 0.7.12, enumflags2_derive 0.7.12, env_filter 2.0.0, env_logger 0.11.11, erased-serde 0.4.11, errno 0.3.14, fdeflate 0.3.7, field-offset 0.3.6, fixedbitset 0.5.7, flate2 1.1.11, form_urlencoded 1.2.2, futures 0.3.34, futures-channel 0.3.34, futures-core 0.3.34, futures-executor 0.3.34, futures-io 0.3.34, futures-macro 0.3.34, futures-sink 0.3.34, futures-task 0.3.34, futures-util 0.3.34, getrandom 0.2.17, getrandom 0.3.4, getrandom 0.4.3, glob 0.3.4, half 2.7.1, hashbrown 0.12.3, hashbrown 0.15.5, hashbrown 0.17.1, heck 0.4.1, heck 0.5.0, hex 0.4.3, hkdf 0.12.4, hmac 0.12.1, html5ever 0.38.0, http 1.5.0, httparse 1.10.1, httpdate 1.0.3, iana-time-zone 0.1.65, idna 1.1.0, image 0.25.9, inout 0.1.4, ipnet 2.12.2, is_terminal_polyfill 1.70.2, itoa 1.0.18, jsonptr 0.6.3, keyboard-types 0.7.0, keyring 3.6.3, lazy_static 1.5.1, libc 0.2.191, lock_api 0.4.14, log 0.4.35, markup5ever 0.38.0, memmap2 0.9.12, mime 0.3.17, num 0.4.3, num-bigint 0.4.8, num-complex 0.4.6, num-conv 0.1.0, num-integer 0.1.47, num-iter 0.1.46, num-rational 0.4.2, num-traits 0.2.19, once_cell 1.21.4, once_cell_polyfill 1.70.2, opus-decoder 0.1.1, ordered-stream 0.2.0, parking_lot 0.12.5, parking_lot_core 0.9.12, pastey 0.2.3, percent-encoding 2.3.2, petgraph 0.8.3, piper 0.2.5, png 0.17.16, png 0.18.1, powerfmt 0.2.1, ppv-lite86 0.2.21, primal-check 0.3.4, proc-macro-crate 1.3.1, proc-macro-crate 2.0.2, proc-macro-crate 3.5.0, proc-macro-error 1.0.4, proc-macro-error-attr 1.0.4, proc-macro2 1.0.107, quinn 0.11.12, quinn-proto 0.11.19, quinn-udp 0.5.16, quote 1.0.47, rand 0.10.3, rand 0.8.8, rand 0.9.5, rand_chacha 0.3.1, rand_chacha 0.9.0, rand_core 0.10.1, rand_core 0.6.4, rand_core 0.9.5, rand_pcg 0.10.2, rayon 1.12.0, rayon-core 1.13.0, ref-cast 1.0.27, ref-cast-impl 1.0.27, regex 1.13.1, regex-automata 0.4.18, regex-lite 0.1.9, regex-syntax 0.8.11, reqwest 0.12.28, rubato 4.0.0, rustfft 6.4.1, rustls-pki-types 1.15.1, scopeguard 1.2.0, screencapturekit 11.1.2, secret-service 4.0.0, security-framework 3.7.0, security-framework-sys 2.17.0, semver 1.0.28, serde 1.0.229, serde-untagged 0.1.9, serde_core 1.0.229, serde_derive 1.0.229, serde_derive_internals 0.29.1, serde_json 1.0.154, serde_repr 0.1.21, serde_spanned 0.6.9, serde_spanned 1.1.3, serde_with 3.17.0, serde_with_macros 3.17.0, serialize-to-javascript 0.1.2, serialize-to-javascript-impl 0.1.2, servo_arc 0.4.3, sha1 0.10.7, sha2 0.10.9, signal-hook-registry 1.4.8, siphasher 1.0.4, smallvec 1.16.5, socket2 0.6.5, softbuffer 0.4.8, stable_deref_trait 1.2.1, static_assertions 1.1.0, strength_reduce 0.2.4, string_cache 0.9.0, swift-rs 1.0.9, syn 1.0.109, syn 2.0.119, syn 3.0.9, sys-locale 0.3.2, tauri-nspanel 2.1.1, tauri-winrt-notification 0.7.3, tendril 0.5.1, thiserror 1.0.69, thiserror 2.0.21, thiserror-impl 1.0.69, thiserror-impl 2.0.21, time 0.3.45, time-core 0.1.7, time-macros 0.2.25, tiny_http 0.12.0, tokio-rustls 0.26.6, toml 1.2.1+spec-1.1.0, toml_datetime 0.6.3, toml_datetime 1.1.3+spec-1.1.0, toml_edit 0.19.15, toml_edit 0.20.2, toml_edit 0.25.19+spec-1.1.0, toml_parser 1.1.5+spec-1.1.0, toml_writer 1.2.0+spec-1.1.0, transpose 0.2.3, tray-icon 0.24.2, typeid 1.0.3, typenum 1.20.1, unicase 2.10.0, unicode-segmentation 1.13.3, url 2.5.8, web_atoms 0.2.6, weezl 0.1.12, windows 0.61.3, windows 0.62.2, windows-collections 0.2.0, windows-collections 0.3.2, windows-core 0.61.2, windows-core 0.62.2, windows-future 0.2.1, windows-future 0.3.2, windows-implement 0.60.2, windows-interface 0.59.3, windows-link 0.1.3, windows-link 0.2.1, windows-numerics 0.2.0, windows-numerics 0.3.1, windows-result 0.3.4, windows-result 0.4.1, windows-strings 0.4.2, windows-strings 0.5.1, windows-sys 0.59.0, windows-sys 0.60.2, windows-sys 0.61.2, windows-targets 0.52.6, windows-targets 0.53.5, windows-threading 0.1.0, windows-threading 0.2.1, windows-version 0.1.7, windows_x86_64_msvc 0.52.6, windows_x86_64_msvc 0.53.1, wl-clipboard-rs 0.9.5, x11rb 0.13.2, x11rb-protocol 0.13.2

**MIT** (144): alsa-sys 0.4.0, ashpd 0.12.3, atk 0.18.2, atk-sys 0.18.2, auto-launch 0.5.0, block2 0.6.2, bytes 1.12.1, cairo-rs 0.18.5, cairo-sys-rs 0.18.2, cargo_metadata 0.19.2, cfb 0.7.3, cookie-factory 0.3.3, darling 0.21.3, darling_core 0.21.3, darling_macro 0.21.3, derive_more 2.1.1, derive_more-impl 2.1.1, dlopen2 0.8.2, dlopen2_derive 0.4.3, dom_query 0.27.0, endi 1.1.1, enigo 0.6.1, extended 0.1.0, fancy-regex 0.20.0, fax 0.2.7, gdk 0.18.2, gdk-pixbuf 0.18.5, gdk-pixbuf-sys 0.18.0, gdk-sys 0.18.2, gdkwayland-sys 0.18.2, gdkx11 0.18.2, gdkx11-sys 0.18.2, generic-array 0.14.7, gio 0.18.4, gio-sys 0.18.1, glib 0.18.5, glib-macros 0.18.5, glib-sys 0.18.1, gobject-sys 0.18.0, gtk 0.18.2, gtk-sys 0.18.2, gtk3-macros 0.18.2, h2 0.4.20, http-body 1.1.0, http-body-util 0.1.5, hyper 1.12.0, hyper-util 0.1.21, ico 0.5.0, infer 0.19.0, is-docker 0.2.0, is-wsl 0.4.0, javascriptcore-rs 1.1.2, javascriptcore-rs-sys 1.1.1, libspa 0.10.1, libspa-sys 0.10.1, memoffset 0.9.1, mime_guess 2.0.5, mio 1.2.4, new_debug_unreachable 1.0.6, nix 0.29.0, nix 0.30.1, nom 8.0.0, objc2 0.6.5, objc2-encode 4.1.0, objc2-foundation 0.3.2, open 5.4.5, os_pipe 1.2.3, pango 0.18.3, pango-sys 0.18.0, phf 0.13.1, phf_generator 0.13.1, phf_macros 0.13.1, phf_shared 0.13.1, pipewire 0.10.1, pipewire-sys 0.10.1, plist 1.8.0, precomputed-hash 0.1.1, quick-xml 0.38.4, quick-xml 0.41.0, realfft 3.5.1, rfd 0.16.1, schemars 0.8.22, schemars 0.9.0, schemars 1.2.3, schemars_derive 0.8.22, simd-adler32 0.3.10, slab 0.4.12, soup3 0.5.0, soup3-sys 0.5.0, strsim 0.11.1, synstructure 0.14.0, sysinfo 0.36.1, tiff 0.10.3, tokio 1.54.1, tokio-macros 2.7.2, tokio-util 0.7.20, tower 0.5.3, tower-http 0.6.11, tower-layer 0.3.3, tower-service 0.3.3, tracing 0.1.44, tracing-attributes 0.1.31, tracing-core 0.1.36, trash 5.2.9, tree_magic_mini 3.2.2, try-lock 0.2.5, urlencoding 2.1.3, urlpattern 0.3.0, want 0.3.2, wayland-backend 0.3.17, wayland-client 0.31.15, wayland-protocols 0.32.12, wayland-protocols-wlr 0.3.12, wayland-scanner 0.31.11, wayland-sys 0.31.11, webkit2gtk 2.0.2, webkit2gtk-sys 2.0.2, webview2-com 0.38.2, webview2-com-macros 0.8.1, webview2-com-sys 0.38.2, which 8.0.6, windowfunctions 0.1.1, windows-capture 1.5.0, winnow 0.5.40, winnow 0.7.15, winnow 1.0.5, winreg 0.10.1, x11 2.21.0, x11-dl 2.21.0, xdg-home 1.3.0, xkbcommon 0.9.0, zbus 4.4.0, zbus 5.13.2, zbus_macros 4.4.0, zbus_macros 5.13.2, zbus_names 3.0.0, zbus_names 4.3.1, zmij 1.0.23, zvariant 4.2.0, zvariant 5.9.2, zvariant_derive 4.2.0, zvariant_derive 5.9.2, zvariant_utils 2.1.0, zvariant_utils 3.3.0

**Apache-2.0 OR MIT** (57): ascii 1.1.0, async-channel 2.5.0, async-executor 1.14.1, async-fs 2.2.0, async-io 2.6.0, async-lock 3.5.0, async-process 2.5.0, async-signal 0.2.14, async-task 4.7.1, atomic-waker 1.1.2, bit-set 0.8.0, bit-vec 0.8.0, blocking 1.7.0, concurrent-queue 2.5.0, ctor 0.8.0, ctor-proc-macro 0.0.7, dtor 0.3.0, dtor-proc-macro 0.0.6, equivalent 1.0.3, event-listener 5.4.2, event-listener-strategy 0.5.4, fastrand 2.5.0, futures-lite 2.6.1, global-hotkey 0.8.0, idna_adapter 1.2.1, indexmap 1.9.3, indexmap 2.14.2, libappindicator 0.9.0, libappindicator-sys 0.9.0, muda 0.19.3, ntapi 0.4.3, parking 2.2.1, pin-project-lite 0.2.17, polling 3.11.0, process-wrap 8.2.2, rustc-hash 2.1.3, tauri 2.11.6, tauri-codegen 2.6.3, tauri-macros 2.6.3, tauri-plugin-autostart 2.5.1, tauri-plugin-dialog 2.7.3, tauri-plugin-fs 2.5.2, tauri-plugin-global-shortcut 2.3.2, tauri-plugin-notification 2.4.0, tauri-plugin-opener 2.5.5, tauri-plugin-single-instance 2.4.5, tauri-runtime 2.11.3, tauri-runtime-wry 2.11.4, tauri-utils 2.9.3, utf8_iter 1.1.0, utf8parse 0.2.2, utf_types 1.0.0, uuid 1.26.1, window-vibrancy 0.6.0, wry 0.55.1, zeroize 1.9.1, zeroize_derive 1.5.0

**Zlib OR Apache-2.0 OR MIT** (23): bytemuck 1.25.2, dispatch2 0.3.1, objc2-app-kit 0.3.2, objc2-application-services 0.3.2, objc2-audio-toolbox 0.3.2, objc2-av-foundation 0.3.2, objc2-cloud-kit 0.3.2, objc2-core-audio 0.3.2, objc2-core-audio-types 0.3.2, objc2-core-data 0.3.2, objc2-core-foundation 0.3.2, objc2-core-graphics 0.3.2, objc2-core-image 0.3.2, objc2-core-services 0.3.2, objc2-core-text 0.3.2, objc2-core-video 0.3.2, objc2-exception-helper 0.1.1, objc2-io-kit 0.3.2, objc2-io-surface 0.3.2, objc2-quartz-core 0.3.2, objc2-security 0.3.2, objc2-web-kit 0.3.2, tinyvec 1.13.3

**MPL-2.0** (20): cssparser 0.36.0, cssparser-macros 0.6.1, dtoa-short 0.3.5, option-ext 0.2.0, selectors 0.36.1, symphonia 0.6.1, symphonia-bundle-flac 0.6.1, symphonia-bundle-mp3 0.6.1, symphonia-codec-aac 0.6.1, symphonia-codec-alac 0.6.1, symphonia-codec-pcm 0.6.1, symphonia-codec-vorbis 0.6.1, symphonia-common 0.6.1, symphonia-core 0.6.1, symphonia-format-caf 0.6.1, symphonia-format-isomp4 0.6.1, symphonia-format-mkv 0.6.1, symphonia-format-ogg 0.6.1, symphonia-format-riff 0.6.1, symphonia-metadata 0.6.1

**MIT/Apache-2.0** (18): bitflags 1.3.2, coreaudio-rs 0.14.2, downcast-rs 1.2.1, foreign-types 0.5.0, foreign-types-macros 0.2.4, foreign-types-shared 0.3.1, ident_case 1.0.1, json-patch 3.0.1, mac-notification-sys 0.6.15, notify-rust 4.17.0, quick-error 2.0.1, serde_urlencoded 0.7.1, unic-char-property 0.9.0, unic-char-range 0.9.0, unic-common 0.9.0, unic-ucd-ident 0.9.0, unic-ucd-version 0.9.0, winapi 0.3.9

**Unicode-3.0** (18): icu_collections 2.1.1, icu_locale_core 2.1.1, icu_normalizer 2.1.1, icu_normalizer_data 2.1.1, icu_properties 2.1.2, icu_properties_data 2.1.2, icu_provider 2.1.1, litemap 0.8.3, potential_utf 0.1.6, tinystr 0.8.5, writeable 0.6.4, yoke 0.8.4, yoke-derive 0.8.4, zerofrom 0.1.8, zerofrom-derive 0.1.8, zerotrie 0.2.5, zerovec 0.11.9, zerovec-derive 0.11.7

**Unlicense OR MIT** (7): aho-corasick 1.1.5, byteorder 1.5.0, byteorder-lite 0.1.0, jiff 0.2.40, jiff-core 0.1.3, memchr 2.8.3, winapi-util 0.1.11

**Apache-2.0** (6): cpal 0.18.2, gethostname 1.1.0, hound 3.5.1, sherpa-onnx-sys 1.13.8, sync_wrapper 1.0.2, tao 0.35.3

**MIT OR Apache-2.0 OR Zlib** (5): lru-slab 0.1.3, raw-window-handle 0.6.2, xkeysym 0.2.1, zune-core 0.4.12, zune-jpeg 0.4.21

**Apache-2.0/MIT** (3): alsa 0.11.0, dbus 0.9.12, libdbus-sys 0.2.7

**BSD-3-Clause** (3): alloc-no-stdlib 2.0.4, alloc-stdlib 0.2.4, subtle 2.6.1

**ISC** (3): libloading 0.7.4, rustls-webpki 0.103.16, untrusted 0.9.0

**Zlib** (3): foldhash 0.1.5, foldhash 0.2.0, zlib-rs 0.6.8

**(MIT OR Apache-2.0) AND Unicode-3.0** (2): unicode-ident 1.0.27, unicode-normalization 0.1.26

**Apache-2.0 OR ISC OR MIT** (2): hyper-rustls 0.27.10, rustls 0.23.46

**Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT** (2): linux-raw-sys 0.12.1, rustix 1.1.6

**BSD-2-Clause OR Apache-2.0 OR MIT** (2): zerocopy 0.8.63, zerocopy-derive 0.8.63

**BSD-3-Clause OR Apache-2.0** (2): moxcms 0.7.11, pxfm 0.1.30

**BSL-1.0** (2): clipboard-win 5.4.3, error-code 3.4.0

**MIT OR Zlib OR Apache-2.0** (2): miniz_oxide 0.8.9, miniz_oxide 0.9.1

**Unlicense** (2): whisper-rs 0.15.1, whisper-rs-sys 0.14.1

**Unlicense/MIT** (2): same-file 1.0.6, walkdir 2.5.0

**0BSD OR MIT OR Apache-2.0** (1): adler2 2.0.1

**Apache-2.0 / MIT** (1): fnv 1.0.7

**Apache-2.0 AND ISC** (1): ring 0.17.14

**Apache-2.0 AND MIT** (1): dpi 0.1.2

**Apache-2.0 OR BSL-1.0** (1): ryu 1.0.23

**BSD-2-Clause OR MIT OR Apache-2.0** (1): mach2 0.6.0

**BSD-3-Clause AND MIT** (1): brotli 8.0.4

**BSD-3-Clause/MIT** (1): brotli-decompressor 5.0.3

**CC0-1.0 OR MIT-0 OR Apache-2.0** (1): dunce 1.0.5

**CDLA-Permissive-2.0** (1): webpki-roots 1.0.11

**Zlib OR MIT OR Apache-2.0** (1): visibility 0.1.1
