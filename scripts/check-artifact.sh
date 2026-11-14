#!/usr/bin/env bash
# Fails when an installer of Dictate is missing what it must ship, or has what it must not. The release workflow runs it on every installer
# before the installer is attached to a release, so that a broken one fails the job and is never published.
#
# Usage: scripts/check-artifact.sh ARTIFACT
#   ARTIFACT  an installer that the Tauri bundler made (it is unpacked in a folder of its own, which is removed afterwards):
#               *.dmg  macOS    mounted read-only with hdiutil; the app is copied out and the image is ejected, whatever the result is
#               *.exe  Windows  the NSIS installer, unpacked with 7z
#               *.deb  Linux    unpacked with dpkg-deb -R (the files, and DEBIAN/control)
#             or a folder that has what one of them unpacks to (the tests of this script use that): a `*.app` is macOS, `usr/bin` is the
#             package, `*.exe` at the top is Windows.
#
# What it requires of every installer:
#   - `dictate-mcp` and `dictate-ffmpeg` (with `.exe` on Windows) next to the main program: Contents/MacOS, usr/bin, the install folder;
#     and, on macOS and Linux, executable (upload-artifact and download-artifact drop the permission bits of a file).
#   - no espeak-ng (GPL-3.0) in the main program, in the two sidecars or in any .so, .dylib or .dll: scripts/check-no-espeak.sh.
#   - `dictate-ffmpeg` is a plain LGPL ffmpeg: scripts/check-ffmpeg-lgpl.sh.
#   - the license files: LICENSE, THIRD_PARTY_NOTICES.md, THIRD_PARTY_LICENSES.txt and COPYING.LGPLv2.1, not empty, where the system keeps
#     them (macOS Contents/Resources/Licenses, Windows `Licenses` next to the program, the package /usr/share/doc/dictate). The MIT, BSD,
#     Apache and LGPL licenses of what is inside ask that their texts go with the binary.
# The package for Linux (deb):
#   - a `copyright` file next to the license files, which is where the tools of Debian look for it;
#   - no /usr/bin/ffmpeg: it would collide with the package `ffmpeg` of Debian and Ubuntu (the sidecar is called dictate-ffmpeg);
#   - the two libraries of sherpa-onnx in /usr/lib/dictate, and the run path `$ORIGIN/../lib/dictate` in /usr/bin/dictate (readelf -d), which
#     is how the program finds them, and no other entry in its RPATH or RUNPATH (the ones of a build in the source tree would make it look in
#     /usr and in /);
#   - a dependency on libpipewire in `Depends` (the program does not start without it).
# The installer for Windows: the four DLLs of the runtime of Visual C++ (vcruntime140.dll, vcruntime140_1.dll, msvcp140.dll, msvcp140_1.dll)
# next to the program: a Windows that has no runtime would not start it. And every DLL of that family that a program or a DLL of the
# installer asks for by name (read from its bytes: msvcp, vcruntime, concrt, vcomp or vccorlib and a number, such as MSVCP140_1.dll) is
# beside it too, so that the next import that a new dependency brings in is found here and not on a clean Windows.
# The disk image for macOS: Info.plist has NSMicrophoneUsageDescription, NSScreenCaptureUsageDescription and LSMinimumSystemVersion 13.0,
# `codesign --verify --strict` passes on the app and on both sidecars (the signature of a sidecar that was written over is broken, and
# macOS kills the program), and each of the three programs is of the architecture that the name of the image says (`lipo -archs`;
# Dictate_<version>_aarch64.dmg is arm64, Dictate_<version>_x64.dmg is x86_64; for a folder, DICTATE_ARTIFACT_ARCH says it) and has 13.0 as
# its minimum system (`vtool -show-build`, `minos`). An image whose architecture is not known is exit status 2, not a pass.
#
# Every problem is told, not only the first. Exit status: 0 when the installer is fine, 1 when it is not, 2 when it could not be looked at
# (a file that is not there, a kind that it cannot unpack, a tool that unpacks it that is missing): a check that did not run is not a pass.
#
# It is written for the bash 3.2 of macOS as well as for bash 4 and later: no `mapfile`, no associative arrays, and no empty array is expanded.
set -u

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

cannot() {
  echo "check-artifact: $*" >&2
  exit 2
}

[ "$#" -eq 1 ] || {
  echo "usage: $0 ARTIFACT" >&2
  exit 2
}
artifact="$1"
[ -e "$artifact" ] || cannot "no such file: $artifact"

work=""
mounted=""
errors=""
cleanup() {
  if [ -n "$mounted" ]; then
    hdiutil detach "$mounted" >/dev/null 2>&1 || hdiutil detach -force "$mounted" >/dev/null 2>&1
  fi
  [ -z "$errors" ] || rm -f "$errors"
  [ -z "$work" ] || rm -rf "$work"
}
trap cleanup EXIT
trap 'exit 1' INT TERM HUP
errors="$(mktemp)"

problems=""
problem() {
  # The lines after the first of a problem (what a check printed) are indented under it.
  problems="$problems  - ${1//$'\n'/$'\n'    }"$'\n'
}

# ---- unpacking ---------------------------------------------------------------------------------------------------------------

# The tool that unpacks NSIS: 7-Zip is `7z` on Linux (p7zip) and `7zz` in a few packages; on a Windows runner it may not be on the PATH.
seven_zip() {
  local name
  for name in 7z 7zz 7za; do
    if command -v "$name" >/dev/null 2>&1; then
      echo "$name"
      return 0
    fi
  done
  for name in "${ProgramFiles:-}/7-Zip/7z.exe" "${PROGRAMFILES:-}/7-Zip/7z.exe" "/c/Program Files/7-Zip/7z.exe"; do
    if [ -x "$name" ]; then
      echo "$name"
      return 0
    fi
  done
  return 1
}

kind=""
root=""
if [ -d "$artifact" ]; then
  root="$artifact"
else
  work="$(mktemp -d)"
  lower="$(printf '%s' "$artifact" | tr 'A-Z' 'a-z')"
  case "$lower" in
    *.deb)
      kind=deb
      mkdir -p "$work/deb"
      dpkg-deb -R "$artifact" "$work/deb" || cannot "dpkg-deb could not unpack $artifact"
      root="$work/deb"
      ;;
    *.exe)
      kind=windows
      zip="$(seven_zip)" || cannot "7z is not installed: $artifact cannot be unpacked"
      "$zip" x -y "-o$work/nsis" "$artifact" >/dev/null || cannot "7z could not unpack $artifact"
      root="$work/nsis"
      ;;
    *.dmg)
      kind=macos
      mkdir -p "$work/volume" "$work/app"
      hdiutil attach -readonly -nobrowse -noverify -noautoopen -mountpoint "$work/volume" "$artifact" >/dev/null ||
        cannot "hdiutil could not mount $artifact"
      mounted="$work/volume"
      volume_app="$(ls -d "$work/volume"/*.app 2>/dev/null | head -n 1)"
      if [ -z "$volume_app" ]; then
        cannot "there is no .app in the disk image $artifact"
      fi
      # Copied out (ditto keeps the signature, the links and the extended attributes), so that the image is ejected before the checks.
      if command -v ditto >/dev/null 2>&1; then
        ditto "$volume_app" "$work/app/$(basename "$volume_app")" || cannot "ditto could not copy the app out of $artifact"
      else
        cp -R "$volume_app" "$work/app/" || cannot "the app could not be copied out of $artifact"
      fi
      hdiutil detach "$mounted" >/dev/null 2>&1 || hdiutil detach -force "$mounted" >/dev/null 2>&1
      mounted=""
      root="$work/app"
      ;;
    *) cannot "do not know how to unpack $artifact: it is a .dmg, a .exe or a .deb (or a folder that one of them unpacks to)" ;;
  esac
fi

if [ -z "$kind" ]; then
  if ls -d "$root"/*.app >/dev/null 2>&1; then
    kind=macos
  elif [ -d "$root/usr/bin" ]; then
    kind=deb
  elif ls "$root"/*.exe >/dev/null 2>&1; then
    kind=windows
  else
    cannot "$artifact has none of the layouts of an installer: no .app, no usr/bin, no .exe at the top"
  fi
fi

# ---- where things are --------------------------------------------------------------------------------------------------------

exe=""
plist=""
app=""
case "$kind" in
  macos)
    app="$(ls -d "$root"/*.app 2>/dev/null | head -n 1)"
    [ -n "$app" ] || cannot "there is no .app in $artifact"
    plist="$app/Contents/Info.plist"
    bin_dir="$app/Contents/MacOS"
    ;;
  deb) bin_dir="$root/usr/bin" ;;
  windows)
    exe=".exe"
    bin_dir="$root"
    ;;
esac

# The value of a key of Info.plist (nothing when it is not there).
plist_value() {
  plutil -extract "$1" raw -o - "$plist" 2>/dev/null
}

if [ "$kind" = macos ]; then
  command -v plutil >/dev/null 2>&1 || cannot "plutil is not installed: Info.plist cannot be read"
  command -v codesign >/dev/null 2>&1 || cannot "codesign is not installed: the signatures cannot be verified"
  command -v lipo >/dev/null 2>&1 || cannot "lipo is not installed: the architecture of the programs cannot be checked"
  command -v vtool >/dev/null 2>&1 || cannot "vtool is not installed: the minimum system of the programs cannot be checked"
  # The architecture that the programs have to be: the name of the image says it, and a folder is told by the variable.
  expected_arch=""
  case "$(printf '%s' "$artifact" | tr 'A-Z' 'a-z')" in
    *_aarch64.dmg) expected_arch=arm64 ;;
    *_x64.dmg) expected_arch=x86_64 ;;
    *)
      if [ -d "$artifact" ]; then expected_arch="${DICTATE_ARTIFACT_ARCH:-}"; fi
      ;;
  esac
  case "$expected_arch" in
    arm64 | x86_64) ;;
    *) cannot "do not know which architecture the programs of $artifact have to be: the name of an image ends in _aarch64.dmg (arm64) or _x64.dmg (x86_64), and for a folder DICTATE_ARTIFACT_ARCH is arm64 or x86_64" ;;
  esac
  # The program is the one that Info.plist names (the app is `Dictate`, the program `dictate`).
  main_name="$(plist_value CFBundleExecutable)"
  main_name="${main_name:-dictate}"
else
  main_name="dictate$exe"
fi

# What the folder is called in what is told, for a person: the path inside the installer.
inside() {
  local path="${1#"$root"/}"
  case "$kind" in
    deb) echo "/$path" ;;
    *) echo "$path" ;;
  esac
}

# ---- the programs ------------------------------------------------------------------------------------------------------------

programs=""
require_program() {
  local path="$1" label
  label="$(inside "$path")"
  if [ ! -f "$path" ]; then
    problem "$label is not in the installer"
    return
  fi
  programs="$programs$path"$'\n'
  if [ "$kind" != windows ] && [ ! -x "$path" ]; then
    problem "$label is not executable (the permission bits of a file are lost when it goes through an artifact of Actions)"
  fi
}

require_program "$bin_dir/$main_name"
require_program "$bin_dir/dictate-mcp$exe"
require_program "$bin_dir/dictate-ffmpeg$exe"

# ---- no espeak-ng ------------------------------------------------------------------------------------------------------------

# The programs, and every shared library there is, wherever it is.
espeak_files=()
while IFS= read -r file; do
  [ -z "$file" ] || espeak_files+=("$file")
done <<EOF
$programs
EOF
while IFS= read -r -d '' file; do
  espeak_files+=("$file")
done < <(find "$root" -type f \( -name '*.so' -o -name '*.so.*' -o -name '*.dylib' -o -name '*.dll' \) -print0)

if [ "${#espeak_files[@]}" -gt 0 ]; then
  if ! bash "$here/check-no-espeak.sh" "${espeak_files[@]}" 2>"$errors"; then
    problem "espeak-ng (GPL-3.0) is in what the installer ships, or a file could not be checked:
$(cat "$errors")"
  fi
fi

# ---- ffmpeg ------------------------------------------------------------------------------------------------------------------

if [ -f "$bin_dir/dictate-ffmpeg$exe" ]; then
  if ! bash "$here/check-ffmpeg-lgpl.sh" "$bin_dir/dictate-ffmpeg$exe" 2>"$errors"; then
    problem "$(inside "$bin_dir/dictate-ffmpeg$exe") is not a plain LGPL ffmpeg:
$(cat "$errors")"
  fi
fi

# ---- the license texts -------------------------------------------------------------------------------------------------------

# Where the installer keeps its license files (the release overlay puts them there).
case "$kind" in
  macos) licenses_dir="$app/Contents/Resources/Licenses" ;;
  deb) licenses_dir="$root/usr/share/doc/dictate" ;;
  windows) licenses_dir="$root/Licenses" ;;
esac

check_licenses() {
  local name
  for name in LICENSE THIRD_PARTY_NOTICES.md THIRD_PARTY_LICENSES.txt COPYING.LGPLv2.1; do
    if [ ! -f "$licenses_dir/$name" ]; then
      problem "$(inside "$licenses_dir/$name") is not in the installer: the licenses of what is inside ask that their texts go with it (the release overlay puts the license files there)"
    elif [ ! -s "$licenses_dir/$name" ]; then
      problem "$(inside "$licenses_dir/$name") is empty: it is a license file with no license text"
    fi
  done
  if [ "$kind" = deb ] && [ ! -s "$licenses_dir/copyright" ]; then
    problem "$(inside "$licenses_dir/copyright") is not in the package, or is empty: the tools of Debian look for the copyright file of a package there"
  fi
}

check_licenses

# ---- what is only in one kind of installer -----------------------------------------------------------------------------------

check_deb() {
  if [ -e "$root/usr/bin/ffmpeg" ]; then
    problem "the package has /usr/bin/ffmpeg: it would collide with the package ffmpeg of Debian and Ubuntu (dpkg refuses to overwrite it); the sidecar is dictate-ffmpeg"
  fi
  local library
  for library in libsherpa-onnx-c-api.so libonnxruntime.so; do
    [ -f "$root/usr/lib/dictate/$library" ] || problem "/usr/lib/dictate/$library is not in the package: the program would not start"
  done

  # The run path that finds those libraries from /usr/bin/dictate: a whole entry of the RPATH or the RUNPATH.
  if [ -f "$bin_dir/$main_name" ]; then
    if ! command -v readelf >/dev/null 2>&1; then
      problem "readelf is not installed: the run path of /usr/bin/dictate cannot be checked"
    else
      local paths
      paths="$(readelf -d "$bin_dir/$main_name" 2>/dev/null | sed -n -E 's/.*\((RPATH|RUNPATH)\).*\[(.*)\].*/\2/p' | tr ':' '\n')"
      if ! printf '%s\n' "$paths" | grep -q -x -F -e '$ORIGIN/../lib/dictate'; then
        problem '/usr/bin/dictate has no RPATH or RUNPATH with $ORIGIN/../lib/dictate: it would not find its libraries in /usr/lib/dictate (readelf -d says: '"$(printf '%s' "$paths" | tr '\n' ' ')"')'
      fi
      # And no other: the entries of a build in the source tree ($ORIGIN, $ORIGIN/.., $ORIGIN/../../../..) are, from /usr/bin, /usr/bin, /usr
      # and /: a program that looks there loads a library that somebody put there.
      local others
      others="$(printf '%s\n' "$paths" | grep -v -x -F -e '$ORIGIN/../lib/dictate' | grep -v '^$' | tr '\n' ' ' || true)"
      if [ -n "$others" ]; then
        problem '/usr/bin/dictate has other entries in its RPATH or RUNPATH than $ORIGIN/../lib/dictate: '"$others"'(the release is built with DICTATE_RPATH_INSTALLED=1, which gives only $ORIGIN/../lib/dictate)'
      fi
    fi
  fi

  # `Depends` is the field of that name (a long one goes on in the lines that start with a blank); Recommends does not count.
  local control="$root/DEBIAN/control" depends
  if [ ! -f "$control" ]; then
    problem "DEBIAN/control is not in the package"
  else
    depends="$(awk '/^Depends:/ { keep = 1; print; next } /^[[:space:]]/ { if (keep) print; next } { keep = 0 }' "$control")"
    case "$depends" in
      *libpipewire*) ;;
      *) problem "the Depends of the package has no libpipewire (the program does not start without it): ${depends:-no Depends field}" ;;
    esac
  fi
}

check_windows() {
  local dll file name
  for dll in vcruntime140.dll vcruntime140_1.dll msvcp140.dll msvcp140_1.dll; do
    [ -f "$root/$dll" ] || problem "$dll is not next to dictate.exe: a Windows without the runtime of Visual C++ would not start it"
  done
  # What the programs and the DLLs ask for: the names are in the import table as plain text, in the capitals of the linker, and the files
  # of the installer are in small letters. (find -iname: a file system that tells the two apart is the one of the Linux that unpacks it.)
  for file in "$root"/*.exe "$root"/*.dll; do
    [ -f "$file" ] || continue
    for name in $(grep -a -o -i -E '(msvcp|vcruntime|concrt|vcomp|vccorlib)[0-9]+[a-z0-9_]*\.dll' "$file" | tr 'A-Z' 'a-z' | sort -u); do
      if [ -z "$(find "$root" -maxdepth 1 -iname "$name" -print -quit)" ]; then
        problem "$(basename "$file") asks for $name, which is not next to it: a Windows without the runtime of Visual C++ would not start it"
      fi
    done
  done
}

check_macos() {
  local key value minimum path
  for key in NSMicrophoneUsageDescription NSScreenCaptureUsageDescription; do
    value="$(plist_value "$key")"
    [ -n "$value" ] || problem "Info.plist has no $key"
  done
  minimum="$(plist_value LSMinimumSystemVersion)"
  [ "$minimum" = "13.0" ] || problem "LSMinimumSystemVersion in Info.plist is '${minimum}', not 13.0"
  for path in "$app" "$bin_dir/dictate-mcp" "$bin_dir/dictate-ffmpeg"; do
    [ -e "$path" ] || continue
    if ! codesign --verify --strict "$path" 2>"$errors"; then
      problem "codesign --verify --strict fails on $(inside "$path"):
$(cat "$errors")"
    fi
  done
  # The architecture and the minimum system of each program: a sidecar of the other architecture is an app that opens and cannot start its
  # MCP server, and a library built for a newer macOS than the one that Info.plist promises makes the program fail to load on macOS 13.
  local archs minima one
  for path in "$bin_dir/$main_name" "$bin_dir/dictate-mcp" "$bin_dir/dictate-ffmpeg"; do
    [ -f "$path" ] || continue
    archs="$(lipo -archs "$path" 2>/dev/null | tr -s ' \n' ' ' | sed 's/ $//')"
    if [ "$archs" != "$expected_arch" ]; then
      problem "$(inside "$path") is of the architecture '${archs:-none that lipo knows}', and this installer is for $expected_arch"
    fi
    minima="$(vtool -show-build "$path" 2>/dev/null | awk '$1 == "minos" { print $2 }')"
    if [ -z "$minima" ]; then
      problem "$(inside "$path") has no minos in its build version (vtool -show-build), so its minimum system is not known to be 13.0"
    else
      for one in $minima; do
        [ "$one" = "13.0" ] || problem "$(inside "$path") has minos $one: it needs a newer macOS than the 13.0 of Info.plist"
      done
    fi
  done
}

case "$kind" in
  deb) check_deb ;;
  windows) check_windows ;;
  macos) check_macos ;;
esac

# ---- the result --------------------------------------------------------------------------------------------------------------

# What the installer has, with the size of each file: the log of a release is where it is looked at afterwards.
echo "what $artifact has (bytes, file):"
(
  cd "$root" &&
    find . -type f -print | LC_ALL=C sort | while IFS= read -r file; do
      printf '%12d %s\n' "$(wc -c <"$file")" "$file"
    done
)

if [ -n "$problems" ]; then
  echo "check-artifact: $artifact ($kind) is not fit to be released:" >&2
  printf '%s' "$problems" >&2
  exit 1
fi
echo "check-artifact: $artifact ($kind) is complete and clean"
