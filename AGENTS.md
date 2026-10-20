# pendle operating contract

Pendle V2 on Arbitrum. SDK/tooling are pinned to the original reviewed head of Petal PR #42 at
`8501029631b4305a18084185a9bb6fb0daed2147`. Do not merge that PR as part of
developing this package.

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
https://github.com/bloom-directory/petal/tree/8501029631b4305a18084185a9bb6fb0daed2147/skills/bloom-petal-development
Load it into your agent before extending this Petal.

