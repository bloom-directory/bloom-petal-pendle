# Pendle Petal

An account-scoped Pendle V2 Petal for Bloom. This first developer release
supports market discovery, portfolio reads, PT buys, PT sells and post-maturity
PT redemption on Arbitrum. It uses Pendle's V3 Convert API for unsigned
transactions and the public HTTP API for discovery and portfolios.

SDK and builder: the reviewed head of Petal PR [#42](https://github.com/bloom-directory/petal/pull/42),
commit `8501029631b4305a18084185a9bb6fb0daed2147`. This work does not merge the PR or follow later changes to its branch.

## Routes

All paths below are relative to `/petals/pendle/`:

| Path | Behavior |
| --- | --- |
| `status.json` | Supported features and exact SDK revision |
| `arbitrum/markets.json` | First page of up to 50 active Arbitrum markets (`total`, `skip`, `limit`, `results`) |
| `arbitrum/markets/<market>/info.json` | Market identities, maturity, accounting/underlying assets |
| `arbitrum/accounts/<wallet>/<index>/portfolio.json` | Pendle's indexed cross-chain positions for the selected account address |
| `arbitrum/<operation>/<wallet>/<index>/<id>/draft.json` | Write a new immutable quote; read the input schema |
| `arbitrum/<operation>/<wallet>/<index>/<id>/plan.md` | Review inputs, receiver, minima, expiry and transaction steps |
| `arbitrum/<operation>/<wallet>/<index>/<id>/confirm.json` | Write exactly `confirm` to advance one outbox step |
| `arbitrum/<operation>/<wallet>/<index>/<id>/status.json` | Reconcile existing outbox entries; never submit on read |

Operations are `buy-pt`, `sell-pt` and `redeem-pt`. Choose the wallet/account
through the path. The receiver is always that account; body overrides are
refused. IDs contain 1–64 ASCII letters, digits, hyphens or underscores.

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

Reads use `GET /core/v2/markets/all` with Arbitrum/active filters and bounded
pagination, `GET /core/v1/42161/markets/<market>`, and
`GET /core/v1/dashboard/positions/database/<account-address>`. Responses use
Pendle's REST JSON shape. Portfolio data is indexed and may lag chain state;
claimable rewards can be cached for up to 24 hours. It is not execution proof.

API reference: https://docs.pendle.finance/pendle-v2-dev/Backend/ApiOverview

## Transaction lifecycle

Creating a draft reads Pendle and chain data but neither stages nor signs.
It authenticates the market against official factories and checks its tokens
onchain. It independently decodes the returned ABI bytes, matching the router,
sender, receiver, market, assets, exact input and encoded minimum output.
It refuses external aggregators, limit-order fills, native assets, unsupported
selectors and noncanonical/trailing calldata.

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
signing authority or add installer provenance. A developer triad must enroll
the exact package through Bloom's own supported tooling. Current release
restrictions on custom-Petal provenance still apply; this package does not
bypass them or establish compatibility with an ordinary release install.

The earlier local-fork acceptance run used a modified developer Machine and
predates the direct-HTTP read implementation. It is historical evidence for
the transaction workflow, not acceptance evidence for this revision on stock
Bloom. The acceptance script remains available to rerun against an appropriately
enrolled developer environment.

The acceptance script uses an isolated local Arbitrum fork, disposable
Signer-created custody and software owner ceremonies. Public Arbitrum is read
only. Local fork storage mutations supply fixture balances; no real assets
are spent. Runtime state contains disposable custody data and stays outside
this package. Acceptance evidence must also be written outside the package
to avoid changing its hash while testing it.

## Deliberately outside this release

YT/LP execution, limit-order signing, Boros, external aggregators, cross-chain
execution, sPENDLE and automated strategies. No API key is required for the
tested public endpoints. Handle upstream failures by retaining the intent;
do not bypass Bloom or widen approval limits to recover.
