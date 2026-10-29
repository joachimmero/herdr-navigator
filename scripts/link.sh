#!/usr/bin/env bash
# Build the release binary, put it where the manifest's commands expect it, link the plugin, and
# put `herdr-md` on PATH.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

cargo build --release
mkdir -p bin "$HOME/.local/bin"
ln -sf ../target/release/herdr-md bin/herdr-md
ln -sf "$ROOT/bin/herdr-md" "$HOME/.local/bin/herdr-md"
herdr plugin link "$ROOT"
