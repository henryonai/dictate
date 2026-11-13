#!/usr/bin/env bash
# Builds the minimal LGPL ffmpeg that Dictate ships next to its own program (a Tauri sidecar called `dictate-ffmpeg`).
#
# Usage: scripts/build-ffmpeg.sh TARGET OUTDIR
#   TARGET  a Rust target triple (the name that Tauri wants after `dictate-ffmpeg-`):
#             aarch64-apple-darwin | x86_64-apple-darwin   build on a Mac; x86_64 is cross-compiled on an arm64 one
#             x86_64-unknown-linux-gnu                     build on Ubuntu 24.04 x86_64; fully static
#             x86_64-pc-windows-msvc                       build on Linux with mingw-w64 (CROSS_PREFIX=x86_64-w64-mingw32-), or in an
#                                                          MSYS2 UCRT64 shell on Windows (no CROSS_PREFIX); the program is for Windows
#                                                          whichever of them makes it
#   OUTDIR  gets `dictate-ffmpeg[.exe]`, `BUILDCONF-TARGET.txt` (the configure line, for the offer of the source that the LGPL asks
#           for) and `ffmpeg-VERSION.tar.xz` (the exact source that the program was built from).
#
# It needs: a C compiler, make, curl, xz, git (Windows), and nasm for the x86_64 targets; on Linux pkg-config and the static archives of
# libvpx, libopus and zlib (apt-get install -y build-essential nasm pkg-config curl xz-utils libvpx-dev libopus-dev zlib1g-dev), on a
# Linux that builds the Windows program gcc-mingw-w64-x86-64-posix and binutils-mingw-w64-x86-64.
#
# What it makes: ffmpeg 9.0.2 with `--disable-everything` and then every demuxer, decoder, parser and bitstream filter (an import can
# need any of them) and only what the app asks of it for the rest (the encoders and muxers of the recordings, the few filters, the
# protocols: the lists below), under the LGPL v2.1 or later:
# `--disable-gpl --disable-nonfree --disable-version3`, and `configure` has to say so ("License: LGPL version 2.1 or later").
# scripts/check-ffmpeg-lgpl.sh is the check of the result. The source is the pinned tarball: its SHA-256 is checked before anything is
# unpacked, and a run that fails leaves nothing in OUTDIR.
#
# For the tests of this script only: FFMPEG_SOURCE_URL and FFMPEG_SOURCE_SHA256 replace the tarball and its hash.
set -euo pipefail

VERSION="9.0.2"
# SHA-256 of https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz (the same one that Homebrew pins).
SHA256="${FFMPEG_SOURCE_SHA256:-8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e}"
URL="${FFMPEG_SOURCE_URL:-https://ffmpeg.org/releases/ffmpeg-$VERSION.tar.xz}"
# Header-only SDKs for the Windows hardware encoders (MIT): the tags and their commits.
NVCODEC_TAG="n12.1.14.0" # nv-codec-headers: nvcuda.dll and nvEncodeAPI64.dll are loaded at run time; ffmpeg 9 accepts 11.1.5.3 to 13.x
NVCODEC_COMMIT="1889e62e2d35ff7aa9baca2bceb14f053785e6f1"
AMF_TAG="v1.5.3" # AMF headers: ffmpeg needs 1.5.2.0 or later; amfrt64.dll is loaded at run time
AMF_COMMIT="57ee0cfb0e1a08d9041f77435653794c1b012be0"

fail() {
  echo "build-ffmpeg: $*" >&2
  exit 1
}

[ "$#" -eq 2 ] || fail "usage: $0 TARGET OUTDIR"
TARGET="$1"
case "$TARGET" in
  aarch64-apple-darwin | x86_64-apple-darwin | x86_64-unknown-linux-gnu | x86_64-pc-windows-msvc) ;;
  *) fail "unknown target $TARGET (it is one of aarch64-apple-darwin, x86_64-apple-darwin, x86_64-unknown-linux-gnu, x86_64-pc-windows-msvc)" ;;
esac
OUT="$(mkdir -p "$2" && cd "$2" && pwd)"
JOBS="$(nproc 2>/dev/null || getconf _NPROCESSORS_ONLN 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 2)"
# The name of the program in OUTDIR is the one that the app looks for next to itself, on every system.
case "$TARGET" in
  *-windows-*) EXE="ffmpeg.exe" SIDECAR="dictate-ffmpeg.exe" ;;
  *) EXE="ffmpeg" SIDECAR="dictate-ffmpeg" ;;
esac

hash_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

WORK="$(mktemp -d)"
# A run that fails leaves nothing in OUTDIR: the files are made in WORK and copied at the end.
trap 'rm -rf "$WORK"' EXIT
cd "$WORK"

echo "downloading $URL" >&2
curl -fsSL --retry 3 -o "ffmpeg-$VERSION.tar.xz" "$URL" || fail "the download failed"
actual="$(hash_of "ffmpeg-$VERSION.tar.xz")"
if [ "$actual" != "$SHA256" ]; then
  fail "the SHA-256 of ffmpeg-$VERSION.tar.xz is $actual, not the pinned $SHA256"
fi
tar -xf "ffmpeg-$VERSION.tar.xz" || fail "the unpacking failed"

# --- what the app asks of ffmpeg (see crates/dictate-services/src/media/{decode,encode/ffmpeg}.rs and record/video.rs) ---------------
# --disable-everything, and then:
#  - every demuxer and every decoder (--enable-demuxers --enable-decoders). An import that symphonia cannot read (AC-3, WMA, AMR, odd
#    AAC, damaged files, whatever else) goes through ffmpeg, and Tools::find takes the ffmpeg that ships before the one of the user, so
#    a format that this build lacks is a format that cannot be imported. The ones that are GPL, version 3 or non-free (and the ones that
#    need a library that is not linked) stay out by themselves, because of the options above: configure says "Disabled ..." for them.
#    rawvideo and pcm_f32le (the frames of the screen and the f32le audio that the app pipes in) and mov and matroska (video.mp4 or
#    video.webm, the probe of a length, the mux) are among them.
#  - all the parsers and the bitstream filters: they are tiny (the mp4 muxer puts aac_adtstoasc in by itself for a raw .aac).
#  - only what the app asks for of the rest: the protocols, the encoders, the muxers and the filters.
PROTOCOLS="file,pipe"
# `anull` and `null` are what ffmpeg puts in a graph that has no filter; aformat, aresample and scale are put in by itself (the rate, the
# channels, bgra to yuv420p).
FILTERS="buffer,buffersink,abuffer,abuffersink,null,anull,format,aformat,aresample,scale,fps,atrim,trim,asetpts,setpts,volume"
# wrapped_avframe: the pseudo-encoder that the null muxer takes when a video is DECODED to nowhere (`-i video.mp4 -f null -`), which the
# tests of the repository do to read how long a recording is. The app never decodes a video (it copies it to find out how long it is).
COMMON=(
  --disable-gpl --disable-nonfree --disable-version3
  --disable-autodetect --disable-everything
  --disable-doc --disable-debug --disable-network --disable-ffplay --disable-ffprobe --disable-avdevice
  --enable-demuxers --enable-decoders --enable-parsers --enable-bsfs
  "--enable-protocol=$PROTOCOLS" "--enable-filter=$FILTERS" --enable-encoder=wrapped_avframe
)

STRIP=strip
CROSS_PREFIX="${CROSS_PREFIX-}"
case "$TARGET" in
  aarch64-apple-darwin | x86_64-apple-darwin)
    [ "$(uname -s)" = Darwin ] || fail "$TARGET is built on a Mac"
    export MACOSX_DEPLOYMENT_TARGET=13.0
    arch="${TARGET%%-*}"
    [ "$arch" = aarch64 ] && arch=arm64 # Rust says aarch64, clang and ffmpeg say arm64
    EXTRA=(--cc="clang -arch $arch" --arch="$arch" --target-os=darwin
      --extra-cflags=-mmacosx-version-min=13.0 --extra-ldflags=-mmacosx-version-min=13.0
      --enable-zlib --enable-videotoolbox --enable-audiotoolbox
      --enable-encoder=aac,h264_videotoolbox,mpeg4,pcm_f32le,pcm_s16le
      --enable-muxer=ipod,mp4,null,pcm_f32le)
    [ "$arch" = "$(uname -m)" ] || EXTRA+=(--enable-cross-compile)
    # The x86 assembly (swscale, swresample, the AAC encoder) needs nasm; the arm64 one does not.
    if [ "$arch" = x86_64 ] && ! command -v nasm >/dev/null 2>&1; then fail "nasm is needed for x86_64 (brew install nasm)"; fi
    ;;
  x86_64-unknown-linux-gnu)
    [ "$(uname -s)-$(uname -m)" = Linux-x86_64 ] || fail "$TARGET is built on Linux x86_64, and this is $(uname -s) $(uname -m)"
    command -v nasm >/dev/null 2>&1 || fail "nasm is needed (sudo apt-get install -y nasm)"
    command -v pkg-config >/dev/null 2>&1 || fail "pkg-config is needed (sudo apt-get install -y pkg-config libvpx-dev libopus-dev zlib1g-dev)"
    # Fully static: only libvpx (the VP9 of the WebM that Linux records), libopus (its audio) and zlib are linked, from the static
    # archives of libvpx-dev, libopus-dev and zlib1g-dev. The result is "not a dynamic executable", so no library of the system
    # is needed and it runs on any Linux.
    EXTRA=(--enable-zlib --enable-libvpx --enable-libopus
      --enable-encoder=aac,libvpx_vp9,libopus,mpeg4,pcm_f32le,pcm_s16le
      --enable-muxer=ipod,mp4,webm,null,pcm_f32le
      --pkg-config-flags=--static --extra-ldflags=-static)
    ;;
  x86_64-pc-windows-msvc)
    # mingw-w64. In an MSYS2 UCRT64 shell on Windows there is no cross prefix; on Linux it is x86_64-w64-mingw32- (gcc-mingw-w64-x86-64-posix).
    if [ "$(uname -s)" = Linux ] && [ -z "$CROSS_PREFIX" ]; then CROSS_PREFIX="x86_64-w64-mingw32-"; fi
    command -v nasm >/dev/null 2>&1 || fail "nasm is needed (sudo apt-get install -y nasm)"
    command -v "${CROSS_PREFIX}gcc" >/dev/null 2>&1 || fail "${CROSS_PREFIX}gcc is needed (sudo apt-get install -y gcc-mingw-w64-x86-64-posix binutils-mingw-w64-x86-64)"
    command -v git >/dev/null 2>&1 || fail "git is needed"
    STRIP="${CROSS_PREFIX}strip"
    deps="$WORK/deps"
    mkdir -p "$deps/include" "$deps/lib/pkgconfig"
    git -c advice.detachedHead=false clone -q --depth 1 --branch "$NVCODEC_TAG" https://github.com/FFmpeg/nv-codec-headers.git
    [ "$(git -C nv-codec-headers rev-parse HEAD)" = "$NVCODEC_COMMIT" ] || fail "nv-codec-headers $NVCODEC_TAG is not the pinned commit"
    make -s -C nv-codec-headers PREFIX="$deps" install
    # AMF: the whole repository is large, so only the headers are checked out (seconds, not minutes).
    git -c advice.detachedHead=false clone -q --filter=blob:none --no-checkout --depth 1 --branch "$AMF_TAG" https://github.com/GPUOpen-LibrariesAndSDKs/AMF.git
    git -C AMF sparse-checkout set --no-cone /amf/public/include && git -C AMF checkout -q
    [ "$(git -C AMF rev-parse HEAD)" = "$AMF_COMMIT" ] || fail "AMF $AMF_TAG is not the pinned commit"
    mkdir -p "$deps/include/AMF" && cp -r AMF/amf/public/include/* "$deps/include/AMF/"
    export PKG_CONFIG_PATH="$deps/lib/pkgconfig"
    # The encoder of the graphics card is `h264_mf` (Media Foundation: Intel, AMD and NVIDIA each have one), with `h264_nvenc` and
    # `h264_amf` for a machine where Media Foundation has none. h264_qsv would need libvpl, which h264_mf makes unnecessary.
    # --enable-d3d11va: h264_mf does not compile without it in ffmpeg 9.0.2 (mfenc.c uses the D3D11 types without a guard; with
    # --disable-autodetect it is off). --enable-nvenc: --enable-ffnvcodec does not imply it.
    EXTRA=(--arch=x86_64 --target-os=mingw32
      "--extra-cflags=-I$deps/include" "--extra-ldflags=-static -static-libgcc"
      --enable-mediafoundation --enable-d3d11va --enable-ffnvcodec --enable-nvenc --enable-amf --pkg-config=pkg-config
      --enable-encoder=aac,h264_mf,h264_nvenc,h264_amf,mpeg4,pcm_f32le,pcm_s16le
      --enable-muxer=ipod,mp4,null,pcm_f32le)
    if [ -n "$CROSS_PREFIX" ]; then EXTRA+=(--cross-prefix="$CROSS_PREFIX" --enable-cross-compile); fi
    ;;
esac

mkdir build
cd build
echo "configuring ffmpeg $VERSION for $TARGET" >&2
"../ffmpeg-$VERSION/configure" "${COMMON[@]}" "${EXTRA[@]}" >configure.log 2>&1 || { tail -40 configure.log >&2; fail "configure failed"; }
grep -q '^License: LGPL version 2.1 or later' configure.log || fail "configure did not say 'License: LGPL version 2.1 or later'"
echo "building with $JOBS jobs" >&2
make -j"$JOBS" >make.log 2>&1 || { tail -40 make.log >&2; fail "make failed"; }

case "$TARGET" in
  *-apple-darwin)
    "$STRIP" -x "$EXE"
    # The linker signed it ad hoc, and stripping broke that: a Mach-O that is not signed does not run on Apple silicon. Tauri signs it
    # again with the identity of the app when it makes the bundle.
    codesign --force --sign - "$EXE"
    ;;
  *) "$STRIP" "$EXE" ;;
esac

# What goes next to the program in the release, for the offer of the source: the configure line. A program that cannot run here (the
# Windows one on Linux, the Intel one without Rosetta) has the same line in config.h (it is what `ffmpeg -version` prints after
# "configuration:").
if "./$EXE" -version >/dev/null 2>&1; then
  "./$EXE" -hide_banner -buildconf >"$WORK/buildconf.txt"
else
  grep -h '^#define FFMPEG_CONFIGURATION' config.h | sed 's/^#define FFMPEG_CONFIGURATION "//; s/"$//' >"$WORK/buildconf.txt"
fi
[ -s "$WORK/buildconf.txt" ] || fail "the configure line of the build could not be read"
# The folder that the header-only SDKs were put in is made for the run: its name is not part of what was built.
sed "s#$WORK#<build folder>#g" "$WORK/buildconf.txt" >"$WORK/buildconf.clean.txt"

# A new file, not a copy over the old one: macOS keeps the code signature of a file that it has run, by the file, and a program that is
# written over in place can be killed (exit status 137) when it is started, with a signature that is valid on disk.
rm -f "$OUT/$SIDECAR"
cp "$EXE" "$OUT/$SIDECAR"
cp "$WORK/buildconf.clean.txt" "$OUT/BUILDCONF-$TARGET.txt"
cp "$WORK/ffmpeg-$VERSION.tar.xz" "$OUT/"
echo "built $OUT/$SIDECAR ($(wc -c <"$OUT/$SIDECAR" | tr -d ' ') bytes)" >&2
