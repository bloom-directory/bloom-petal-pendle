# pendle operating contract

Pendle V2 on every chain in `Chain::ALL`: the 13 chains where Pendle has
deployed its router and a market factory. SDK and tooling are pinned to Petal commit
`7dd2722db5de305dd63ca4ed03591f3ab82fa40b`, where PR #45 merged into `master`.
Move every copy of the pin together: `route/Cargo.toml` (then
`cargo update --manifest-path route/Cargo.toml -p bloom-petal-sdk`),
`petal-build.toml`, `scripts/build.sh`, `SDK_REV` in `route/src/lib.rs`, the
report in `scripts/triad-acceptance.py`, the CLI install in
`.github/workflows/ci.yml`, the development skill below, and the README.

All account routes use adjacent `[wallet]/[index]` captures and
`petal::wallet_account(ctx)`. There is no account-zero fallback or body-supplied
sender/receiver. Keep route composition in `route/files/` and reusable ABI,
HTTP, persistence and transaction logic in focused `route/src/` modules.

Every route under `[chain]` resolves it with `route_chain` or `selected`, so an
unsupported chain is refused even by static listings. The exception is the
`[wallet]/$index.rs` listings, which stay `petal::account_listing!()` because
Bloom lists the accounts itself. Every chain read, stage, confirm, inspect,
Pendle URL and store key takes that `Chain`.
`Chain::id()` is the trust anchor: a live `eth_chainId` read only confirms it.
To add a chain:
- add a `Chain` variant with Bloom's chain name and its id, to `Chain::ALL`
  too. Use Bloom's name where Bloom ships the chain; otherwise pick the name
  an operator adds to `config.toml`;
- pin its factories in `evm::factories`: whichever of `marketFactory` and
  `marketFactoryV3` onwards Pendle's `deployments/<chain id>-core.json` has, in
  that order, and check the router address there. A chain with no router or
  market factory (Ink and Katana today) has nothing to trade;
- allow its `/core/v1/<id>/markets/*` and `/core/v3/sdk/<id>/convert` paths in
  `petal.toml`, and name it in the consent summary;
- record a market and quote fixture from the public API, add it to `recorded`
  in `route/tests/operations.rs`, and pin the chain's id and factories in the
  tests;
- add it to the README's Chains table, with its config entry if Bloom does not
  ship it, and add an entry to `CHAINS` in `scripts/triad-acceptance.py`.

Decode actual calldata, enforce exact inputs and minimum outputs, and refuse
unknown selectors, aggregator callbacks, embedded limit orders and native-token
routes. All signing and broadcasting belong to Bloom's transaction outbox.
Never add a local private key or direct Broker/Signer client.

Anyone can create a market through Pendle's factory, so `info.json`, drafts
and every confirm refuse a market that is not on Pendle's curated list
(`api::curated`, or `api::uncurated` where the refusal needs more advice).
That includes sells and redemptions: Pendle's router trusts the amount the
SY reports, so `min_output` does not protect an exit through a compromised
SY. A step Bloom may already have sent stays Bloom's to settle, since a
Petal cannot read receipts to tell whether it landed. Never rely on the
factory check alone, or on the market endpoint's 404 for an uncurated
market, which Pendle does not document.

The quote's five minutes are one clock for the whole operation, set when the
draft is written and never extended by a confirm. After expiry nothing new is
staged, and only a step whose transaction approval Bloom has already asked
for, or that Bloom may already have sent, continues. Each step usually takes
three confirm writes (stage, ask for the owner approval, send), and the second
is what lets a step outlive the quote, so only an approval asked for through
the Petal counts: one opened through Bloom's outbox leaves the step to stop
with the quote. Never tell an agent to send a Petal step through Bloom's
outbox instead, since that skips the Petal's rechecks. `plan.md` carries these
rules with the operation's own numbers; keep the README's "Steps, owner
approvals and the deadline" and the `confirm.json` read bodies agreeing with
it, and point to it rather than repeating it elsewhere.

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

