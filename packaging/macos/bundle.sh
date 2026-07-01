#!/usr/bin/env bash
# Build the release binary and wrap it in Jitter.app (menu-bar-only agent).
# Usage: packaging/macos/bundle.sh   (run from the repo root)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
APP="$ROOT/dist/Jitter.app"

cargo build --release --manifest-path "$ROOT/Cargo.toml"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp "$ROOT/packaging/macos/Info.plist" "$APP/Contents/Info.plist"
cp "$ROOT/target/release/jitter" "$APP/Contents/MacOS/jitter"
chmod +x "$APP/Contents/MacOS/jitter"

# Ad-hoc sign so it runs locally without "damaged app" errors. For distribution
# to other machines, replace with: codesign --deep --sign "Developer ID Application: ..."
# then notarize (xcrun notarytool submit).
codesign --force --deep --sign - "$APP"

echo "Built: $APP"
