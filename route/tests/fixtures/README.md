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
- `curated-arbitrum-susdai.json` and `curated-arbitrum-usdai-lookalike.json`,
  recorded on 2026-11-10: Arbitrum, `GET /core/v2/markets/all?chainId=42161`
  with `ids=42161-<market>`, for the `market.json` market and for
  `0x3e510adb77ddb7a5c079c60e94711a20208bd588`. The second market comes from
  Pendle's factory, but Pendle has not curated it, so the list is empty. It is
  a "USDai" market on another SY than the curated USDai market's.

One market and one quote for each other chain, recorded on 2026-11-23 with
`enableAggregator` and `useLimitOrder` off and 0.1% slippage. Where Pendle had
active markets, the most liquid one, buying PT with 1 SY:

- `market-base-googlc.json`, `quote-base-googlc.json`: Base, PT-GOOGLc (December 2026).
- `market-bsc-clisbnb.json`, `quote-bsc-clisbnb.json`: BNB Chain, PT-clisBNB (April 2027).
- `market-hyperliquid-khype.json`, `quote-hyperliquid-khype.json`: HyperEVM, PT-kHYPE (March 2027).
- `market-robinhood-pfe.json`, `quote-robinhood-pfe.json`: Robinhood Chain, PT-PFE (December 2026).
- `market-monad-susde.json`, `quote-monad-susde.json`: Monad, PT-sUSDE (January 2027).
- `market-xlayer-usdg.json`, `quote-xlayer-usdg.json`: X Layer, PT-USDG (February 2027, 6 decimals).
- `market-plasma-susde.json`, `quote-plasma-susde.json`: Plasma, PT-sUSDE (January 2027).

Where Pendle had none, the most recently matured market, redeeming 1 PT to SY:

- `market-optimism-wsteth.json`, `quote-optimism-wsteth-redeem.json`: OP Mainnet, PT-wstETH (September 2024).
- `market-sonic-x33.json`, `quote-sonic-x33-redeem.json`: Sonic, PT-x33 (February 2026).
- `market-mantle-cmeth.json`, `quote-mantle-cmeth-redeem.json`: Mantle, PT-cmETH (September 2025).
- `market-berachain-ibgt.json`, `quote-berachain-ibgt-redeem.json`: Berachain, PT-iBGT (December 2025).

The market fixtures keep only the fields `api::Market` reads. Quotes and
curated-list answers are the full response.

These fixtures test decoding and rejection independently of live API changes.
They are not live settlement evidence.
