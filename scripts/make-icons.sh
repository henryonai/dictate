#!/usr/bin/env bash
# Makes every icon file of the app from its drawings. The outputs are committed: this script only documents how they were made, and
# is needed again only when a drawing changes. CI does not run it.
#
# Usage: scripts/make-icons.sh
#
# The drawings are in app/src-tauri/icons/source/ (SVG, edit them, never the PNGs):
#
#   icons/source/loah.svg
#                       the app icon: a 1024 px canvas with the macOS icon grid (an 824 px tile, 185 px corners, 100 px of clear
#                       margin). Everything the bundle lists comes from it: icon.icns, icon.ico, 32x32.png, 64x64.png, 128x128.png
#                       (the one the READMEs show), 128x128@2x.png, icon.png and the Windows Store logos (Square*Logo.png, StoreLogo.png).
#   icons/source/loah-tray.svg
#                       the glyph in one color (currentColor, black when drawn on its own) for the menu bar of the Mac. It becomes
#                       icons/tray/tray-template.png (18 px) and tray-template@2x.png (36 px): a template image, which the menu bar
#                       paints dark or light to match itself. tray.rs shows the 36 px one, which the menu bar draws 18 points high.
#   icons/source/loah-tray-tile.svg
#                       the tray icon of Windows and Linux: a graphite tile with the blue bars and the near-white lines, which shows on
#                       a light and on a dark taskbar. It becomes icons/tray/tray.png (16 px) and tray@2x.png (32 px); tray.rs shows
#                       the 32 px one.
#
# How: the Tauri CLI draws an SVG itself (`tauri icon` takes a square PNG or SVG, and `-p SIZE` makes a PNG of just that size), so this
# needs nothing but `pnpm --dir app install` (Node and pnpm; no ImageMagick, no rsvg-convert). `tauri icon` also makes the folders of
# the iOS and Android icons, which the app has no use for: the files are made in a scratch folder and only the desktop ones are copied.
#
# After a change look at the PNGs at 16, 32, 128 and 1024 px (`pnpm --dir app tauri icon app/src-tauri/icons/source/loah.svg -p 16
# -p 32 -p 128 -p 1024 -o SOME_FOLDER`): the three bars and the three lines must stay apart at 16 px.
#
# The tray images are in the program: tray.rs includes them when it is compiled (build.rs watches the folder), so build again after
# running this.

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
icons="$root/app/src-tauri/icons"
drawings="$icons/source"

for drawing in loah.svg loah-tray.svg loah-tray-tile.svg; do
  [ -f "$drawings/$drawing" ] || { echo "missing: app/src-tauri/icons/source/$drawing" >&2; exit 1; }
done

scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT

# The CLI is chatty (it lists every file, with the iOS and Android ones): its output is shown only when it fails.
tauri_icon() {
  pnpm --dir "$root/app" tauri icon "$@" > "$scratch/tauri.log" 2>&1 || { cat "$scratch/tauri.log" >&2; return 1; }
}

# The app icon, in every format the bundle lists.
tauri_icon "$drawings/loah.svg" --output "$scratch/app"
find "$scratch/app" -maxdepth 1 -type f -exec cp {} "$icons/" \;

# The tray images. `-p` names each file after its size (18x18.png): they get their own names here.
mkdir -p "$icons/tray"
tauri_icon "$drawings/loah-tray.svg" -p 18 -p 36 --output "$scratch/template"
cp "$scratch/template/18x18.png" "$icons/tray/tray-template.png"
cp "$scratch/template/36x36.png" "$icons/tray/tray-template@2x.png"
tauri_icon "$drawings/loah-tray-tile.svg" -p 16 -p 32 --output "$scratch/tile"
cp "$scratch/tile/16x16.png" "$icons/tray/tray.png"
cp "$scratch/tile/32x32.png" "$icons/tray/tray@2x.png"

echo "Icons written to app/src-tauri/icons/ (and icons/tray/)."
