# pendle operating contract

Pendle V2 on Arbitrum. SDK and tooling are pinned to Petal commit
`7dd2722db5de305dd63ca4ed03591f3ab82fa40b`, where PR #45 merged into `master`.
Move every copy of the pin together: `route/Cargo.toml` (then
`cargo update --manifest-path route/Cargo.toml -p bloom-petal-sdk`),
`petal-build.toml`, `scripts/build.sh`, `SDK_REV` in `route/src/lib.rs`, the
report in `scripts/triad-acceptance.py`, the development skill below, and the
README.

All account routes use adjacent `[wallet]/[index]` captures and
`petal::wallet_account(ctx)`. There is no account-zero fallback or body-supplied
sender/receiver. Keep route composition in `route/files/` and reusable ABI,
HTTP, persistence and transaction logic in focused `route/src/` modules.

Decode actual calldata, enforce exact inputs and minimum outputs, and refuse
unknown selectors, aggregator callbacks, embedded limit orders and native-token
routes. All signing and broadcasting belong to Bloom's transaction outbox.
Never add a local private key or direct Broker/Signer client.

An accepted write, API quote or successful simulation is not execution proof.
Unknown staging/submission outcomes require reconciliation. Never generate a
new outbox entry to work around an ambiguous existing one.

Run `scripts/check.sh`. Integration uses a disposable local fork and software
ceremonies; these do not establish physical-passkey or production acceptance.
Never broadcast on mainnet or use real funds without explicit authorization.

## Development skill

Petals are authored against the `bloom-petal-development` skill. This Petal
carries it at `.agents/skills/bloom-petal-development/SKILL.md`, byte for byte
from the contract crate at the pinned Petal commit: the same file `petal new`
writes into a new scaffold. It is available offline and matches the pinned
tooling. Its directory carries the skill's name, as the Agent Skills format
requires. Load it into your agent before extending this Petal.

When the pin moves, replace it with the new commit's copy, from a Petal
checkout: `git show <rev>:skills/bloom-petal-development/SKILL.md`. It is part
of the Petal package, so editing or refreshing it changes the package hash;
Bloom does not serve it under `/petals/pendle/`.

