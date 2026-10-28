#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PETAL_REV="4fefa736df67fa1ee51aeaa0c749d2f11414257d"
if [[ -n "${PETAL_BIN:-}" ]]; then
  "$PETAL_BIN" build --root "$ROOT"
else
  tool_root="$ROOT/target/petal-tool"
  if [[ ! -x "$tool_root/bin/petal" ]]; then
    cargo install --git https://github.com/bloom-directory/petal --rev "$PETAL_REV" --locked --root "$tool_root" bloom-petal-cli
  fi
  "$tool_root/bin/petal" build --root "$ROOT"
fi
