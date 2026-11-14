#!/usr/bin/env python3
"""Writes THIRD_PARTY_LICENSES.txt: the license texts of everything that an installer of Dictate carries.

Usage (from anywhere; it needs `cargo`, and `pnpm install` run for the interface):
    python3 scripts/generate-licenses.py            writes THIRD_PARTY_LICENSES.txt at the root of the repository
    python3 scripts/generate-licenses.py --check    writes nothing, and says whether the file is the one that this would write

What is in the file:
  1. The native components that the installers carry (FFmpeg and what is linked into it, whisper.cpp, the models, sherpa-onnx, ONNX
     Runtime): the texts in licenses/native, which are copied from the releases that Dictate is built with.
  2. Every Rust crate that the program of an installer is linked with, with the license files that the crate ships (LICENSE*, COPYING*,
     NOTICE*, so the copyright lines and the NOTICE of an Apache crate are there). `cargo tree -e normal` for each of the four systems of the
     installers says which crates those are (build scripts and tests are not shipped). A crate that ships no license file gets the standard
     text of its license from licenses/spdx, and its authors from Cargo.toml.
  3. The npm packages that the interface bundles: Svelte (with clsx and esm-env), @tauri-apps/api and @tauri-apps/plugin-opener.

Identical texts are written once, with the list of what uses them. The header has a digest of the third-party packages of Cargo.lock, which
app/ui/src/docs.test.ts compares with the real Cargo.lock: a dependency that changes makes the test fail until this script is run again.
"""
import hashlib
import json
import os
import re
import subprocess
import sys

ROOT = os.path.realpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
OUT = os.path.join(ROOT, "THIRD_PARTY_LICENSES.txt")
TARGETS = [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
]
# The programs of the installers: the app and the MCP server.
PACKAGES = ["dictate-app", "dictate-mcp"]
# What the bundle of the interface has in it (app/ui/package.json has the first two; the others are what the runtime of Svelte uses).
NPM_PACKAGES = ["svelte", "clsx", "esm-env", "@tauri-apps/api", "@tauri-apps/plugin-opener"]
LICENSE_FILE = re.compile(r"^(LICEN[CS]E|COPYING|NOTICE|UNLICENSE|COPYRIGHT)([._-].*)?$", re.IGNORECASE)
# Which license text stands in for a crate that has none of its own: the first of these that its expression names (or all of them for AND).
FALLBACK_ORDER = ["MIT", "Apache-2.0", "BSD-3-Clause", "BSD-2-Clause", "Zlib", "ISC", "Unlicense", "MPL-2.0", "BSL-1.0", "0BSD", "CC0-1.0"]

# The native components, in the order of the file: (title, what it is, the files of licenses/native).
NATIVE = [
    ("FFmpeg 9.0.2 (the program dictate-ffmpeg)", "LGPL-2.1-or-later: the text is COPYING.LGPLv2.1, next to this file. This is what FFmpeg says of the license of its files.", ["ffmpeg-LICENSE.md"]),
    ("libvpx 1.14.0 (inside dictate-ffmpeg on Linux)", "BSD-3-Clause, and the additional IP rights grant of the WebM project.", ["libvpx-LICENSE.txt", "libvpx-PATENTS.txt"]),
    ("libopus 1.4 (inside dictate-ffmpeg on Linux)", "BSD-3-Clause.", ["opus-COPYING.txt"]),
    ("zlib (inside dictate-ffmpeg on Linux)", "The zlib license.", ["zlib-LICENSE.txt"]),
    ("glibc 2.39 (inside dictate-ffmpeg on Linux)", "LGPL-2.1-or-later: the text is COPYING.LGPLv2.1, next to this file.", []),
    ("libgcc, the runtime of GCC 13 (inside dictate-ffmpeg on Linux and on Windows)", "GPL-3.0-or-later with the GCC Runtime Library Exception 3.1.", ["gcc-COPYING.RUNTIME.txt", "gcc-COPYING3.txt"]),
    ("mingw-w64 runtime and winpthreads (inside dictate-ffmpeg on Windows)", "Public domain and permissive licenses; the notices that the runtime asks for.", ["mingw-w64-runtime-COPYING.txt", "mingw-w64-COPYING.txt", "winpthreads-COPYING.txt"]),
    ("nv-codec-headers n12.1.14.0 (headers the Windows ffmpeg was compiled with)", "MIT.", ["nv-codec-headers-NOTICE.txt"]),
    ("AMF headers v1.5.3 (headers the Windows ffmpeg was compiled with)", "MIT.", ["amf-LICENSE.txt"]),
    ("whisper.cpp and ggml (linked into the program)", "MIT.", ["whisper.cpp-LICENSE.txt"]),
    ("OpenAI Whisper (the models that are downloaded)", "MIT.", ["openai-whisper-LICENSE.txt"]),
    ("pyannote segmentation 3.0 (the model that is downloaded)", "MIT.", ["pyannote-segmentation-3.0-LICENSE.txt"]),
    ("sherpa-onnx 1.13.8 (linked into the program, or libsherpa-onnx-c-api.so)", "Apache-2.0. The project has no NOTICE file.", ["sherpa-onnx-LICENSE.txt"]),
    ("ONNX Runtime 1.28.2 (inside the sherpa-onnx libraries, or libonnxruntime.so)", "MIT, and the notices of the third-party code that ONNX Runtime contains. The project has no NOTICE file.", ["onnxruntime-LICENSE.txt", "onnxruntime-ThirdPartyNotices.txt"]),
]

RULE = "=" * 100


def run(*command):
    return subprocess.run(command, cwd=ROOT, check=True, capture_output=True, text=True).stdout


def read(path):
    with open(path, "rb") as handle:
        return handle.read().decode("utf-8", errors="replace").replace("\r\n", "\n").strip("\n")


def lock_digest():
    """The digest of the third-party packages of Cargo.lock: `name version checksum` lines, sorted. Workspace crates have no source, so
    the version that release-please bumps in them does not change it."""
    text = read(os.path.join(ROOT, "Cargo.lock"))
    lines = []
    for block in text.split("[[package]]")[1:]:
        fields = dict(re.findall(r'^(\w+) = "([^"]*)"$', block, re.MULTILINE))
        if "source" in fields:
            lines.append(f"{fields['name']} {fields['version']} {fields.get('checksum', '')}")
    lines.sort()
    return hashlib.sha256("\n".join(lines).encode()).hexdigest()


def linked_crates():
    """(name, version) of every third-party crate in the normal dependencies of the programs, for each of the four systems."""
    found = set()
    for target in TARGETS:
        command = ["cargo", "tree", "-e", "normal", "--target", target, "--prefix", "none", "--locked", "--no-dedupe"]
        for package in PACKAGES:
            command += ["-p", package]
        for line in run(*command).splitlines():
            match = re.match(r"^(\S+) v(\S+)(.*)$", line)
            if not match:
                continue
            name, version, rest = match.groups()
            # A crate of the workspace has its path in parentheses; the ones from crates.io have nothing (a proc-macro or a repeat is marked).
            if "(/" in rest or "(\\" in rest or re.search(r"\([A-Za-z]:", rest):
                continue
            found.add((name, version))
    return sorted(found)


def package_table():
    metadata = json.loads(run("cargo", "metadata", "--format-version", "1", "--locked"))
    return {(p["name"], p["version"]): p for p in metadata["packages"]}


def license_files(directory, declared):
    """The license files of a crate or package: the ones at the top of its folder, and the one that its manifest names."""
    names = sorted(name for name in os.listdir(directory) if LICENSE_FILE.match(name) and os.path.isfile(os.path.join(directory, name)))
    if declared:
        path = os.path.normpath(os.path.join(directory, declared))
        if os.path.isfile(path) and os.path.basename(path) not in names and path.startswith(directory):
            names.append(os.path.relpath(path, directory))
    # A folder that holds them (LICENSES/, licenses/).
    for folder in ("LICENSES", "licenses", "LICENSE"):
        sub = os.path.join(directory, folder)
        if os.path.isdir(sub):
            names += [os.path.join(folder, name) for name in sorted(os.listdir(sub)) if os.path.isfile(os.path.join(sub, name))]
    return [(name, read(os.path.join(directory, name))) for name in names]


def standard_text(expression):
    """The standard texts (licenses/spdx) for a license expression: the first named of FALLBACK_ORDER for an OR, all of them for an AND."""
    ids = [token for token in re.split(r"\s+|\(|\)|/", expression or "") if token and token not in ("OR", "AND", "WITH")]
    have = [i for i in FALLBACK_ORDER if i in ids and os.path.isfile(os.path.join(ROOT, "licenses", "spdx", i + ".txt"))]
    if not have:
        return []
    chosen = have[:1] if re.search(r"\bOR\b|/", expression) else have
    return [(i, read(os.path.join(ROOT, "licenses", "spdx", i + ".txt"))) for i in chosen]


def group(entries):
    """entries: (label, [(file name, text)]) -> {text: [labels]}, in the order of first use."""
    texts = {}
    for label, files in entries:
        for _, text in files:
            texts.setdefault(text, [])
            if label not in texts[text]:
                texts[text].append(label)
    return texts


def render_groups(groups, title_of):
    out = []
    for text, users in sorted(groups.items(), key=lambda item: (item[1][0].lower(), item[0])):
        out.append(RULE)
        out.append(title_of(text))
        out.append(RULE)
        out.append("")
        out.append("Used by:")
        out += ["  " + user for user in users]
        out.append("")
        out.append(text)
        out.append("")
        out.append("")
    return "\n".join(out)


def first_line(text):
    for line in text.splitlines():
        if line.strip():
            return line.strip()[:90]
    return ""


def rust_section():
    table = package_table()
    entries = []
    synthesized = []
    for name, version in linked_crates():
        package = table[(name, version)]
        directory = os.path.dirname(package["manifest_path"])
        label = f"{name} {version}"
        files = license_files(directory, package.get("license_file"))
        if not files:
            standard = standard_text(package.get("license"))
            if not standard:
                sys.exit(f"{label}: no license file in {directory}, and no standard text for '{package.get('license')}' in licenses/spdx")
            authors = ", ".join(a.split("<")[0].strip() for a in package.get("authors") or [])
            # The label says what the text is: the crate declares the license, but its package has no file to copy the copyright lines from.
            label += f" (declares {package['license']}; no license file in its package" + (f"; authors: {authors}" if authors else "") + ")"
            files = standard
            synthesized.append(f"{name} {version}")
        entries.append((label, files))
    return entries, synthesized


def npm_section():
    ui = os.path.join(ROOT, "app", "ui", "node_modules")
    svelte = os.path.realpath(os.path.join(ui, "svelte"))
    entries = []
    for name in NPM_PACKAGES:
        direct = os.path.join(ui, *name.split("/"))
        directory = os.path.realpath(direct) if os.path.isdir(direct) else os.path.realpath(os.path.join(svelte, "..", name))
        if not os.path.isdir(directory):
            sys.exit(f"{name} is not installed: run `pnpm --dir app install` first")
        manifest = json.load(open(os.path.join(directory, "package.json")))
        label = f"{name} {manifest['version']}"
        files = license_files(directory, None)
        # plugin-opener ships only an SPDX file for its license ("Apache-2.0 OR MIT"): the texts are the ones of @tauri-apps/api, the same
        # project (the Tauri Programme within The Commons Conservancy).
        if name == "@tauri-apps/plugin-opener":
            api = os.path.realpath(os.path.join(ui, "@tauri-apps", "api"))
            files = [(n, f"[{label} declares Apache-2.0 OR MIT and ships only an SPDX file; these are the license files of @tauri-apps/api, from the same project]\n\n{t}") for n, t in license_files(api, None)]
        if not files:
            sys.exit(f"{label}: no license file in {directory}")
        entries.append((label, files))
    return entries


def build():
    out = []
    out.append(f"""Third-party license texts of Dictate
=====================================

This file has the license texts of the software that an installer of Dictate carries or that Dictate downloads, with the copyright lines
and the notices that the licenses ask to keep. The list of components, the written offer of the FFmpeg source and where each thing comes
from are in THIRD_PARTY_NOTICES.md, next to this file. Dictate itself is under the MIT license (LICENSE); the text of the LGPL 2.1, for FFmpeg
and glibc, is COPYING.LGPLv2.1.

This file is made by scripts/generate-licenses.py: do not edit it by hand.
Cargo.lock (third-party packages): sha256:{lock_digest()}

PART 1. Native components
PART 2. Rust crates
PART 3. npm packages (the interface)

""")
    out.append("\n" + RULE + "\nPART 1. NATIVE COMPONENTS\n" + RULE + "\n")
    for title, note, files in NATIVE:
        out.append(RULE + "\n" + title + "\n" + note + "\n" + RULE + "\n")
        for name in files:
            out.append("--- " + name + " ---\n\n" + read(os.path.join(ROOT, "licenses", "native", name)) + "\n\n")
    entries, synthesized = rust_section()
    out.append("\n" + RULE + f"\nPART 2. RUST CRATES ({len(entries)} crates linked into the programs)\n" + RULE + "\n")
    if synthesized:
        out.append(f"{len(synthesized)} crates ship no license file in the package that cargo downloads; the standard text of their license is used for them: " + ", ".join(synthesized) + "\n\n")
    out.append(render_groups(group(entries), lambda text: first_line(text)))
    npm = npm_section()
    out.append("\n" + RULE + "\nPART 3. NPM PACKAGES (the interface that is bundled in the program)\n" + RULE + "\n")
    out.append(render_groups(group(npm), lambda text: first_line(text)))
    return "\n".join(out).rstrip("\n") + "\n"


def main():
    text = build()
    if "--check" in sys.argv:
        current = read(OUT) + "\n" if os.path.isfile(OUT) else ""
        if current != text:
            print("THIRD_PARTY_LICENSES.txt is not what scripts/generate-licenses.py writes: run it again", file=sys.stderr)
            sys.exit(1)
        print("THIRD_PARTY_LICENSES.txt is up to date")
        return
    with open(OUT, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(text)
    print(f"wrote {os.path.relpath(OUT, ROOT)} ({len(text) // 1024} KiB)")


if __name__ == "__main__":
    main()
