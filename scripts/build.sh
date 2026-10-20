#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PETAL_REV="8501029631b4305a18084185a9bb6fb0daed2147"
if [[ -n "${PETAL_BIN:-}" ]]; then
  "$PETAL_BIN" build --root "$ROOT"
else
  tool_root="$ROOT/target/petal-tool"
  if [[ ! -x "$tool_root/bin/petal" ]]; then
    cargo install --git https://github.com/bloom-directory/petal --rev "$PETAL_REV" --locked --root "$tool_root" bloom-petal-cli
  fi
  "$tool_root/bin/petal" build --root "$ROOT"
fi
