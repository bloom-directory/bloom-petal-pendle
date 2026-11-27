# Pendle Petal

An account-scoped Pendle V2 Petal for Bloom. This developer release supports
market discovery, portfolio reads, PT buys, PT sells and post-maturity PT
redemption on the 13 chains where Pendle has deployed its router and a market
factory. It uses Pendle's V3 Convert API for unsigned transactions and the
public HTTP API for discovery and portfolios.

SDK and builder: Petal commit `7dd2722db5de305dd63ca4ed03591f3ab82fa40b`, where
PR [#45](https://github.com/bloom-directory/petal/pull/45) merged into `master`.
The `bloom-petal-development` skill from that commit is in
`.agents/skills/bloom-petal-development/SKILL.md`.

## Chains

`<chain>` in the paths below is Bloom's name for the chain. The chain id is
pinned in the Petal, and Bloom must report it for that chain before any market
is trusted. Bloom asks its RPCs for the chain id once and keeps the answer, so
later reads may reach a different RPC in the chain's list; Bloom still signs
with the chain id in its config. List only RPCs you trust for that chain.
Market factories are pinned from Pendle's
`deployments/<chain id>-core.json`; chains Pendle reached later have only its
later factory versions.

| `<chain>` | Chain | Chain id | Pinned market factories | In Bloom's default config |
| --- | --- | --- | --- | --- |
| `ethereum` | Ethereum | 1 | `marketFactory`, V3–V6 | yes |
| `arbitrum` | Arbitrum One | 42161 | `marketFactory`, V3–V6 | yes |
| `base` | Base | 8453 | V5, V6 | yes |
| `bsc` | BNB Chain | 56 | `marketFactory`, V3–V6 | yes |
| `hyperliquid` | HyperEVM | 999 | V5, V6 | yes |
| `optimism` | OP Mainnet | 10 | `marketFactory`, V3–V6 | yes |
| `robinhood` | Robinhood Chain | 4663 | V6 | yes |
| `monad` | Monad | 143 | V6 | no |
| `sonic` | Sonic | 146 | V5, V6 | no |
| `mantle` | Mantle | 5000 | V3–V6 | no |
| `berachain` | Berachain | 80094 | V5, V6 | no |
| `xlayer` | X Layer | 196 | V6 | no |
| `plasma` | Plasma | 9745 | V5, V6 | no |

Pendle had no active markets on Optimism, Sonic, Mantle or Berachain on 23
November 2026; matured PT there can still be redeemed. Pendle's docs also list
Ink and Katana, but it has deployed no router or market factory on either,
only cross-chain PT contracts, so the Petal does not support them.

A chain that is not in Bloom's default config works once the operator adds it
to `~/.bloom/config.toml` under the name above and restarts Bloom. Until then,
`markets.json`, `info.json` and `portfolio.json` still read Pendle's API, but
drafts and confirms, which read the chain, fail with "`<chain>` chain read
failed: Bloom reported not found. If Bloom has no `<chain>` chain, add one with
chain_id <id> to its config.toml". For example:

```toml
[chains.monad]
name = "monad"
chain_id = 143
rpc_urls = ["https://rpc.monad.xyz"]
display_name = "Monad"
native_symbol = "MON"
native_decimals = 18
```

| `<chain>` | Native symbol | Public RPC | OP Stack |
| --- | --- | --- | --- |
| `monad` | MON | `https://rpc.monad.xyz` | no |
| `sonic` | S | `https://rpc.soniclabs.com` | no |
| `mantle` | MNT | `https://rpc.mantle.xyz` | yes: add `op_stack = true` |
| `berachain` | BERA | `https://rpc.berachain.com` | no |
| `xlayer` | OKB | `https://rpc.xlayer.tech` | yes: add `op_stack = true` |
| `plasma` | XPL | `https://rpc.plasma.to` | no |

Each RPC above reported its chain id on 23 November 2026. Mantle and X Layer
blocks carry OP Stack deposit transactions. Berachain blocks carry a type
`0x7e` system transaction that is not an OP Stack deposit. Choose RPCs you
trust. On 24 November 2026 a triad with these six entries, pointed at local
forks, staged, signed and mined Pendle transactions on each of them; see
acceptance below. Nothing has been broadcast to these chains' public networks.

## Routes

All paths below are relative to `/petals/pendle/`:

| Path | Behavior |
| --- | --- |
| `status.json` | Supported chains and features, and the exact SDK revision |
| `<chain>/markets.json` | Every active market on Pendle's curated list for the chain, as a compact summary (see below) |
| `<chain>/markets/<market>/info.json` | Market identities, maturity, token symbols and decimals, accounting/underlying assets, for a market on Pendle's curated list |
| `<chain>/accounts/<wallet>/<index>/portfolio.json` | Pendle's indexed positions for the selected account address on this chain |
| `<chain>/<operation>/<wallet>/<index>/<id>/draft.json` | Write a new immutable quote; read the input schema |
| `<chain>/<operation>/<wallet>/<index>/<id>/plan.md` | Review inputs, receiver, minima, expiry and transaction steps |
| `<chain>/<operation>/<wallet>/<index>/<id>/confirm.json` | Write exactly `confirm` to move the next outbox step on: stage it, ask for its owner approval, then send it |
| `<chain>/<operation>/<wallet>/<index>/<id>/status.json` | Reconcile existing outbox entries; never submit on read |

Operations are `buy-pt`, `sell-pt` and `redeem-pt`. Choose the chain and the
wallet/account through the path. The receiver is always that account; body
overrides are refused. IDs contain 1–64 ASCII letters, digits, hyphens or
underscores. Each chain keeps its own operations, so the same id can be used on
several chains and an operation is never visible from another chain's paths.

`markets.json` returns `{chain, chainId, total, count, truncated, results}`.
Each result has the market `address`, `name`, `expiry`, the `pt`, `yt`, `sy`,
`underlyingAsset` and `accountingAsset` addresses, `inputTokens` and
`outputTokens` (what the SY accepts and pays out), `liquidityUsd` and
`impliedApy`. The Petal reads up to 400 markets in pages of 40; `truncated` is
`true` when fewer distinct markets came back than Pendle's `total`. Symbols
and decimals are in `info.json`. Decimals differ between markets: PT-reUSD on
Ethereum has 6, for example. A zero address in `inputTokens` or `outputTokens`
is the native token (ETH), which some SYs accept; drafts do not accept it yet.

Take a draft's `market` only from `markets.json`, or, for a sell or a
redemption, from a market with a PT balance in `portfolio.json`: a matured
market leaves `markets.json`. Its `marketId` there is
`<chain id>-<market>`, and a matured position can be under `openPositions`
or `closedPositions`. Never take one from a link or a message.
Anyone can create a Pendle market through Pendle's factory, and a malicious
SY can keep what is deposited. The onchain factory check proves only that a
market came from that factory, not that Pendle reviewed it. So `info.json`,
a draft and every `confirm` look the market up in Pendle's curated list, and
refuse one that is not on it: `market <address> is not on Pendle's curated
list`. The lookup leaves out `isActive`, so a matured market can still be
redeemed. A market Pendle drops after an incident is refused at the next
`confirm`, which says to create a new reviewed intent for a listed market.
If that step's staged entry is unsent and its owner approval, if any, still
awaits its ceremony, the refusal also names the file that discards the
entry, which holds its nonce until then (see below). A failed or
inconsistent lookup is not Pendle's answer, so its error offers no
discard. There is no override: once Pendle drops a market, its PT cannot be
sold or redeemed through the Petal, even by an operation drafted before.
Pendle's router checks an exit's minimum output against the amount the
market's SY reports, so the minimum does not protect an exit through a
compromised SY. A new exit could not be drafted anyway: Pendle's own market
and quote endpoints refuse a market that is not on its curated list. A
holder must exit outside the Petal. Reading `draft.json` gives the same
guidance, with this chain's and account's paths.

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
market's SY wrapper: one of its `inputTokens` for a buy, one of its
`outputTokens` for a sell or redemption. A draft checks them against the SY's
own `getTokensIn()` or `getTokensOut()` onchain, and a refusal names the
tokens the SY accepts or pays out, leaving out the native token's zero
address, which the Petal refuses. A sell or redemption pays out nothing else,
so an exit can land in a different asset from the one the PT was bought with.
On Base, PT-yUTY's SY accepts USDC, UTY and yUTY but pays out only yUTY: a PT
bought with USDC sells back into yUTY, and getting USDC again takes a separate
swap.
Slippage is 1–100 bps; price impact ceiling is 0–300 bps.
The ceiling bounds the size of the impact Pendle reports, which is negative
for an adverse move. Pendle reports a few bps even for small trades, and
about a ten-millionth for a matured redemption, so a ceiling under about 5
bps refuses most trades, and 0 refuses any trade with a reported impact.
PT maturity/redemption is expressed in the accounting asset, which can differ
from the underlying asset. API APYs and expected amounts remain estimates.

## HTTP data sources

Reads use `GET /core/v2/markets/all` with chain and active filters and bounded
pagination, `GET /core/v1/<chain id>/markets/<market>`, and
`GET /core/v1/dashboard/positions/database/<account-address>`. `info.json`,
drafts and confirms check a market with
`GET /core/v2/markets/all?chainId=<chain id>&ids=<chain id>-<market>`, which
is empty for a market Pendle has not curated. The positions
endpoint covers every chain; the Petal keeps only the selected chain's entries.
Quotes come from `POST /core/v3/sdk/<chain id>/convert`. `petal.toml` allows
these paths for the 13 chain ids above only. Portfolio data is indexed and may
lag chain state; claimable rewards can be cached for up to 24 hours. It is not
execution proof.

API reference: https://docs.pendle.finance/pendle-v2-dev/Backend/ApiOverview

## Transaction lifecycle

Creating a draft reads Pendle and chain data but neither stages nor signs.
It first refuses a market that is not on Pendle's curated list. It then
checks that the market belongs to the selected chain and that Bloom's RPC
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
noncanonical/trailing calldata. Every `confirm` repeats the curated-list,
chain, factory, maturity and calldata checks before staging, and rebuilds
each approval step from the intent: the token being spent, the router, and
zero or the exact input.

The wallet's policy must allow this Petal's transactions on each chain it
trades on: add `{"chain": "<chain>", "destination": "petal:pendle"}` to
`allowed_destinations`, with the chain's name from the Chains table. Bloom
matches that entry only against transactions this Petal staged: its router
calls and token approvals. Bloom does not propose the entry, since Pendle is
not one of the Petals its setup installs: write the edited policy to
`/wallets/<wallet>/policy.json` and the owner approves it in a ceremony. Do
not list the router, the token or the PT instead. Bloom takes a contract
call's target as its destination, so any staged call to a listed contract
would pass, from another Petal or the agent's own raw transaction.

Read
[Steps, owner approvals and the deadline](#steps-owner-approvals-and-the-deadline)
before the first `confirm`: it gives the steps, their owner approvals and
their deadlines. Complete the owner approval shown by Bloom, then retry the
same path. Do not confirm the entry
through its own `confirm` file in Bloom's outbox: that sends it without the
Petal's curated-list, factory, maturity and allowance rechecks. Before 0.5.1,
Bloom's agent guide and the `retry_path` in the entry's
`approval_challenge.json` said to do exactly that
([bloom#476](https://github.com/bloom-directory/bloom/issues/476)). From
0.5.1, they send the retry back through `petals/pendle/` instead
([bloom#479](https://github.com/bloom-directory/bloom/pull/479)). Writing
`cancel` there to discard an entry is a different matter (see below). Status
exposes the outbox IDs so the owner can inspect pending approvals in the
selected account.
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
derives the operation's `state` from its steps. It is `prepared` when the
next `confirm` stages a step, `staged` when it asks for that step's owner
approval or sends it, `approval_required` while an
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
rechecking the market, even one Pendle has since dropped. A Petal cannot
read receipts, so it cannot tell a transaction that landed from one the
network never saw, and only Bloom can settle either. So a buy Bloom may
already have sent can still land after Pendle drops its market. Only a
transaction approval is cleared: a
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

Lost stage replies are `staging_unknown` and never restaged. When Bloom
staged the entry but the Petal could not claim it, status shows the outbox id
and the next `confirm` finishes the claim. A persisted writer lock after a
crash requires operator inspection; locks are not silently expired. The Petal
exposes no force-retry or force-unlock route. What happens when the quote
expires is in
[Steps, owner approvals and the deadline](#steps-owner-approvals-and-the-deadline).

`execution_confirmed` means every step has a successful normalized Bloom
receipt. It does not independently prove a quoted APY or output balance.
There is no Pendle-specific Broker clear-signing reader in this release:
the normal owner review still presents contract-call bytes. The Petal plan
does not replace an independent Broker reader.

### Steps, owner approvals and the deadline

A draft decides its steps from the router's mined allowance for the token
being spent: the input token for a buy, the PT for a sell or redemption.

| Router's allowance when the draft is written | Steps | Owner approvals |
| --- | --- | --- |
| covers the amount | `pendle_execution` | 1 |
| zero | `approve_input`, `pendle_execution` | 2 |
| nonzero but short | `reset_allowance`, `approve_input`, `pendle_execution` | 3 |

Each step is one outbox entry and one owner ceremony. The first trade after
an install that changes this package's hash also needs a wallet-policy
ceremony (see below). `plan.md` lists the steps and gives each ceremony's
deadline as a UTC time. Before the first ceremony, tell the owner how many
prompts to expect and by when.

Each step usually takes three writes of `confirm` to the Petal's
`confirm.json`. The first stages the entry. The second asks Bloom to send it,
and Bloom answers with the owner approval it needs, which status shows as the
step's `approval`, of `kind` `transaction`. The third sends it after the
ceremony. A ceremony that lapses needs the second write again. Write the
second straight after the first: an approval asked for that way is what lets
the step outlive the quote. Ask for approvals and send only through the
Petal's `confirm.json`. The Petal records only the approvals asked for there:
the owner can approve a ceremony opened through the entry's outbox `confirm`
file or `bloom wallet confirm`, and the Petal still refuses the step once the
quote expires.

The quote expires five minutes after the draft is written, and those five
minutes are one budget for the whole operation: confirming a step does not
extend them. After expiry nothing new is staged, and only a step whose
transaction approval Bloom has already asked for, or that Bloom may already
have sent, continues. A step is staged only once the one before it has mined.
So before the quote expires, every step but the last must be approved and
mined, and the last must be staged with its approval asked for; only the last
ceremony can finish later. Each earlier ceremony therefore has its own
deadline: `plan.md` leaves a minute, or two on Ethereum, for each later step
to be sent and mined and the next staged. With three steps, the first
ceremony is due two minutes before expiry and the second one minute before
(four and two on Ethereum). The quote's deadline is the operation's
`expires_ms` in `status.json`. A step's `approval.expires_ms` is the
ceremony's own: Bloom sets it up to ten minutes out and shows it on the
ceremony page, so a ceremony finished in time for its page can still miss the
quote.

Before drafting, read the wallet policy at `/wallets/<wallet>/policy.json`.
If `allowed_petal_packages` does not list this package's hash (`bloom petals
ls` shows its first 12 characters for `petals/pendle/`), or an update is
pending under `/wallets/<wallet>/policy-updates/pending/`, the first step's
second write asks for the policy update instead of its transaction approval.
Count that ceremony too. Bloom hands back any pending update, even an
unrelated one such as the owner's own edit: have the owner approve it only if
it adds this package's hash and nothing they did not intend, and otherwise
cancel it, as [Transaction lifecycle](#transaction-lifecycle) describes. Then
write `confirm` again, before the first ceremony's deadline, to ask for the
step's transaction approval: a step that has asked only for the policy update
stops with the quote.

If the quote expires first:

- A step whose transaction approval Bloom has asked for can still go out:
  complete its ceremony and write `confirm` to the Petal's `confirm.json`
  again. If the ceremony lapsed, that write asks Bloom for a fresh one for the
  same entry. When the step is an approval or a reset, the operation stops
  after it, since the execution is never staged, and the allowance that step
  set stays.
- Otherwise `confirm` refuses with `quote expired`, and status shows
  `expired`. Expiry does not cancel a submitted transaction or revoke an
  allowance. Reconcile the earlier steps in `status.json` first, then:
  - If the next step was never staged, nothing holds the account's nonce:
    create a new intent. Its draft sees the allowance already set, so it
    needs fewer steps.
  - A staged step's unsent entry holds the account's nonce, blocking its
    later transactions, and Bloom hands it to an identical new intent. When
    the refusal names the file that discards it,
    `/wallets/<wallet>/<index>/chains/<chain>/outbox/pending/<id>/confirm`,
    write `cancel` there, then create a new intent.
  - When the refusal names none, Bloom may be signing the entry, for example
    after a ceremony opened outside the Petal, and refuses to discard it.
    Keep reading `status.json`, and create a new intent only once the step
    is `failed`: Bloom expired it unsent, by default an hour after staging,
    and keeps an entry it may have signed until that is settled. If the step
    becomes `submitted`, `broadcast_unknown` or `confirmed`, check its
    transaction hash on chain first: the trade may already have gone out.
  - Never send the step through Bloom's outbox instead. That skips the
    Petal's curated-list, factory, maturity and allowance rechecks, and
    Pendle may have dropped the market since the quote.

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

`scripts/triad-acceptance.py --chain <chain>` runs against an isolated local
fork of that chain, with disposable Signer-created custody and software owner
ceremonies. Ethereum buys PT-reUSD with reUSD, sells half back, then redeems
PT-srUSDe, which matured on 22 October 2026, for srUSDe. Arbitrum, Base, BNB
Chain, HyperEVM, Robinhood Chain, Monad, X Layer and Plasma buy PT with the
market's SY and sell half back. Optimism, Sonic, Mantle and Berachain have no
active market, so they redeem a matured PT for its SY. Every run also checks
that `info.json` refuses `0x3e510adb77ddb7a5c079c60e94711a20208bd588`, an
Arbitrum market Pendle has not curated, which no chain's curated list holds.
The public chains are read only.

Each run creates a wallet, and the Broker allows four anonymous registrations
per five minutes, so start runs at least 80 seconds apart. A fork pins the
block it started at; Robinhood Chain's public RPC stops serving that block's
state within the hour, so start its fork just before its run. Base's
tokenized-stock SYs, such as GOOGLc's, price through a Base precompile that
Anvil cannot run, so Base trades PT-yUTY.

What a fork run does not show: only Ethereum pays with a token other than SY,
so routes through an SY's input and output tokens are exercised on Ethereum
alone. Anvil mines its own blocks with its own fee rules, so a run does not
reproduce a chain's real fees, such as Monad charging the full gas limit or
Mantle's fee model, or real OP Stack receipts. Sells and redemptions ask for a
minimum output of 1, so their floor checks show only that the encoded minimum
was met.
Local fork storage mutations supply fixture balances; no real assets are spent.
Runtime state contains disposable custody data and stays outside this package.
Acceptance evidence must also be written outside the package to avoid changing
its hash while testing it.

## Deliberately outside this release

YT/LP execution, limit-order signing, Boros, external aggregators, native ETH
inputs, Ink and Katana, cross-chain execution, sPENDLE
and automated strategies. No API key is required for the
tested public endpoints. Handle upstream failures by retaining the intent;
do not bypass Bloom or widen approval limits to recover.
