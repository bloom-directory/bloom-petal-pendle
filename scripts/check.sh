#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cargo fmt --manifest-path "$ROOT/route/Cargo.toml" -- --check
cargo test --manifest-path "$ROOT/route/Cargo.toml" --locked
cargo clippy --manifest-path "$ROOT/route/Cargo.toml" --all-targets --locked -- -D warnings
bash "$ROOT/scripts/check-route-architecture.sh"
bash "$ROOT/scripts/build.sh"
petal_rev="$(sed -n 's/^PETAL_REV="\(.*\)"$/\1/p' "$ROOT/scripts/build.sh")"
"${PETAL_BIN:-$ROOT/target/petal-tool/$petal_rev/bin/petal}" check --root "$ROOT"
