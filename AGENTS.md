# pendle operating contract

Pendle V2 on Arbitrum. SDK and tooling are pinned to Petal commit
`4fefa736df67fa1ee51aeaa0c749d2f11414257d`, where PR #42 merged into `master`.
Move every copy of the pin together: `route/Cargo.toml` (then
`cargo update --manifest-path route/Cargo.toml -p bloom-petal-sdk`),
`petal-build.toml`, `scripts/build.sh`, `SDK_REV` in `route/src/lib.rs`, the
report in `scripts/triad-acceptance.py`, and the links in this file and the README.

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

Petals are authored against the `bloom-petal-development` skill:
https://github.com/bloom-directory/petal/tree/4fefa736df67fa1ee51aeaa0c749d2f11414257d/skills/bloom-petal-development
Load it into your agent before extending this Petal.

