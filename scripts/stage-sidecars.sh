#!/usr/bin/env bash
# Puts the programs and libraries that go into an installer where the Tauri bundler looks for them.
#
# Usage: scripts/stage-sidecars.sh [--dry-run] TARGET [FFMPEG_DIR]
#   TARGET      a Rust target triple: aarch64-apple-darwin, x86_64-apple-darwin, x86_64-unknown-linux-gnu or x86_64-pc-windows-msvc
#   FFMPEG_DIR  where scripts/build-ffmpeg.sh wrote `dictate-ffmpeg[.exe]` for TARGET (target/ffmpeg/TARGET unless it is given)
#   --dry-run   says what it would do, in the lines of a real run, and does nothing (it needs none of the files)
#
# What it makes, from the root of the repository:
#   - the MCP server: `cargo build -p dictate-mcp --release --locked --target TARGET`, copied to
#     app/src-tauri/binaries/dictate-mcp-TARGET[.exe]
#   - the ffmpeg: app/src-tauri/binaries/dictate-ffmpeg-TARGET[.exe]
#   - Linux: the two libraries of sherpa-onnx (libsherpa-onnx-c-api.so, libonnxruntime.so) in app/src-tauri/libs/, from
#     SHERPA_ONNX_LIB_DIR (scripts/fetch-sherpa-onnx-no-tts.sh prints it; target/sherpa-onnx-no-tts/lib unless it is set)
#   - Windows: the runtime of Visual C++ (vcruntime140.dll, vcruntime140_1.dll, msvcp140.dll) in app/src-tauri/libs/, from the
#     redistributable folder of the Visual Studio of the machine (VC_REDIST_DIR, when it is set, names the folder of the three)
#
# `externalBin` of the bundler takes `binaries/NAME` and looks for `binaries/NAME-TRIPLE[.exe]`; the file goes in the package as NAME. The
# release configuration (app/src-tauri/tauri.release.conf.json) names them: the ordinary one does not, so that a build or a test that
# is not for an installer does not need these files. Both folders are ignored by git.
#
# What is missing is found before anything is built: the build of the MCP server takes minutes.
set -euo pipefail

fail() {
  echo "stage-sidecars: $*" >&2
  exit 1
}

dry_run=0
args=()
for arg in "$@"; do
  case "$arg" in
    --dry-run) dry_run=1 ;;
    *) args+=("$arg") ;;
  esac
done
if [ "${#args[@]}" -lt 1 ] || [ "${#args[@]}" -gt 2 ]; then
  fail "usage: $0 [--dry-run] TARGET [FFMPEG_DIR]"
fi
target="${args[0]}"
case "$target" in
  aarch64-apple-darwin | x86_64-apple-darwin | x86_64-unknown-linux-gnu) exe="" ;;
  x86_64-pc-windows-msvc) exe=".exe" ;;
  *) fail "unknown target $target (it is one of aarch64-apple-darwin, x86_64-apple-darwin, x86_64-unknown-linux-gnu, x86_64-pc-windows-msvc)" ;;
esac

cd "$(dirname "${BASH_SOURCE[0]}")/.."
ffmpeg_dir="${args[1]:-target/ffmpeg/$target}"
bin_dir="app/src-tauri/binaries"
lib_dir="app/src-tauri/libs"

# What there is to copy, as "source -> destination" pairs, in the order of the copies.
mcp_from="target/$target/release/dictate-mcp$exe"
ffmpeg_from="$ffmpeg_dir/dictate-ffmpeg$exe"
copies=("$mcp_from -> $bin_dir/dictate-mcp-$target$exe" "$ffmpeg_from -> $bin_dir/dictate-ffmpeg-$target$exe")
# What has to be there before the build starts.
needed=("$ffmpeg_from")
hints=("run scripts/build-ffmpeg.sh $target $ffmpeg_dir first")

case "$target" in
  x86_64-unknown-linux-gnu)
    sherpa="${SHERPA_ONNX_LIB_DIR:-target/sherpa-onnx-no-tts/lib}"
    for library in libsherpa-onnx-c-api.so libonnxruntime.so; do
      copies+=("$sherpa/$library -> $lib_dir/$library")
      needed+=("$sherpa/$library")
      hints+=("run scripts/fetch-sherpa-onnx-no-tts.sh and set SHERPA_ONNX_LIB_DIR to what it prints")
    done
    ;;
  x86_64-pc-windows-msvc)
    redist="${VC_REDIST_DIR:-}"
    if [ -z "$redist" ] && [ "$dry_run" -eq 0 ]; then
      # The folder of the runtime in the Visual Studio that is installed (the newest version of it): VC/Redist/MSVC/<version>/x64/Microsoft.VC14x.CRT.
      vswhere="$(printenv 'ProgramFiles(x86)' || true)/Microsoft Visual Studio/Installer/vswhere.exe"
      if [ -x "$vswhere" ]; then
        studio="$("$vswhere" -latest -products '*' -property installationPath | tr -d '\r')"
        studio="$(cygpath -u "$studio" 2>/dev/null || printf '%s' "$studio")"
        redist="$(ls -d "$studio"/VC/Redist/MSVC/*/x64/Microsoft.VC*.CRT 2>/dev/null | sort -V | tail -n 1 || true)"
      fi
      [ -n "$redist" ] || fail "the redistributable folder of Visual C++ was not found: set VC_REDIST_DIR to the folder that has vcruntime140.dll"
    fi
    for dll in vcruntime140.dll vcruntime140_1.dll msvcp140.dll; do
      copies+=("${redist:-<VC_REDIST_DIR>}/$dll -> $lib_dir/$dll")
      needed+=("$redist/$dll")
      hints+=("VC_REDIST_DIR is not the folder of the runtime of Visual C++")
    done
    ;;
esac

if [ "$dry_run" -eq 0 ]; then
  for i in "${!needed[@]}"; do
    [ -f "${needed[$i]}" ] || fail "no such file: ${needed[$i]}: ${hints[$i]}"
  done
fi

echo "cargo build -p dictate-mcp --release --locked --target $target"
if [ "$dry_run" -eq 0 ]; then
  cargo build -p dictate-mcp --release --locked --target "$target"
  [ -f "$mcp_from" ] || fail "no such file: $mcp_from: the build did not make the MCP server"
fi

for pair in "${copies[@]}"; do
  from="${pair%% -> *}"
  to="${pair##* -> }"
  echo "copy $from -> $to"
  if [ "$dry_run" -eq 0 ]; then
    mkdir -p "$(dirname "$to")"
    # Not written over: macOS kills a program whose file is overwritten in place once it has been signed or run (the kernel keeps what it
    # knows of the file by its inode), so a staged file that is there from an earlier run is removed and the copy is a new file.
    rm -f "$to"
    cp -L "$from" "$to"
    # The bundler gives the package the permissions of the file. A sidecar that came without the execute bit (a file that went through
    # upload-artifact and download-artifact has none) would be in the installer as a file that cannot run: on macOS and Linux it is
    # made executable here. (The libraries are not programs.)
    case "$to" in
      "$bin_dir"/*) [ -n "$exe" ] || chmod 0755 "$to" ;;
    esac
  fi
done
