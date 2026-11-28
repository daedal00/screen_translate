#!/bin/sh
# Fails if any crate in the workspace pulls in tauri (or any tauri-plugin-*) transitively.
set -eu
cd "$(dirname "$0")/.."
out=$(cargo tree --workspace --edges normal,build,dev 2>&1) || { echo "$out"; exit 2; }
if echo "$out" | grep -qi tauri; then
  echo "FAIL: the workspace depends on tauri:"; echo "$out" | grep -i tauri; exit 1
fi
echo "ok: no tauri in the workspace dependency tree"
