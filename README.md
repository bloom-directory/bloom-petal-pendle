# Pendle Petal

An account-scoped Pendle V2 Petal for Bloom. This first developer release
supports market discovery, portfolio reads, PT buys, PT sells and post-maturity
PT redemption on Arbitrum. It uses Pendle's V3 Convert API for unsigned
transactions and a bounded read-only MCP client for discovery/portfolios.

SDK and builder: the reviewed head of Petal PR [#42](https://github.com/bloom-directory/petal/pull/42),
commit `8501029631b4305a18084185a9bb6fb0daed2147`. This work does not merge the PR or follow later changes to its branch.

## Routes

All paths below are relative to `/petals/pendle/`:

| Path | Behavior |
| --- | --- |
| `status.json` | Supported features and exact SDK revision |
| `arbitrum/markets.json` | First 50 active markets from Pendle |
| `arbitrum/markets/<market>/info.json` | Market identities, maturity, accounting/underlying assets |
| `arbitrum/accounts/<wallet>/<index>/portfolio.json` | Pendle's cross-chain portfolio for the selected account address |
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

`scripts/dev-machine-sdk42.patch` records the isolated Machine changes used
for local integration: the #42 contract pin, lineage-only developer enrollment
(no signing operation classes), and a generic local-Petal launcher hook.
It applies to Bloom `caefe751baae789a82a0451ff3ee56971ddfe769`.
Broker is `d1ab181ec376af4fe5d5b8ea321cc285c9c75413`; Signer is
`bc88ad690757165ebf1978410ea5c7ec652215d3`. These are local developer changes,
not merged or production release pins.

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
