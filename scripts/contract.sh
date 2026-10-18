#!/usr/bin/env bash
# Schema contract gate between the Rust markdown↔Yjs converter and the editor.
# Rust writes Yjs fixtures → the JS test feeds them through y-prosemirror with
# Milkdown's real schema and writes editor-made fixtures → Rust checks those.
# Run after any change to crates/kawruh-core/src/markdown or app/src/editor/schema.ts.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo test -q -p kawruh-core markdown
(cd app && pnpm -s vitest run tests/contract.test.ts)
cargo test -q -p kawruh-core cross_language
echo "schema contract OK"
