#!/usr/bin/env bash
# Build the release binary, put it where the manifest's commands expect it, link the plugin, and
# put `herdr-navigator` on PATH.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

cargo build --release
mkdir -p bin "$HOME/.local/bin"
ln -sf ../target/release/herdr-navigator bin/herdr-navigator
ln -sf "$ROOT/bin/herdr-navigator" "$HOME/.local/bin/herdr-navigator"
herdr plugin link "$ROOT"
