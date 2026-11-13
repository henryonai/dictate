#!/usr/bin/env bash
# Fails when an ffmpeg is not a plain LGPL v2.1-or-later build.
#
# Usage: scripts/check-ffmpeg-lgpl.sh FILE...
#
# ffmpeg is LGPL only when it is built without --enable-gpl, --enable-nonfree and --enable-version3 (version3 makes it LGPL v3, which
# the build that ships does not want), and with none of the GPL or non-free libraries (x264, x265, xvid, fdk-aac, ...). Dictate is MIT
# and ships the ffmpeg that scripts/build-ffmpeg.sh makes next to its own program; this is the check that what ships is that one.
#
# What it looks at, for each file:
#   - the bytes of the file, so that it also works on a Windows .exe that was built on Linux and cannot run there: the configure
#     line and the name of the license are strings in the program. It fails on a GPL, non-free or version 3 option or library in
#     them, on code of x264, x265, xvid or fdk-aac, and when the string "LGPL version 2.1 or later" (libavutil's name of the
#     license it was built under) is not in them. (Not on a plain "libx264": that name is in every build, because the H.264 decoder
#     looks for the signature of x264 in a stream.)
#   - when the program can be run here: what `-buildconf` prints (the same options), what `-L` prints, which has to be the LGPL with
#     "version 2.1" and nothing that says GPL, version 3 or nonfree, and what `-encoders` lists, which has none of x264, x265, xvid or
#     fdk-aac (the list is what the program can do, whatever its configure line says).
#   - what it links: macOS only what is in /usr/lib and /System/Library, Linux nothing at all (the ffmpeg that ships is static, so
#     that it runs on any system), Windows only the DLLs that Windows itself has. A library of its own is code that the lines above
#     do not cover. When the tool that lists them is not installed (otool, readelf, objdump) the check fails for a macOS or Linux
#     program on that system and says so for the others: the Windows program is also checked, with mingw's objdump, by the job that
#     builds it on Linux, and the Windows runner (Git Bash) may have none.
#
# Exit status: 0 when every file is fine, 1 when one is not, 2 when a file is not there (a path that is misspelled must not make the
# check pass on nothing). The files that fail are named on stderr, with what is wrong; the ones that pass are named on stdout.
set -u

if [ "$#" -eq 0 ]; then
  echo "usage: $0 FILE..." >&2
  exit 2
fi

# The configure options that make a build GPL, version 3 or non-free, or add a GPL or non-free library. `--disable-gpl` is not one:
# the text is `--enable-`.
BAD_OPTIONS=(
  --enable-gpl --enable-nonfree --enable-version3
  --enable-libx264 --enable-libx265 --enable-libxvid --enable-libfdk-aac --enable-libvidstab --enable-librubberband
  --enable-frei0r --enable-avisynth --enable-libxavs --enable-libdavs2 --enable-libcdio --enable-libdvdread --enable-libdvdnav
)

# The encoders that only a GPL or non-free build has.
GPL_ENCODERS="libx264 libx264rgb libx265 libxvid libfdk_aac"

# The DLLs that Windows has (and the ones that a build with mingw links in by itself: msvcrt.dll). Lower case, without `.dll`.
WINDOWS_DLLS="kernel32 user32 gdi32 advapi32 shell32 ole32 oleaut32 bcrypt ncrypt ws2_32 msvcrt ucrtbase ntdll shlwapi crypt32 secur32 rpcrt4 winmm version imm32 setupapi cfgmgr32 powrprof psapi userenv iphlpapi mfplat mf mfuuid mfreadwrite d3d11 dxgi dxva2 d3d9 comdlg32 comctl32 shcore combase"

host_system="$(uname -s)"
found=""

# Adds a reason to what is wrong with the file (once).
add() {
  case "|$found|" in
    *"|$1|"*) ;;
    *) found="${found:+$found|}$1" ;;
  esac
}

# Whether the bytes of a file have the text. LC_ALL=C: the bytes of a program are not text in any language.
has_bytes() { LC_ALL=C grep -a -q -F -e "$1" -- "$2"; }

# The kind of a program, from the first bytes of it.
format_of() {
  case "$(LC_ALL=C head -c 4 -- "$1" | od -An -tx1 | tr -d ' \n')" in
    7f454c46) echo elf ;;
    cffaedfe | cefaedfe | feedfacf | feedface | cafebabe | bebafeca) echo macho ;;
    4d5a*) echo pe ;;
    *) echo unknown ;;
  esac
}

# The format that a program has to be to run on this system.
native_format() {
  case "$host_system" in
    Darwin) echo macho ;;
    Linux) echo elf ;;
    *) echo pe ;;
  esac
}

# Says (as a reason) what a file links that is not allowed.
check_links() {
  local file="$1" format libs lib tool name
  format="$(format_of "$file")"
  case "$format" in
    macho)
      if ! command -v otool >/dev/null 2>&1; then
        link_tool_missing "$format" otool
        return
      fi
      # Lines that end with a colon are the name of the file (and of each architecture of a universal one).
      libs="$(otool -L "$file" 2>/dev/null | sed '/:$/d; s/^[[:space:]]*//; s/ (compatibility.*$//')"
      while IFS= read -r lib; do
        [ -n "$lib" ] || continue
        case "$lib" in
          /usr/lib/* | /System/Library/*) ;;
          *) add "links $lib, which is not a library of the system" ;;
        esac
      done <<EOF
$libs
EOF
      ;;
    elf)
      if ! command -v readelf >/dev/null 2>&1; then
        link_tool_missing "$format" readelf
        return
      fi
      # The ffmpeg that ships is static: nothing is needed, whatever it is (even the C library).
      libs="$(readelf -d "$file" 2>/dev/null | sed -n 's/.*(NEEDED)[^[]*\[\(.*\)\]/\1/p')"
      while IFS= read -r lib; do
        [ -n "$lib" ] || continue
        add "links $lib (the ffmpeg for Linux has to be static: no shared library at all)"
      done <<EOF
$libs
EOF
      ;;
    pe)
      tool=""
      for name in x86_64-w64-mingw32-objdump objdump llvm-objdump; do
        if command -v "$name" >/dev/null 2>&1; then
          tool="$name"
          break
        fi
      done
      if [ -z "$tool" ]; then
        link_tool_missing "$format" objdump
        return
      fi
      libs="$("$tool" -p "$file" 2>/dev/null | sed -n 's/^[[:space:]]*DLL Name: *//p' | tr -d '\r')"
      while IFS= read -r lib; do
        [ -n "$lib" ] || continue
        name="$(printf '%s' "$lib" | tr 'A-Z' 'a-z')"
        name="${name%.dll}"
        case "$name" in
          api-ms-win-*) ;;
          *)
            case " $WINDOWS_DLLS " in
              *" $name "*) ;;
              *) add "links $lib, which is not a DLL of Windows" ;;
            esac
            ;;
        esac
      done <<EOF
$libs
EOF
      ;;
    *)
      echo "note: $file is not a program of a kind that this check can list the libraries of: what it links is not checked"
      ;;
  esac
}

# The tool that lists the libraries of a program is not here: for a program that this system runs that is a failure, for another
# (a Windows ffmpeg checked on Linux without mingw's objdump) it is said and the rest of the check goes on.
link_tool_missing() {
  if [ "$1" = "$(native_format)" ] && [ "$1" != pe ]; then
    add "what it links cannot be checked: $2 is not installed"
  else
    echo "note: what $file links is not checked: $2 is not installed"
  fi
}

bad=0
missing=0
for file in "$@"; do
  if [ ! -f "$file" ]; then
    echo "no such file: $file" >&2
    missing=1
    continue
  fi
  found=""

  # The bytes of the file.
  for option in "${BAD_OPTIONS[@]}"; do
    if has_bytes "$option" "$file"; then add "$option"; fi
  done
  if ! has_bytes 'LGPL version 2.1 or later' "$file"; then
    add "no 'LGPL version 2.1 or later' in it"
  fi
  # The code of x264 itself (the encoder, not the name that the H.264 decoder looks for), and of the other encoders that are GPL or
  # non-free.
  for symbol in x264_encoder_open x265_encoder_open xvid_encore aacEncOpen; do
    if has_bytes "$symbol" "$file"; then add "$symbol: code of a GPL or non-free encoder"; fi
  done

  # What the program says when it is asked (when it runs here: a Windows program built on Linux does not).
  if [ -x "$file" ] && conf="$("$file" -hide_banner -buildconf 2>/dev/null)"; then
    for option in "${BAD_OPTIONS[@]}"; do
      if printf '%s\n' "$conf" | grep -q -F -e "$option"; then add "$option"; fi
    done
    if license="$("$file" -hide_banner -L 2>&1)"; then
      case "$license" in
        *"GNU Lesser General Public"*) ;;
        *) add "'-L' does not say that it is the LGPL" ;;
      esac
      case "$license" in
        *"version 2.1 of the License"*) ;;
        *) add "'-L' does not say version 2.1 of the license" ;;
      esac
      case "$license" in
        *"version 3 of the License"*) add "'-L' says version 3 of the license" ;;
      esac
      case "$license" in
        *nonfree*) add "'-L' says nonfree" ;;
      esac
      case "$license" in
        *"GNU General Public"*) add "'-L' says GPL" ;;
      esac
    else
      add "'-L' did not run"
    fi
    # The names of the encoders: the second word of a line that starts with the six flags of one (`V....D h264_videotoolbox ...`).
    encoders="$("$file" -hide_banner -encoders 2>/dev/null | awk 'length($1) == 6 && NF >= 2 { print $2 }')"
    for encoder in $GPL_ENCODERS; do
      case "
$encoders
" in
        *"
$encoder
"*) add "it lists the encoder $encoder, which is GPL or non-free" ;;
      esac
    done
  else
    echo "note: $file cannot run here: it is checked by its bytes and what it links"
  fi

  check_links "$file"

  if [ -n "$found" ]; then
    echo "NOT a plain LGPL ffmpeg: $file: $(printf '%s' "$found" | sed 's/|/, /g')" >&2
    bad=1
  else
    echo "plain LGPL ffmpeg: $file"
  fi
done

if [ "$missing" -ne 0 ]; then
  exit 2
fi
exit "$bad"
