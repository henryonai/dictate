#!/usr/bin/env bash
# Regenerates tests/golden/expected.jsonl and expected-empty.jsonl: the replies of the macOS (Swift) dictate-mcp
# to tests/golden/requests.jsonl (against tests/golden/fixture) and tests/golden/requests-empty.jsonl (against an
# empty library folder). crates/dictate-mcp/tests/golden.rs checks the Rust dictate-mcp against these files, so
# this script is only needed on a Mac, when the requests, the fixture or the Swift server change.
#
# Needs: macOS with a Swift toolchain, jq, and the Swift repository (default: ../dictate next to this one;
# set DICTATE_SWIFT_REPO to use another checkout).
#
# How the Swift binary is built:
#   1. `swift build -c release --product dictate-mcp` in the Swift repository. That resolves every package of
#      Package.swift, WhisperKit included, so it needs the network (or a populated .build).
#   2. If that fails, or GOLDEN_SWIFT_BUILD=scratch is set: copy only Sources/DictateCore and Sources/DictateMCP
#      into a temporary package whose Package.swift has no WhisperKit and no Dictate app target (dictate-mcp
#      needs only DictateCore), and build that. Nothing in the Swift repository is touched.
#
# The server runs with TZ=Asia/Ho_Chi_Minh, so the dates in the replies do not depend on this machine's zone.
# Replies are written with sorted keys (one JSON object per line, as the server writes them): Swift orders the keys
# of an object differently on every run, and sorted keys keep a regenerated file diffable. The test compares
# parsed JSON, so key order does not matter there.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
golden="$root/tests/golden"
swift_repo=${DICTATE_SWIFT_REPO:-"$root/../dictate"}
zone=Asia/Ho_Chi_Minh

command -v jq >/dev/null || { echo "golden.sh: jq is required" >&2; exit 1; }
[ -d "$swift_repo/Sources/DictateCore" ] || { echo "golden.sh: no Swift repository at $swift_repo (set DICTATE_SWIFT_REPO)" >&2; exit 1; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

build_in_repository() {
    (cd "$swift_repo" && swift build -c release --product dictate-mcp) || return 1
    binary="$swift_repo/.build/release/dictate-mcp"
}

build_from_scratch_copy() {
    local package="$work/package"
    mkdir -p "$package/Sources"
    cp -R "$swift_repo/Sources/DictateCore" "$swift_repo/Sources/DictateMCP" "$package/Sources/"
    cat > "$package/Package.swift" <<'SWIFT'
// swift-tools-version:6.2
import PackageDescription

let package = Package(
    name: "GoldenReference",
    platforms: [.macOS(.v26)],
    products: [.executable(name: "dictate-mcp", targets: ["DictateMCP"])],
    targets: [
        .target(name: "DictateCore", path: "Sources/DictateCore"),
        .executableTarget(name: "DictateMCP", dependencies: ["DictateCore"], path: "Sources/DictateMCP"),
    ]
)
SWIFT
    (cd "$package" && swift build -c release --product dictate-mcp)
    binary="$package/.build/release/dictate-mcp"
}

binary=
if [ "${GOLDEN_SWIFT_BUILD:-}" = scratch ] || ! build_in_repository; then
    echo "golden.sh: building dictate-mcp from a copy without WhisperKit" >&2
    build_from_scratch_copy
fi
[ -x "$binary" ] || { echo "golden.sh: no dictate-mcp binary at $binary" >&2; exit 1; }

# $1: library folder, $2: requests file, $3: expected file.
replay() {
    DICTATE_MEETINGS_DIR=$1 TZ=$zone "$binary" < "$2" > "$work/raw.jsonl"
    jq -S -c . < "$work/raw.jsonl" > "$3"
    echo "golden.sh: wrote $(wc -l < "$3" | tr -d ' ') replies to ${3#"$root"/} from $(wc -l < "$2" | tr -d ' ') requests" >&2
}

replay "$golden/fixture" "$golden/requests.jsonl" "$golden/expected.jsonl"
empty_library="$work/empty-library"
mkdir "$empty_library"
replay "$empty_library" "$golden/requests-empty.jsonl" "$golden/expected-empty.jsonl"
