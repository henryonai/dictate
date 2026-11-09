#!/usr/bin/env bash
# Regenerates the media files in crates/dictate-services/tests/fixtures/media/ that the decoder tests
# (crates/dictate-services/src/media/decode.rs) read. The outputs are committed: this script only documents how they were
# made, and is needed again only when a fixture has to change.
#
# Every audio file is the same clip: 1 second of a 440 Hz sine at half scale, so that a test can check the length and
# count the zero crossings (880) whatever the codec. Each file is well under 60 KB.
#
#   tone.wav          PCM, 8 bit, 44.1 kHz, mono (16-bit would be 88 KB)
#   tone.mp3          MP3 (LAME), 44.1 kHz, mono, 32 kbit/s
#   tone.m4a          AAC-LC in MP4, 48 kHz, mono, 48 kbit/s
#   tone.mp4          H.264 video (64x48, 10 fps, a test pattern) and the same AAC audio
#   tone.webm         Opus in WebM, 48 kHz, mono, 24 kbit/s (symphonia has no Opus decoder: opus-decoder reads it)
#   tone.opus         Opus in Ogg, the same audio (voice notes of chat apps)
#   tone-16k.webm     Opus in WebM that was encoded from a 16 kHz source: the file says 16 kHz, but Opus decodes to 48 kHz
#   video-only.mp4    the video of tone.mp4 with no audio track at all
#   tone-ima4.mov     IMA ADPCM (ima4) in QuickTime, 22.05 kHz, mono: a codec that symphonia does not know at all (its
#                     track has no codec parameters), so the file is not "no audio" but only ffmpeg can read it
#   tone-ac3.mkv      AC-3 in Matroska: a codec that symphonia cannot decode, so only ffmpeg can read it
#   two-voices.wav    25.6 s of speech, 16 kHz mono 16-bit, for the speaker separation test (diarize.rs): two voices of
#                     macOS's `say` (Samantha, Daniel) take turns, four sentences, so the speakers change at about
#                     5.9, 13.5 and 19.1 s. Made only where `say` exists (macOS); the voices differ a little between
#                     macOS versions, which moves the turns by a few hundredths of a second.
#
# Needs: ffmpeg with libmp3lame, libopus and libx264 (macOS: brew install ffmpeg). A tone is made by ffmpeg itself, so
# neither `say` nor `afconvert` is needed for it; only two-voices.wav needs `say`. The metadata is stripped and the
# muxers write no version tag, so that a regenerated file is the same bytes for the same ffmpeg.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
out="$root/crates/dictate-services/tests/fixtures/media"

command -v ffmpeg >/dev/null || {
    echo "ffmpeg is not installed (macOS: brew install ffmpeg)" >&2
    exit 1
}

mkdir -p "$out"
cd "$out"

# The inputs come first and the options of the output after them (an option before an input is an option of the input).
ff() {
    ffmpeg -y -hide_banner -loglevel error "$@"
}

# 1 s of 440 Hz at 48 kHz (the sine source is at 1/8 of full scale: `volume=4` makes it half scale), and 1 s of a test pattern.
tone=(-f lavfi -i "sine=frequency=440:sample_rate=48000:duration=1")
video=(-f lavfi -i "testsrc=size=64x48:rate=10:duration=1")
plain=(-map_metadata -1 -fflags +bitexact -flags:a +bitexact -flags:v +bitexact)
half=(-af "volume=4" -ac 1)
h264=(-c:v libx264 -preset veryslow -crf 40 -pix_fmt yuv420p)

ff "${tone[@]}" "${plain[@]}" "${half[@]}" -ar 44100 -c:a pcm_u8 tone.wav
ff "${tone[@]}" "${plain[@]}" "${half[@]}" -ar 44100 -c:a libmp3lame -b:a 32k tone.mp3
ff "${tone[@]}" "${plain[@]}" "${half[@]}" -ar 48000 -c:a aac -b:a 48k tone.m4a
ff "${video[@]}" "${tone[@]}" "${plain[@]}" "${half[@]}" -map 0:v -map 1:a "${h264[@]}" -c:a aac -b:a 48k -ar 48000 \
    -shortest tone.mp4
ff "${tone[@]}" "${plain[@]}" "${half[@]}" -ar 48000 -c:a libopus -b:a 24k tone.webm
ff "${tone[@]}" "${plain[@]}" "${half[@]}" -ar 48000 -c:a libopus -b:a 24k tone.opus
ff "${tone[@]}" "${plain[@]}" "${half[@]}" -ar 16000 -c:a libopus -b:a 24k tone-16k.webm
ff "${video[@]}" "${plain[@]}" -an "${h264[@]}" video-only.mp4
ff "${tone[@]}" "${plain[@]}" "${half[@]}" -ar 22050 -c:a adpcm_ima_qt tone-ima4.mov
ff "${tone[@]}" "${plain[@]}" "${half[@]}" -ar 48000 -c:a ac3 -b:a 32k tone-ac3.mkv

# Speech for the speaker separation test: each sentence is spoken to its own 16 kHz file, and ffmpeg joins them.
if command -v say >/dev/null; then
    voices=$(mktemp -d)
    trap 'rm -rf "$voices"' EXIT
    speak() {
        say -v "$1" --file-format=WAVE --data-format=LEI16@16000 -o "$voices/$2.wav" "$3"
    }
    speak Samantha 1 "Good morning everyone, today we will review the quarterly budget and decide on the hiring plan for next year."
    speak Daniel 2 "Thanks, I think we should freeze hiring until the second quarter, because revenue is behind the forecast by about ten percent."
    speak Samantha 3 "That makes sense, but the support team is overloaded, can we at least hire two people for support?"
    speak Daniel 4 "Fine, two support hires, and we revisit everything else in April. Let's write that down as a decision."
    ff -i "$voices/1.wav" -i "$voices/2.wav" -i "$voices/3.wav" -i "$voices/4.wav" \
        -filter_complex "concat=n=4:v=0:a=1" "${plain[@]}" -ar 16000 -ac 1 -c:a pcm_s16le two-voices.wav
else
    echo "say is not installed (macOS only): two-voices.wav is left as it is" >&2
fi

ls -l "$out"
