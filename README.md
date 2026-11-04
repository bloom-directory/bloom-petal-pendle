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
`true` when fewer distinct markets came back than Pendle's `total`. Symbols
and decimals are in `info.json`. Decimals differ between markets: PT-reUSD on
Ethereum has 6, for example. A zero address in `inputTokens` or `outputTokens`
is the native token (ETH), which some SYs accept; drafts do not accept it yet.

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
The ceiling bounds the size of the impact Pendle reports, which is negative
for an adverse move. Pendle reports a few bps even for small trades, and
about a ten-millionth for a matured redemption, so a ceiling under about 5
bps refuses most trades, and 0 refuses any trade with a reported impact.
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
receiver, market, assets, exact input and encoded minimum output. The encoded
minimum must be at least `min_output` and at least
`floor(expected × (10000 − slippage_bps) / 10000)`, which is how Pendle
encodes it. The expected output is Pendle's estimate, so `min_output` is the
one floor that doesn't come from Pendle: set it yourself. Quotes are
requested with Pendle's limit-order fills turned off. It refuses external
aggregators, limit-order fills, native assets, unsupported selectors and
noncanonical/trailing calldata. Every `confirm` repeats the chain, factory,
maturity and calldata checks before staging, and rebuilds each approval step
from the intent: the token being spent, the router, and zero or the exact
input.

The wallet's policy must allow each transaction's target on that chain: the
Pendle router, the input token for a buy's approval, and the PT for a sell's
or redemption's approval. Destinations are per chain, so Ethereum needs its
own entries.

The plan contains exact-amount token approval steps only when allowance is
insufficient. A nonzero insufficient allowance is reset first. Each write to
`confirm.json` advances one step through Bloom's outbox. Complete the owner
approval shown by Bloom, then retry the same path. Status exposes the outbox
IDs so the owner can inspect pending approvals in the selected account.
When Bloom refuses a confirm, the error carries Bloom's own reason, such as
a nonce gap naming the entry that holds it, or a simulation revert. URLs,
which may carry a provider key, and control and invisible formatting
characters are removed, and advice the Petal cannot follow (writing
`override`, restaging with an explicit nonce, discarding an entry that may
be signed) is replaced. When no pending entry holds the account's next
nonce, nothing can fill the gap: the error says the entry can never be
sent, and names the file that discards it while Bloom cannot be signing it.
The
Petal SDK reduces a reply that says "denied", "not found", "invalid" or
"permission" to a bare status, as it does for a policy refusal or a refused
approval. Then the error says what the step's own outbox entry shows: Bloom
discarded it unsent, with the policy checks that refused it, kept it after
a denial, or gave no reason the Petal can show. A refusal for a step Bloom
may already have sent says so: check its hash on chain before creating a
new intent.
`status.json` is not cached: every read reconciles with the outbox and
derives the operation's `state` from its steps. It is `prepared` or `staged`
when the next `confirm` stages or sends a step, `approval_required` while an
owner approval is pending, `submitted` while a transaction is in flight and
`expired` once the quote has expired and no step can continue.

While an outbox entry is pending, Bloom returns it instead of staging
another for the same package, route, sender, target, value and calldata.
Two drafts of the same intent, approvals of the same token and amount, and
every allowance reset have the same calldata. So each operation claims the
entries it stages, by outbox id and creation time, since ids repeat after
Bloom restarts, and only for an entry that holds the step's own
transaction. When Bloom returns an entry another operation holds,
`confirm` stages nothing for this one and says what to do: finish that
operation, or discard the entry if Bloom cannot have sent or signed it;
finish that operation if the owner already approved it; wait for Bloom to
expire the entry if its approval has ended; confirm that operation's route
if it may have been sent; or check its hashes on chain if Bloom sent a
replacement or cancel for it, or will not resend a lost reply.

Discarding means writing `cancel` to the entry's `confirm` file in Bloom's
outbox. Bloom 0.4.1 discards whatever is written there, and keeps its
signing state private, so the Petal names that file only for an entry with
no send attempt whose owner approval, if any, still awaits its ceremony
(`approval_challenge.json` in state `awaiting_ceremony`). Later Bloom
refuses to discard an entry that may have been signed.

Operations that spend the same token share the router's allowance, and one
exact approval replaces another, so run them one at a time. `confirm`
refuses to stage or send a Pendle transaction the mined allowance cannot
cover, which would revert. It does not count a transaction still in flight,
so a swap confirmed while another on the same token waits to mine can
revert. After a refusal, discard the staged swap if the refusal says so,
then create a new intent: an entry left pending holds its nonce, and Bloom
stages later transactions behind it. The Broker also allows one live
ceremony per wallet, and at most 12 new ones in five minutes, whatever the
token: a transaction ceremony it refuses to open ends the step unsent, while
a refused wallet-policy ceremony leaves the entry pending.

When Bloom asks for an approval, the step's `approval` gives its action id,
expiry and `kind`. A `transaction` approval is in the step's outbox entry. It
is `unknown` when the Petal cannot read the entry's intent. A
`wallet_policy` approval is the wallet's pending policy update, which must
list this package's hash in `allowed_petal_packages`. Bloom hands back any
pending update, even one the package does not need, such as the owner's own
edit, and a Petal can read neither the update nor the wallet policy. The
approval shows the package hash as `package`: check on the ceremony page
that the update adds it and nothing you did not intend. To drop an update
instead, cancel it and wait a few seconds before confirming: the Broker
refuses a new ceremony for a short cooldown after a cancel, and Bloom then
discards the entry. A cancel made on the ceremony page can instead leave
the entry pending, with the next confirm denied while Bloom catches up;
confirm again. Once Bloom has asked for a step's transaction approval, a
wallet-policy update it hands back does not replace it: `confirm` names the
update and records nothing, so the transaction approval still lets the step
continue after the quote expires. After an install that changes the hash,
the first `confirm` of a step staged after it asks for the update, under
`/wallets/<wallet>/policy-updates/pending/<action_id>/`. Staging does not.
Bloom lets only the build that staged an entry inspect or confirm it, so
finish operations before installing another build, or reinstall the build
that staged them.

Bloom 0.4.1 moves an entry it expires or refuses under policy, and a send it
cancels, to `outbox/failed/` and keeps reporting the status it had, so the
Petal checks that directory on every read instead of trusting the status. An
unsent entry that is cancelled reports `cancelled`. An entry there
with no transaction hash, no broadcast marker and no cancel was never sent:
the step and the operation become `failed` and `confirm` refuses it.
Reconcile earlier steps, whose approvals stay granted to the router, and
create a new intent. Bloom writes a broadcast marker before it submits a
transaction and never removes it, and a lost submit reply leaves the hash
only there. An entry there with a hash, a marker or a cancel may have gone
out, as may one Bloom reports `failed` or `cancelled` that is not in
`failed/`. The step and the operation become `broadcast_unknown`, showing the
transaction's hash and its cancel's (`cancel_tx_hash`) when Bloom recorded
them, and `confirm` refuses it. Check both on chain before creating a new
intent: Bloom 0.4.1 never looks for a receipt for an entry in `failed/`, so
even a cancelled send may have mined. Later Bloom keeps a cancelled send in
its sent queue, and the step `submitted`, until one of the two mines; a mined
cancel makes the step `failed`. Bloom 0.4.1 also leaves a send the owner
sped up with `replace` in its sent queue, and can leave a cancelled send
there when moving it fails, and follows only the original hash. So a sent
entry with a replacement (`replacement_tx_hash`,
`replacement_broadcast_attempted.json`) or a cancel (`cancel_tx_hash`,
`cancel_broadcast_attempted.json`) is `broadcast_unknown`, showing the
hashes, until Bloom records a receipt. Later Bloom follows both: a fee
repair becomes the entry's own hash and a cancel reports `cancelling`, so
those steps stay `submitted`. An entry later Bloom reports
`signing_unknown`, released by an older Bloom although it may have been
signed, is `broadcast_unknown`.

A lost submit reply can also leave the broadcast marker in a pending entry.
The step stays `staged`, showing the marker's hash, asks for no ceremony and
never expires: the next `confirm` lets Bloom settle that attempt, without
rechecking the market. Only a transaction approval is cleared: a
wallet-policy approval Bloom asks for to settle it stays shown. When Bloom
could not settle the attempt, it writes why in `reconcile_ambiguous.json`.
It resends the same signed transaction only after a failed public resubmit,
so then the step also stays `staged` and the next `confirm` lets it try
again. For any other reason (the account's nonce moved past it, another
transaction holds the nonce, a private relay, or a policy now asking for
private routing) Bloom refuses every confirm until the hash shows on chain,
so the step is `broadcast_unknown`. So is one whose reason cannot be read. A
marker with no readable hash, or a same-nonce replacement or cancel recorded
in the pending entry (`replacement_tx_hash`,
`replacement_broadcast_attempted.json`, `cancel_tx_hash`,
`cancel_broadcast_attempted.json`), makes the step `broadcast_unknown`
instead: check the hashes on chain.

Bloom reports the first entry it finds under an id, in `pending/`, then
`sent/`, then `failed/`. When that entry's creation time is not the step's,
the step reads its own entry in `sent/` or `failed/` instead, and is
`broadcast_unknown` if it finds neither. When Bloom refuses to report the
id because another account's entry now holds it, the step reads its own
entry the same way.

`plan.md` reads the input and output decimals from the tokens onchain. Token
symbols and the maturity date are Pendle's, printed without line breaks or
markup.

Quotes expire after five minutes. After that, `confirm` stages nothing new,
and only a step whose transaction approval Bloom already requested, or that
Bloom may already have sent, can continue, so a ceremony that outlasts the
quote doesn't leave an approved entry stuck in the outbox. If that ceremony
lapsed, Bloom may open a fresh one for the same entry. A step that has only
asked for the wallet-policy approval stops with the quote, and status shows
`expired`. Expiry does not cancel a submitted transaction or revoke an
allowance. An expired operation's unsent entry holds its nonce, blocking the
account's later transactions, and Bloom hands it to an identical new intent
until Bloom expires it an hour after staging. When `confirm`'s refusal names
it, discard it by writing `cancel` to
`/wallets/<wallet>/<index>/chains/<chain>/outbox/pending/<id>/confirm`; when
it does not, Bloom may have signed it, so the Petal names no discard. Later
Bloom refuses to discard an entry that may have been signed. Reconcile
previous steps before creating a fresh intent. Lost stage replies are
`staging_unknown` and never restaged. When Bloom staged the entry but the
Petal could not claim it, status shows the outbox id and the next `confirm`
finishes the claim. A persisted writer lock after a crash requires operator
inspection; locks are not silently expired. The Petal exposes no force-retry
or force-unlock route.

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
software owner ceremonies. Ethereum buys PT-reUSD with reUSD, sells half
back, then redeems PT-srUSDe, which matured on 22 October 2026, for srUSDe;
Arbitrum buys PT-sUSDai with SY-sUSDai and sells half back. The public chains are read only.
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
