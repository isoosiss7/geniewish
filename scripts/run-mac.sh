#!/bin/bash
set -euo pipefail

geniewish_root="$(cd "$(dirname "$0")/.." && pwd)"
geniewish_bundle="$geniewish_root/target/geniewish.app"
cd "$geniewish_root"
cargo build
mkdir -p "$geniewish_bundle/Contents/MacOS" "$geniewish_bundle/Contents/Resources"
printf '%s\n' "$geniewish_root" > "$geniewish_bundle/Contents/Resources/workspace.txt"
git rev-parse --short HEAD > "$geniewish_bundle/Contents/Resources/build.txt"
cp native/mac/Info.plist "$geniewish_bundle/Contents/Info.plist"
xcrun swiftc -swift-version 5 -warnings-as-errors native/mac/Diagnostics.swift native/mac/Voice.swift native/mac/main.swift \
    -o "$geniewish_bundle/Contents/MacOS/geniewish-mac"
cp target/debug/geniewish "$geniewish_bundle/Contents/MacOS/geniewish-core"
codesign --force --sign - --identifier org.geniewish.mac "$geniewish_bundle"

case "${1:-}" in
    --build-only) printf '%s\n' "$geniewish_bundle" ;;
    --diagnose|--self-test) "$geniewish_bundle/Contents/MacOS/geniewish-mac" "$1" ;;
    *) open "$geniewish_bundle" --args --workspace "$geniewish_root" "$@" ;;
esac
