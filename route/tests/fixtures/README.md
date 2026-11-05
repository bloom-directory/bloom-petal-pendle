Public unsigned Pendle API responses. The receiver of every quote is the public
test address for secp256k1 scalar 1. No signature, credential or user wallet
material is included.

- `market.json` and `quote.json`, recorded on 2026-10-19: Arbitrum,
  `POST /core/v3/sdk/42161/convert`, SY-sUSDai to its February 2027 PT, without
  an aggregator.
- `market-ethereum-reusd.json` and `quote-ethereum-reusd.json`, recorded on
  2026-10-29: Ethereum, `POST /core/v3/sdk/1/convert`, 1 reUSD to PT-reUSD
  (December 2026, 6 decimals), with `enableAggregator` and `useLimitOrder` off.
- `quote-ethereum-reusd-limit-fill.json`, recorded on 2026-10-29: the same
  swap for 500,000 reUSD with Pendle's default `useLimitOrder`, which embeds a
  LimitRouter flash fill. The Petal must refuse it.
- `quote-sell.json`, recorded on 2026-10-30: Arbitrum, 1 PT-sUSDai of the
  `market.json` market to SY-sUSDai, with `enableAggregator` and
  `useLimitOrder` off.
- `market-ethereum-srusde.json` and `quote-ethereum-srusde-redeem.json`,
  recorded on 2026-10-30: Ethereum, redeeming 1 matured PT-srUSDe (October
  2026) to srUSDe, with `enableAggregator` and `useLimitOrder` off.

The market fixtures keep only the fields `api::Market` reads. Quotes are the
full response.

These fixtures test decoding and rejection independently of live API changes.
They are not live settlement evidence.
