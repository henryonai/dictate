# Loah: rename, new icon, Graphite theme and a calmer meetings window

Status: draft for the owner's approval · 2026-11-14 · builds on v2.0.0 (main `c53f072`)

## Why

Dictate 2.0.0 is out, but the name only describes half the app and is hard to find: Microsoft had an Office add-in called Dictate, Apple's feature is Dictation, and GitHub has about a thousand "dictate" repositories. Most of the owner's time in the app is now spent on meetings, not dictation. Very few people have 2.0.0 installed, so this is the cheapest moment to rename.

The meetings window works but makes the owner do too much reading: the video takes a big block even for short or audio-only recordings, the summary is a dense column where only the chapters link back to the recording, and short recordings leave a large empty panel. The theme is clean but has three unrelated colors (blue accent, red Record, coral logo) and lights the current transcript paragraph in the warning color.

Research behind this document (comparable tools, naming checks, mockups and sketches): https://claude.ai/artifact/LDEea8eBA45Ce4KUokWL3o

## Goals

1. The app, repository, installers, command line and MCP server are called **Loah**.
2. A new icon: **idea A, "voice becomes lines"** — sound bars on the left turning into lines of text on the right — in **blue on graphite**, plus a one-color tray glyph.
3. The **Graphite** theme: today's neutral look with the fixes listed below, and a Theme setting.
4. The meetings window changes **1, 2, 3, 5 and 8** below.
5. Nobody loses a recording, a setting or an API key, and an MCP setup that pointed at Dictate keeps working or tells the user exactly what to change.

## Non-goals (this round)

- Action items as a checklist with owners (change 4), chapter marks on the scrubber (change 6), asking across all recordings and a global action-item list (change 7). They come in a later round.
- Code signing, notarization, auto-update.
- Changing the meetings library format or the MCP tool names, arguments and output text.

## Anti-goals (failure even if it "works")

- A user opens Loah after updating and their meetings list is empty, their settings are back to defaults, or their API key is gone.
- Claude (or another MCP client) silently loses the meeting tools after the update.
- The rename leaves "Dictate" in visible places (window titles, tray menu, installer, notifications, README) — except where the text explains the rename.
- The theme change makes Vietnamese text render worse.

## Constraints

Unchanged from the project: Tauri 2.11.x, Rust MSRV 1.85, MIT, no GPL shipped (LGPL ffmpeg sidecar), English + Vietnamese interface with key parity, Conventional Commits with release-please, no AI attribution in commits, no local absolute paths in the repo, unsigned builds with the existing Gatekeeper/SmartScreen guidance. Platforms: macOS 13+ (Apple silicon and Intel), Windows 10/11 x64, Linux x64 .deb (Ubuntu 24.04+).

## 1. The rename

### Decided by the owner
- Name: **Loah**.

### Builder's defaults (the owner may override)
- Repository: rename `henryonai/dictate` → `henryonai/loah` on GitHub (GitHub redirects old links and clones). This is an outward action: the controller asks the owner before doing it, after the code is merged.
- App identifier: `io.github.henryonai.loah`. Product name `Loah`. Installers `Loah_<version>_aarch64.dmg`, `_x64.dmg`, `_x64-setup.exe`, `_amd64.deb` (deb package name `loah`).
- Programs: `loah` (app, with `--toggle` and `--record`), `loah-mcp`, `loah-ffmpeg`. Install paths follow the existing pattern: `/Applications/Loah.app/Contents/MacOS/loah-mcp`, `%LOCALAPPDATA%\Loah\loah-mcp.exe`, `/usr/bin/loah-mcp`.
- MCP server name `loah`; the `INSTRUCTIONS` text says Loah. Tool names, arguments and output structure stay identical; the only text changes are the app name in the instructions, in two tool descriptions ("List the user's Loah recordings", "…ticked them off in Loah") and in the get_meeting line "Action items as ticked off in Loah (x/y done):" (ruled during planning: "no Dictate left" wins over byte-identical text for these three strings).
- The word "dictate" as the name of the dictation feature stays where it is an identifier (tray item id, the Linux GlobalShortcuts portal id `dictate` — renaming it would lose the user's portal binding — and internal function names).
- Version: the rename ships as **2.1.0** (a `feat:`), not 3.0.0, because every old path keeps working through the migration below. The release notes and README lead with the rename.
- Environment variables: `LOAH_MEETINGS_DIR` and `LOAH_FFMPEG`; the old `DICTATE_MEETINGS_DIR` and `DICTATE_FFMPEG` are still read when the new ones are unset.

### Migration (first launch of Loah, and every launch of loah-mcp)
- **Settings and data folders**: if the Loah folder does not exist and the Dictate folder does, **copy only the items Dictate 2.0.0 wrote** (not `permission-guide-shown`: Loah has a new bundle id without permissions, so the guide must show again) — `settings.json`, `whisper-models/`, `diarization-models/`, `Dictations/` — into the Loah folder (never move or delete the old ones). Not the whole folder: on macOS it also holds the Swift app's `Models`/`Recordings`, and on Windows `%LOCALAPPDATA%\Dictate` is the old program's install folder (ruled during planning). Model files are hard-linked when on the same volume, copied otherwise.
- **Meetings library**: if `Loah Meetings` does not exist in Documents and `Dictate Meetings` does, keep using `Dictate Meetings` where it is (no move). New installs use `Loah Meetings`. loah-mcp applies exactly the same rule (it shares the library code), so the app and the MCP server always see the same folder.
- **API keys**: on first read, if the credential store has no Loah entry, read the Dictate entry, save it under Loah, and leave the old entry in place.
- **Swift Dictate** history stays as today (its settings and keys are not imported; its library folder is the same `Dictate Meetings`).
- Each migration step is logged once ("Copied your settings from Dictate") and failures fall back to the old location with a visible note in Settings, never to empty defaults.

### MCP for existing users
- README "Upgrading from Dictate" section: the new command path per OS and the one-line `claude mcp add` to re-point it. Keeping the client-side key `dictate` keeps the tool names the user's prompts already use.
- On macOS the old `/Applications/Dictate.app` is not touched by the Loah installer; the README tells the user to delete it after re-pointing MCP. (Builder may add a check in Settings → General that warns when an old Dictate MCP path is still configured in Claude's config file, if it can be read without touching keys — optional.)

### Old Dictate still installed (builder's defaults, marked as the designer's call)
- **Linux:** the `loah` deb declares `Replaces: dictate`, `Conflicts: dictate` and `Provides: dictate`, so installing it removes the old package, and it ships `/usr/bin/dictate-mcp` and `/usr/bin/dictate` as two-line `exec` wrapper scripts for `loah-mcp` and `loah` (wrappers instead of symlinks, ruled during planning: the Tauri deb `files` map copies files, and a wrapper behaves the same for MCP stdio) for the 2.x series, so MCP clients set up for Dictate keep working. The symlinks are removed in a later major release, announced in the release notes.
- **Windows:** Loah installs beside Dictate (a per-user NSIS installer cannot safely remove another product). `%LOCALAPPDATA%\Dictate\dictate-mcp.exe` keeps working until the user uninstalls Dictate. The README upgrade steps say: re-point MCP, then uninstall Dictate from Installed apps.
- **macOS:** as above, `/Applications/Dictate.app` is left alone; the README says re-point MCP, then delete it.
- **Both running:** on launch, Loah checks whether an old Dictate (the Tauri 2.0.0 app, by its identifier `io.github.henryonai.dictate`, or the Swift app `local.dictate`) is running or registered to start at login. If so, Settings → General shows one notice: "Dictate is still installed and uses the same shortcut and meetings folder. Quit it and remove it, then re-point your MCP client to Loah," with a button that opens the README section. Loah never quits, unregisters or deletes the old app itself.
- **Start at login:** when the copied settings say `launch_at_login: true`, Loah registers itself as a login item during migration (the setting is otherwise only applied when it changes).

### Where the name appears
Everything user-visible: window titles, tray menu and tooltip, notifications, Settings text, error messages, installer metadata, `.desktop` file, NSIS Start-menu entry, both READMEs, CONTRIBUTING, SECURITY, THIRD_PARTY_NOTICES, issue templates, release workflow names and asset names, release-please config. Crate names (`dictate-core`, `dictate-services`, `dictate-mcp`, `dictate-app`) may be renamed to `loah-*` — builder's call; renaming them is preferred if it does not make the change unreviewable.

## 2. The icon

- Idea A from the research page: three sound bars on the left, three lines of text on the right, on a rounded square.
- Colors: graphite background (about `#232327`), sound bars in the accent blue (about `#4c9aff`), text lines in near-white (about `#e9e9ee`).
- Deliverables: a master SVG; the full Tauri icon set generated from it (`icon.icns`, `icon.ico`, PNGs, Windows Store logos); a one-color tray/menu-bar glyph (macOS template image, Windows and Linux tray icons) that reads at 16 px; the icon at 16, 32, 128 and 1024 px checked by eye for legibility.
- The README header shows the new icon.

## 3. The Graphite theme

### Palette (tokens in `app/ui/src/tokens.css`, imported by every window; light and dark)
| Token | Dark | Light |
|---|---|---|
| bg | `#1c1c1f` | `#f5f5f7` |
| panel | `#212124` | `#eeeef1` |
| line | `#36363c` | `#d9d9de` |
| ink | `#ececef` | `#1d1d1f` |
| muted | `#9a9aa3` | `#6e6e73` |
| accent | `#4c9aff` | `#0a64d6` |
| on-accent | `#0b1626` | `#ffffff` |
| selected | `#232f42` | `#e3eefc` |
| current paragraph tint | accent at 13 % | `#eaf2fd` |
| timestamp | `#8cbcff` | `#0a64d6` |

Record stays red (it means "live"); danger/ok/warn keep their meaning. Exact values may be tuned for contrast (WCAG AA for text) — builder's call.

### Rules
- **One accent**: selected recording, current paragraph, main buttons, focus rings.
- **Current paragraph**: a 2 px accent bar on its left and the faint tint; the warning color is no longer used for it.
- **Reading**: summary text 14 px, line height about 1.55, lines no wider than about 75 characters, a little more space between bullets.
- **Three button weights**: filled for the one main action in a place (Record, Ask), outlined for common actions (Copy summary, Export), plain text for the rest.
- **Timestamps** in a monospaced face with tabular digits.
- **Containers**: the transcript and the summary columns use the same kind of container; no framed box running empty to the bottom of the window.
- **Font**: Be Vietnam Pro (SIL OFL 1.1) bundled with the app (no network), regular/medium/semibold, with the system font as fallback. Its license goes into THIRD_PARTY_NOTICES and the license-texts file.
- **Theme setting**: Settings → General → Theme: System (default), Light, Dark. Saved in settings; applies at once to every window (meetings, settings, the dictation panel and card, the recording HUD and picker).

## 4. The meetings window

### Change 1 — summary lines jump to their moment
- Key points, decisions, action items and open questions show a timestamp chip when the line has one; clicking it seeks the player and scrolls the transcript, like chapters do now.
- New summaries: the summary prompt asks for one `[mm:ss]` per item (the moment it is based on), written at the end of the item's text. Existing summaries without times show unchanged; "Summarize again" gives them times.
- The time is part of the item's text in the summary Markdown (content, not a new format): the headings and the bullet structure stay exactly as they are. MCP `get_meeting` and `list_action_items` therefore return items with a trailing `[mm:ss]` for new summaries — useful to assistants, whose instructions already ask them to cite `[mm:ss]`. The action-item parser must keep the owner and the text split correctly when a trailing time is present (the time stays in the text, never in the owner); tests cover items with and without a time. Copy summary and Export keep the times. (Designer's call; the Swift app is being replaced, so its display of the raw time is accepted.)
- Q&A answers that contain `[mm:ss]` are clickable the same way (they may already be — keep it).

### Change 2 — a player sized to the recording
- Audio-only recordings: a slim player bar (play/pause, position, time, speed) — no black video box. A waveform is welcome if cheap; builder's call.
- Video: a smaller default height (about a third of the column), with **Enlarge** and **Hide video** controls; the choice is remembered across recordings.

### Change 3 — tabs in the right column
- Tabs: **Summary · Actions (n) · Chapters (n) · Ask**. The count is hidden when zero.
- Summary tab opens on a short overview: the summary paragraph clamped to about three lines with "Show more", then Key points, Decisions, Open questions.
- The last tab chosen is remembered. Q&A moves into the Ask tab (no longer pinned under the summary).
- Copy summary and Export stay in the recording header.

### Change 5 — a tighter list
- Titles on one line with ellipsis; the full title on hover (and for screen readers).
- Small badges on the second line: duration, then state — `failed`, `not summarized`, `processing`, or `N actions`. Error rows keep "click for how to fix" in the badge's tooltip and open the existing error card.

### Change 8 — short recordings and narrow windows
- The transcript fills its column; no empty framed area under a short transcript.
- A recording shorter than about a minute, or with very little speech, shows one quiet line in the Summary tab: "Short recording — there isn't much to summarize."
- Below about 1100 px the list folds away behind the existing Hide list button (open by choice, as an overlay or by pushing); the layout test in `layout.test.ts` is updated for the new widths.

## What is left to the builder
- Exact pixel sizes, spacing, contrast tuning and animation.
- Whether to rename the crates.
- How large model files are migrated (copy vs. hard link), within the "old folder stays usable" rule.
- Waveform in the audio player.
- The optional old-MCP-path warning.

## Acceptance
- Fresh install on each OS: shows Loah everywhere, new icon in Dock/taskbar/tray, Graphite theme, Theme setting works.
- Update from Dictate 2.0.0 on macOS with an existing library, settings and keys: Loah shows the same meetings, settings and keys; nothing in the old folders is deleted.
- Linux: installing the Loah deb over the Dictate deb removes the old package and `/usr/bin/dictate-mcp` still answers (symlink). Windows and macOS: with Dictate still installed, Loah shows the "Dictate is still installed" notice.
- loah-mcp lists the same five tools with byte-identical output and finds the same library as the app (old and new folder cases).
- The release workflow builds and checks Loah installers; `check-artifact.sh` expects the new names.
- Existing tests updated; new tests for: each migration step (copy, keep-library, key copy, failure fallback), env var fallback, theme setting, summary timestamp chips, player modes, tabs, list badges, short-recording note, narrow layout.
