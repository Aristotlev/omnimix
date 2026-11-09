#!/bin/bash
# OmniMix for macOS: one-shot build. Run from the repo root: bash build-mac.sh
set -euo pipefail
cd "$(dirname "$0")"
echo "== OmniMix macOS build =="
sw_vers

if ! xcode-select -p >/dev/null 2>&1; then
  echo "Installing Xcode Command Line Tools. A dialog opens. Run this script again when it finishes."
  xcode-select --install || true
  exit 1
fi

[ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"
if ! command -v cargo >/dev/null 2>&1; then
  echo "Installing Rust (rustup, user-local)..."
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
  source "$HOME/.cargo/env"
fi

if ! command -v npm >/dev/null 2>&1; then
  export NVM_DIR="$HOME/.nvm"
  [ -s "$NVM_DIR/nvm.sh" ] && . "$NVM_DIR/nvm.sh"
fi
if ! command -v npm >/dev/null 2>&1; then
  echo "Node.js is required. Install it with: brew install node   (or: nvm install --lts)"
  exit 1
fi

export MACOSX_DEPLOYMENT_TARGET=14.4
if [ -f "$HOME/.tauri/omnimix.key" ]; then
  export TAURI_SIGNING_PRIVATE_KEY="$(cat "$HOME/.tauri/omnimix.key")"
  export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$(cat "$HOME/.tauri/omnimix.key.password" 2>/dev/null || true)"
  (cd app && npm install --no-audit --no-fund && npx tauri build)
else
  # Local build without the release key: skip signed update artifacts.
  (cd app && npm install --no-audit --no-fund && npx tauri build --config '{"bundle":{"createUpdaterArtifacts":false}}')
fi
cargo build --release -p omnimix-probe

OUT="$PWD/dist-mac"
mkdir -p "$OUT"
rm -rf "$OUT/OmniMix.app"
cp -R target/release/bundle/macos/OmniMix.app "$OUT/"
cp target/release/omnimix-probe "$OUT/"
hdiutil create -volname OmniMix -srcfolder "$OUT/OmniMix.app" -ov -format UDZO "$OUT/OmniMix-macos.dmg" >/dev/null
echo
echo "Done:"
echo "  $OUT/OmniMix.app"
echo "  $OUT/OmniMix-macos.dmg"
echo "  $OUT/omnimix-probe   (run: $OUT/omnimix-probe list)"
open "$OUT"
