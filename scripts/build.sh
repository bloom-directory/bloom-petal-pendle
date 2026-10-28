#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PETAL_REV="7dd2722db5de305dd63ca4ed03591f3ab82fa40b"
if [[ -n "${PETAL_BIN:-}" ]]; then
  "$PETAL_BIN" build --root "$ROOT"
else
  tool_root="$ROOT/target/petal-tool"
  if [[ ! -x "$tool_root/bin/petal" ]]; then
    cargo install --git https://github.com/bloom-directory/petal --rev "$PETAL_REV" --locked --root "$tool_root" bloom-petal-cli
  fi
  "$tool_root/bin/petal" build --root "$ROOT"
fi
