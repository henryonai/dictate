# License texts

What is in this folder is the source of `THIRD_PARTY_LICENSES.txt`, the file of license texts that every installer of Dictate carries (next to `LICENSE`, [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md) and `COPYING.LGPLv2.1`). [scripts/generate-licenses.py](../scripts/generate-licenses.py) joins these files with the license files of every Rust crate and npm package that the program is built from:

```sh
python3 scripts/generate-licenses.py
```

It needs `cargo`, and the interface installed with `pnpm --dir app install`. Run it again, and commit `THIRD_PARTY_LICENSES.txt`, whenever `Cargo.lock` changes (the digest at the top of the file says which `Cargo.lock` it was made from, and a test in `app/ui/src/docs.test.ts` fails until it is current) or a native component below changes version. `python3 scripts/generate-licenses.py --check` says whether the file is current and writes nothing.

## `native/`: the components that are not Rust crates or npm packages

Each file is the license file of the release that Dictate is built with, copied as it is. When a version changes, copy the new file and change the version here and in the generator.

| File | What it is | Where it was taken from |
|---|---|---|
| `ffmpeg-LICENSE.md` | What FFmpeg 9.0.2 says of the licenses of its files | `LICENSE.md` of <https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz> (its `COPYING.LGPLv2.1` is the file of the same name at the root of the repository, which is also the text of the LGPL 2.1 for glibc) |
| `libvpx-LICENSE.txt`, `libvpx-PATENTS.txt` | libvpx 1.14.0: BSD-3-Clause and the additional IP rights grant | `LICENSE` and `PATENTS` of <https://github.com/webmproject/libvpx> at `v1.14.0` |
| `opus-COPYING.txt` | libopus 1.4: BSD-3-Clause | `COPYING` of <https://github.com/xiph/opus> at `v1.4` |
| `zlib-LICENSE.txt` | zlib | `LICENSE` of <https://github.com/madler/zlib> at `v1.3.1` (the text of 1.3 is the same) |
| `gcc-COPYING.RUNTIME.txt`, `gcc-COPYING3.txt` | The GCC Runtime Library Exception 3.1 and the GPL 3, for libgcc | <https://github.com/gcc-mirror/gcc> at `releases/gcc-13.3.0` |
| `mingw-w64-runtime-COPYING.txt`, `mingw-w64-COPYING.txt`, `winpthreads-COPYING.txt` | The notices that the runtime of mingw-w64 and winpthreads ask for | <https://github.com/mingw-w64/mingw-w64> at `v11.0.1` (`COPYING.MinGW-w64-runtime`, `COPYING.MinGW-w64` and `mingw-w64-libraries/winpthreads/COPYING`) |
| `nv-codec-headers-NOTICE.txt` | The MIT notice of the header files that the Windows encoders are compiled with | The comment at the top of `include/ffnvcodec/nvEncodeAPI.h` of <https://github.com/FFmpeg/nv-codec-headers> at `n12.1.14.0` |
| `amf-LICENSE.txt` | The AMF headers v1.5.3 (MIT) | `LICENSE.txt` of <https://github.com/GPUOpen-LibrariesAndSDKs/AMF> at `v1.5.3` |
| `whisper.cpp-LICENSE.txt` | whisper.cpp and ggml (MIT) | `whisper.cpp/LICENSE` in the source of the crate `whisper-rs-sys` 0.14.1, which has whisper.cpp in it |
| `openai-whisper-LICENSE.txt` | OpenAI Whisper, the models (MIT) | `LICENSE` of <https://github.com/openai/whisper> |
| `pyannote-segmentation-3.0-LICENSE.txt` | pyannote segmentation 3.0, in the ONNX form that is downloaded (MIT) | `LICENSE` of <https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0> |
| `sherpa-onnx-LICENSE.txt` | sherpa-onnx 1.13.8 (Apache-2.0); the project has no `NOTICE` file | `LICENSE` of <https://github.com/k2-fsa/sherpa-onnx> at `v1.13.8` |
| `onnxruntime-LICENSE.txt`, `onnxruntime-ThirdPartyNotices.txt` | ONNX Runtime 1.28.2 (MIT) and the notices of the code that it contains; the project has no `NOTICE` file | `LICENSE` and `ThirdPartyNotices.txt` of <https://github.com/microsoft/onnxruntime> at `v1.28.2`, the version that `cmake/onnxruntime-*.cmake` of sherpa-onnx 1.13.8 downloads |

## `spdx/`: standard texts

A Rust crate that ships no license file in its package gets the standard text of the license that it declares, from the [SPDX license list](https://github.com/spdx/license-list-data) (`text/<identifier>.txt`), with the authors that its `Cargo.toml` names. (`cargo about` was tried first and is not used: it leaves out the crates that a feature of one target brings in, such as the keyring backend of macOS.)
