#!/usr/bin/env bash
# Fetches the build of sherpa-onnx for Linux x86_64 that has no text to speech in it, checks its SHA-256 and unpacks it.
#
# Usage: scripts/fetch-sherpa-onnx-no-tts.sh [DEST]        (DEST is target/sherpa-onnx-no-tts unless it is given)
#   export SHERPA_ONNX_LIB_DIR="$(scripts/fetch-sherpa-onnx-no-tts.sh)"
#
# Why: the speech synthesis of sherpa-onnx has espeak-ng in it, which is GPL-3.0 and which Dictate has no use for. The official
# Linux library is a shared one that has all of its own code in it, so the ordinary archive ships espeak-ng with the app; this one
# is the same release built with the synthesis off (scripts/check-no-espeak.sh is what finds out). The build script of
# `sherpa-onnx-sys` takes the library from SHERPA_ONNX_LIB_DIR when it is set, and `crates/dictate-services/build.rs` makes sure it
# is on Linux.
#
# It prints the folder to put in SHERPA_ONNX_LIB_DIR (DEST/lib, as an absolute path) on stdout, and nothing else there; the rest
# goes to stderr. A DEST that has a `lib` folder already (a CI cache restored it) is used as it is, with no download. The archive
# has to be the pinned one: its SHA-256 is checked before anything is unpacked, and a run that fails leaves no DEST behind, so that
# a half-made one is never taken for a finished one. The unpacking is done beside DEST and moved into place.
#
# For the tests of this script only: SHERPA_ONNX_NO_TTS_URL and SHERPA_ONNX_NO_TTS_SHA256 replace the release and its hash.
#
# There is no such build for ARM64 Linux: the release has only the one with espeak-ng (`linux-aarch64-shared-cpu-lib`).

set -euo pipefail

VERSION="1.13.8"
ARCHIVE="sherpa-onnx-v${VERSION}-linux-x64-shared-no-tts-lib.tar.bz2"
URL="${SHERPA_ONNX_NO_TTS_URL:-https://github.com/k2-fsa/sherpa-onnx/releases/download/v${VERSION}/${ARCHIVE}}"
# The digest that GitHub lists for the release asset, which a download of it hashes to as well.
SHA256="${SHERPA_ONNX_NO_TTS_SHA256:-bf2d998c8b07012cd5098f3b92673bc1333fd9b927767d7cb664be8190d8bc0b}"

fail() {
  echo "fetch-sherpa-onnx-no-tts: $*" >&2
  exit 1
}

hash_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

if [ -z "${SHERPA_ONNX_NO_TTS_URL:-}" ]; then
  case "$(uname -s)-$(uname -m)" in
    Linux-x86_64 | Linux-amd64) ;;
    *) fail "the release has a build without text to speech only for Linux x86_64, and this is $(uname -s) $(uname -m)" ;;
  esac
fi

dest="${1:-target/sherpa-onnx-no-tts}"
parent="$(dirname "$dest")"
mkdir -p "$parent"
dest="$(cd "$parent" && pwd)/$(basename "$dest")"

if [ -d "$dest/lib" ]; then
  echo "$dest/lib"
  exit 0
fi

# Beside DEST, so that the last step is a rename; removed whatever happens.
work="$(mktemp -d "$parent/.fetch-sherpa-onnx.XXXXXX")"
trap 'rm -rf "$work"' EXIT

echo "downloading $URL" >&2
curl -fsSL --retry 3 -o "$work/$ARCHIVE" "$URL" || fail "the download failed"

actual="$(hash_of "$work/$ARCHIVE")"
if [ "$actual" != "$SHA256" ]; then
  fail "the SHA-256 of the archive is $actual, not the pinned $SHA256"
fi

mkdir "$work/unpacked"
tar -xjf "$work/$ARCHIVE" -C "$work/unpacked" --strip-components=1 || fail "the unpacking failed"
[ -d "$work/unpacked/lib" ] || fail "the archive has no lib folder"

rm -rf "$dest"
mv "$work/unpacked" "$dest"
echo "$dest/lib"
