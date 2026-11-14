#!/usr/bin/env bash
# Fails when a program or a library has code of espeak-ng in it.
#
# Usage: scripts/check-no-espeak.sh FILE...
#
# The speaker separation library (sherpa-onnx) comes with espeak-ng, which is GPL-3.0, for its speech synthesis, which Dictate does
# not use. A static link takes from an archive only what the program refers to, so espeak-ng stays out of a program that only
# separates speakers; this is the check that it does, on the files that are shipped. A shared library has all of its own code in
# it, whatever the program refers to, so the check is on it too.
#
# What it looks for, as plain bytes, so that it works the same on an ELF file, a Mach-O file and a Windows executable, and in
# one that is stripped:
#   ESPEAK_DATA_PATH  the variable espeak-ng reads to find its data: a string in its code, which a program keeps
#   espeak_ng_        the names of its functions, which a program that is not stripped keeps
# Both are in the espeak-ng archives that sherpa-onnx ships (libespeak-ng.a, espeak-ng.lib) and in its shared library for Linux,
# and in none of the programs that were checked when this was written. They are specific: the names of sherpa's own speaker
# functions (`OfflineSpeakerDiarization`) and a bare "espeak" in some text are not matches.
#
# Exit status: 0 when no file has any, 1 when a file has, 2 when it was not asked about a file that is there (a path that is
# misspelled must not make the check pass on nothing). The files that have some are named on stderr, with what was found in them.

set -u

if [ "$#" -eq 0 ]; then
  echo "usage: $0 FILE..." >&2
  exit 2
fi

found=0
missing=0
for file in "$@"; do
  if [ ! -f "$file" ]; then
    case "$file" in
      # The shell hands a pattern over as it is when no file matches it: what was meant to be there is not.
      *[\*\?\[]*) echo "no file matches $file: the libraries that should be there are missing" >&2 ;;
      *) echo "no such file: $file" >&2 ;;
    esac
    missing=1
    continue
  fi
  # What matched, for the log: the first match of each marker (the whole name of a function, not only the start of it).
  # LC_ALL=C: the bytes of a program are not text in any language, and a locale may refuse some of them.
  matched=""
  for pattern in 'ESPEAK_DATA_PATH' 'espeak_ng_[A-Za-z0-9_]*'; do
    hit="$(LC_ALL=C grep -a -o -m 1 -e "$pattern" -- "$file" | head -n 1 || true)"
    if [ -n "$hit" ]; then
      matched="${matched:+$matched, }$hit"
    fi
  done
  if [ -n "$matched" ]; then
    echo "espeak-ng (GPL-3.0) is in $file: found $matched" >&2
    found=1
  else
    echo "no espeak-ng in $file"
  fi
done

if [ "$missing" -ne 0 ]; then
  exit 2
fi
exit "$found"
