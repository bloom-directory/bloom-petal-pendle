# Pendle Petal

An account-scoped Pendle V2 Petal for Bloom. This developer release supports
market discovery, portfolio reads, PT buys, PT sells and post-maturity PT
redemption on Ethereum and Arbitrum. It uses Pendle's V3 Convert API for
unsigned transactions and the public HTTP API for discovery and portfolios.

SDK and builder: Petal commit `7dd2722db5de305dd63ca4ed03591f3ab82fa40b`, where
PR [#45](https://github.com/bloom-directory/petal/pull/45) merged into `master`.
The `bloom-petal-development` skill from that commit is in
`.agents/skills/bloom-petal-development/SKILL.md`.

## Routes

All paths below are relative to `/petals/pendle/`. `<chain>` is `ethereum` or
`arbitrum`, Bloom's names for chains 1 and 42161:

| Path | Behavior |
| --- | --- |
| `status.json` | Supported chains and features, and the exact SDK revision |
| `<chain>/markets.json` | Every active market on the chain, as a compact summary (see below) |
| `<chain>/markets/<market>/info.json` | Market identities, maturity, token symbols and decimals, accounting/underlying assets |
| `<chain>/accounts/<wallet>/<index>/portfolio.json` | Pendle's indexed positions for the selected account address on this chain |
| `<chain>/<operation>/<wallet>/<index>/<id>/draft.json` | Write a new immutable quote; read the input schema |
| `<chain>/<operation>/<wallet>/<index>/<id>/plan.md` | Review inputs, receiver, minima, expiry and transaction steps |
| `<chain>/<operation>/<wallet>/<index>/<id>/confirm.json` | Write exactly `confirm` to advance one outbox step |
| `<chain>/<operation>/<wallet>/<index>/<id>/status.json` | Reconcile existing outbox entries; never submit on read |

Operations are `buy-pt`, `sell-pt` and `redeem-pt`. Choose the chain and the
wallet/account through the path. The receiver is always that account; body
overrides are refused. IDs contain 1–64 ASCII letters, digits, hyphens or
underscores. Each chain keeps its own operations, so the same id can be used on
both chains and an operation is never visible from the other chain's paths.

`markets.json` returns `{chain, chainId, total, count, truncated, results}`.
Each result has the market `address`, `name`, `expiry`, the `pt`, `yt`, `sy`,
`underlyingAsset` and `accountingAsset` addresses, `inputTokens` and
`outputTokens` (what the SY accepts and pays out), `liquidityUsd` and
`impliedApy`. The Petal reads up to 400 markets in pages of 40; `truncated` is
`true` if Pendle listed more. Symbols and decimals are in `info.json`. Decimals
differ between markets: PT-reUSD on Ethereum has 6, for example. A zero address
in `inputTokens` or `outputTokens` is the native token (ETH), which some SYs
accept; drafts do not accept it yet.

Draft body (all amounts are decimal strings in base units):

```json
{
  "market": "0x...",
  "token": "0x...",
  "amount": "1000000000000000000",
  "min_output": "1000000000000000000",
  "slippage_bps": 10,
  "max_price_impact_bps": 100
}
```

`token` is the input asset for buys and output asset for sells/redemptions.
SY is supported directly. Other ERC-20s must be native inputs/outputs of that
market's SY wrapper. Slippage is 1–100 bps; price impact ceiling is 0–300 bps.
PT maturity/redemption is expressed in the accounting asset, which can differ
from the underlying asset. API APYs and expected amounts remain estimates.

## HTTP data sources

Reads use `GET /core/v2/markets/all` with chain and active filters and bounded
pagination, `GET /core/v1/<chain id>/markets/<market>`, and
`GET /core/v1/dashboard/positions/database/<account-address>`. The positions
endpoint covers every chain; the Petal keeps only the selected chain's entries.
Quotes come from `POST /core/v3/sdk/<chain id>/convert`. `petal.toml` allows
these paths for chain ids 1 and 42161 only. Portfolio data is indexed and may
lag chain state; claimable rewards can be cached for up to 24 hours. It is not
execution proof.

API reference: https://docs.pendle.finance/pendle-v2-dev/Backend/ApiOverview

## Transaction lifecycle

Creating a draft reads Pendle and chain data but neither stages nor signs.
It checks that the market belongs to the selected chain and that Bloom's RPC
for that chain reports the expected chain id. It then authenticates the market
against that chain's official factories, pinned from Pendle's
`deployments/<chain id>-core.json`, and checks its tokens onchain. It
independently decodes the returned ABI bytes, matching the router, sender,
receiver, market, assets, exact input and encoded minimum output. Quotes are
requested with Pendle's limit-order fills turned off. It refuses external
aggregators, limit-order fills, native assets, unsupported selectors and
noncanonical/trailing calldata. Every `confirm` repeats the chain, factory,
maturity and calldata checks before staging.

The wallet's policy must allow each transaction's target on that chain: the
Pendle router, the input token for a buy's approval, and the PT for a sell's
approval. Destinations are per chain, so Ethereum needs its own entries.

The plan contains exact-amount token approval steps only when allowance is
insufficient. A nonzero insufficient allowance is reset first. Each write to
`confirm.json` advances one step through Bloom's outbox. Complete the owner
approval shown by Bloom, then retry the same path. Status exposes the outbox
IDs so the owner can inspect pending approvals in the selected account.

Quotes expire after five minutes. Expiry does not cancel an already submitted
transaction or revoke an allowance. Reconcile previous steps before creating
a fresh intent. Lost stage replies are `staging_unknown` and never restaged.
A persisted writer lock after a crash requires operator inspection; locks are
not silently expired. The Petal exposes no force-retry or force-unlock route.

`execution_confirmed` means every step has a successful normalized Bloom
receipt. It does not independently prove a quoted APY or output balance.
There is no Pendle-specific Broker clear-signing reader in this release:
the normal owner review still presents contract-call bytes. The Petal plan
does not replace an independent Broker reader.

## Build and local integration

```sh
scripts/check.sh
# Against a running local developer Machine:
bloom petals build .
bloom petals install .
```

Set `PETAL_BIN` to a CLI built from the pinned SDK checkout to reuse that build.
Both the generated route workspace and route crate pin the same SDK SHA.
`route/Cargo.lock` is committed. All 35 route components must build and pass
capability checks; host-crate tests alone do not compile route controllers.

This repository contains no Bloom source patches. Installation does not grant
signing authority or add installer provenance. The Petal signs nothing itself,
so it declares no signing operation classes. On a developer triad,
`bloom petals install` is enough; Bloom's developer provenance enrollment is
for Petals that sign, and it refuses this one. Current release restrictions on
custom-Petal provenance still apply; this package does not bypass them or
establish compatibility with an ordinary release install.

Operations are stored under `operations/<chain>/<operation>/<id>.json`
(schema `bloom.pendle.operation.v2`). Builds before Ethereum support stored
them without the chain and cannot be read by this one. Finish or reconcile any
operation in flight with the build that created it before upgrading.

`scripts/triad-acceptance.py --chain <ethereum|arbitrum>` runs against an
isolated local fork of that chain, with disposable Signer-created custody and
software owner ceremonies. Ethereum buys PT-reUSD with reUSD and sells half
back; Arbitrum buys PT-sUSDai with SY-sUSDai. The public chains are read only.
Local fork storage mutations supply fixture balances; no real assets are spent.
Runtime state contains disposable custody data and stays outside this package.
Acceptance evidence must also be written outside the package to avoid changing
its hash while testing it.

## Deliberately outside this release

YT/LP execution, limit-order signing, Boros, external aggregators, native ETH
inputs, chains other than Ethereum and Arbitrum, cross-chain execution, sPENDLE
and automated strategies. No API key is required for the
tested public endpoints. Handle upstream failures by retaining the intent;
do not bypass Bloom or widen approval limits to recover.
