//! Decode actual ABI bytes, never the API's descriptive contractParamInfo.
use crate::{
    Chain, DraftInput, Operation, ROUTER, Result, address, amount, api::Market, host::Host,
};
use alloy::{
    primitives::{Address, Bytes, U256},
    sol,
    sol_types::SolCall,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

sol! {
    struct SwapData { uint8 swapType; address extRouter; bytes extCalldata; bool needScale; }
    struct TokenInput { address tokenIn; uint256 netTokenIn; address tokenMintSy; address pendleSwap; SwapData swapData; }
    struct TokenOutput { address tokenOut; uint256 minTokenOut; address tokenRedeemSy; address pendleSwap; SwapData swapData; }
    struct ApproxParams { uint256 guessMin; uint256 guessMax; uint256 guessOffchain; uint256 maxIteration; uint256 eps; }
    struct Order { uint256 salt; uint256 expiry; uint256 nonce; uint8 orderType; address token; address YT; address maker; address receiver; uint256 makingAmount; uint256 lnImpliedRate; uint256 failSafeRate; bytes permit; }
    struct FillOrderParams { Order order; bytes signature; uint256 makingAmount; }
    struct LimitOrderData { address limitRouter; uint256 epsSkipMarket; FillOrderParams[] normalFills; FillOrderParams[] flashFills; bytes optData; }
    function swapExactTokenForPt(address receiver, address market, uint256 minPtOut, ApproxParams guessPtOut, TokenInput input, LimitOrderData limit) external payable;
    function swapExactSyForPt(address receiver, address market, uint256 exactSyIn, uint256 minPtOut, ApproxParams guessPtOut, LimitOrderData limit) external;
    function swapExactPtForToken(address receiver, address market, uint256 exactPtIn, TokenOutput output, LimitOrderData limit) external;
    function swapExactPtForSy(address receiver, address market, uint256 exactPtIn, uint256 minSyOut, LimitOrderData limit) external;
    function redeemPyToToken(address receiver, address YT, uint256 netPyIn, TokenOutput output) external;
    function redeemPyToSy(address receiver, address YT, uint256 netPyIn, uint256 minSyOut) external;
    function readTokens() external view returns (address sy, address pt, address yt);
    function expiry() external view returns (uint256 value);
    function isValidMarket(address market) external view returns (bool valid);
    function getTokensIn() external view returns (address[] tokens);
    function getTokensOut() external view returns (address[] tokens);
    function allowance(address owner, address spender) external view returns (uint256 value);
    function balanceOf(address owner) external view returns (uint256 value);
    function decimals() external view returns (uint8 value);
    function approve(address spender, uint256 amount) external returns (bool);
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    pub to: String,
    pub data: String,
    #[serde(default = "zero")]
    pub value: String,
    pub from: String,
}
fn zero() -> String {
    "0".into()
}
fn rpc_string(result: &str) -> Result<String> {
    let value: serde_json::Value =
        serde_json::from_str(result).unwrap_or_else(|_| result.to_owned().into());
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "invalid chain read response".into())
}
pub fn call<H: Host, C: SolCall>(
    host: &mut H,
    chain: Chain,
    to: Address,
    call: C,
) -> Result<C::Return> {
    let params = json!([{"to":format!("{to:#x}"),"data":format!("0x{}",hex::encode(call.abi_encode()))}, "latest"]);
    let result = rpc_string(&host.chain(chain, "eth_call", &params.to_string())?)?;
    let bytes = hex::decode(result.strip_prefix("0x").ok_or("invalid eth_call hex")?)
        .map_err(|_| "invalid eth_call hex")?;
    C::abi_decode_returns(&bytes).map_err(|_| "contract returned invalid ABI bytes".into())
}
/// The chain id reported by the RPC that Bloom has configured for `chain`.
pub fn chain_id<H: Host>(host: &mut H, chain: Chain) -> Result<u64> {
    let result = rpc_string(&host.chain(chain, "eth_chainId", "[]")?)?;
    u64::from_str_radix(
        result.strip_prefix("0x").ok_or("invalid eth_chainId hex")?,
        16,
    )
    .map_err(|_| "invalid eth_chainId hex".into())
}
/// Pendle's market factories (`marketFactory` and `marketFactoryV3` to `V6`),
/// pinned from `deployments/<chain id>-core.json` in pendle-core-v2-public.
pub fn factories(chain: Chain) -> [&'static str; 5] {
    match chain {
        Chain::Ethereum => [
            "0x27b1dacd74688af24a64bd3c9c1b143118740784",
            "0x1a6fcc85557bc4fb7b534ed835a03ef056552d52",
            "0x3d75bd20c983edb5fd218a1b7e0024f1056c7a2f",
            "0x6fcf753f2c67b83f7b09746bbc4fa0047b35d050",
            "0x6d247b1c044fa1e22e6b04fa9f71baf99eb29a9f",
        ],
        Chain::Arbitrum => [
            "0xf5a7de2d276dbda3eef1b62a9e718eff4d29ddc8",
            "0x2fcb47b58350cd377f94d3821e7373df60bd9ced",
            "0xd9f5e9589016da862d2abce980a5a5b99a94f3e8",
            "0xd29e76c6f15ada0150d10a1d3f45accd2098283b",
            "0x49f2f7002669e0e4425fa0203975625ab4af3143",
        ],
    }
}

pub fn verify_market<H: Host>(
    host: &mut H,
    chain: Chain,
    market: &Market,
    operation: Operation,
    token: Address,
) -> Result<bool> {
    if market.chain_id != chain.id() {
        return Err(format!(
            "market belongs to chain {}, not {}",
            market.chain_id,
            chain.name()
        ));
    }
    // A Bloom chain pointed at the wrong network fails here, before any
    // factory lookup could misreport it as an unauthenticated market.
    if chain_id(host, chain)? != chain.id() {
        return Err(format!(
            "Bloom's {} RPC does not report chain id {}",
            chain.name(),
            chain.id()
        ));
    }
    let target = address(&market.address)?;
    let mut valid = false;
    for factory in factories(chain) {
        if call(
            host,
            chain,
            address(factory)?,
            isValidMarketCall { market: target },
        )
        .is_ok_and(|valid| valid)
        {
            valid = true;
            break;
        }
    }
    if !valid {
        return Err("market is not authenticated by a supported Pendle factory".into());
    }
    let tokens = call(host, chain, target, readTokensCall {})?;
    if tokens.sy != address(&market.sy.address)?
        || tokens.pt != address(&market.pt.address)?
        || tokens.yt != address(&market.yt.address)?
    {
        return Err("API token identities disagree with the on-chain market".into());
    }
    let expiry = call(host, chain, tokens.pt, expiryCall {})?;
    let expired = expiry <= U256::from(host.now_ms() / 1000);
    match operation {
        Operation::BuyPt | Operation::SellPt if expired => {
            return Err("market has matured; use redeem-pt".into());
        }
        Operation::RedeemPt if !expired => {
            return Err("redeem-pt requires a matured market; use sell-pt before maturity".into());
        }
        _ => {}
    }
    if token != tokens.sy {
        let supported = if operation == Operation::BuyPt {
            call(host, chain, tokens.sy, getTokensInCall {})?
        } else {
            call(host, chain, tokens.sy, getTokensOutCall {})?
        };
        if !supported.contains(&token) {
            return Err("asset is not natively supported by this market's SY; aggregator routes are disabled".into());
        }
    }
    Ok(expired)
}

fn empty_limit(limit: &LimitOrderData) -> Result<()> {
    if !limit.limitRouter.is_zero()
        || !limit.epsSkipMarket.is_zero()
        || !limit.normalFills.is_empty()
        || !limit.flashFills.is_empty()
        || !limit.optData.is_empty()
    {
        return Err("embedded limit-order fills are not supported".into());
    }
    Ok(())
}
fn no_swap(swap: &SwapData, pendle_swap: Address) -> Result<()> {
    if swap.swapType != 0
        || !swap.extRouter.is_zero()
        || !swap.extCalldata.is_empty()
        || swap.needScale
        || !pendle_swap.is_zero()
    {
        return Err("external swap routing is not supported".into());
    }
    Ok(())
}
fn decode<C: SolCall>(bytes: &[u8]) -> Result<C> {
    let value = C::abi_decode(bytes).map_err(|_| "invalid Pendle calldata")?;
    if value.abi_encode() != bytes {
        return Err("noncanonical or trailing Pendle calldata".into());
    }
    Ok(value)
}

/// Returns the minimum output enforced in the actual transaction bytes.
pub fn validate_transaction(
    tx: &Transaction,
    owner: &str,
    op: Operation,
    req: &DraftInput,
    market: &Market,
) -> Result<U256> {
    if address(&tx.to)? != address(ROUTER)?
        || address(&tx.from)? != address(owner)?
        || tx.value != "0"
    {
        return Err(
            "quote target, sender or native value does not match the bounded intent".into(),
        );
    }
    let bytes = hex::decode(
        tx.data
            .strip_prefix("0x")
            .ok_or("calldata needs 0x prefix")?,
    )
    .map_err(|_| "invalid calldata hex")?;
    if bytes.len() > 32 * 1024 {
        return Err("calldata exceeds 32 KiB".into());
    }
    let owner = address(owner)?;
    let target = address(&req.market)?;
    let token = address(&req.token)?;
    let exact = amount(&req.amount)?;
    let (receiver, bound_market, spent, minimum) = match op {
        Operation::BuyPt if bytes.starts_with(&swapExactSyForPtCall::SELECTOR) => {
            let c = decode::<swapExactSyForPtCall>(&bytes)?;
            empty_limit(&c.limit)?;
            if token != address(&market.sy.address)? {
                return Err("SY input mismatch".into());
            }
            (c.receiver, c.market, c.exactSyIn, c.minPtOut)
        }
        Operation::BuyPt => {
            let c = decode::<swapExactTokenForPtCall>(&bytes)?;
            empty_limit(&c.limit)?;
            no_swap(&c.input.swapData, c.input.pendleSwap)?;
            if c.input.tokenIn != token || c.input.tokenMintSy != token {
                return Err("input token mismatch".into());
            }
            (c.receiver, c.market, c.input.netTokenIn, c.minPtOut)
        }
        Operation::SellPt if bytes.starts_with(&swapExactPtForSyCall::SELECTOR) => {
            let c = decode::<swapExactPtForSyCall>(&bytes)?;
            empty_limit(&c.limit)?;
            if token != address(&market.sy.address)? {
                return Err("SY output mismatch".into());
            }
            (c.receiver, c.market, c.exactPtIn, c.minSyOut)
        }
        Operation::SellPt => {
            let c = decode::<swapExactPtForTokenCall>(&bytes)?;
            empty_limit(&c.limit)?;
            no_swap(&c.output.swapData, c.output.pendleSwap)?;
            if c.output.tokenOut != token || c.output.tokenRedeemSy != token {
                return Err("output token mismatch".into());
            }
            (c.receiver, c.market, c.exactPtIn, c.output.minTokenOut)
        }
        Operation::RedeemPt if bytes.starts_with(&redeemPyToSyCall::SELECTOR) => {
            let c = decode::<redeemPyToSyCall>(&bytes)?;
            if c.YT != address(&market.yt.address)? || token != address(&market.sy.address)? {
                return Err("redemption token mismatch".into());
            }
            (c.receiver, target, c.netPyIn, c.minSyOut)
        }
        Operation::RedeemPt => {
            let c = decode::<redeemPyToTokenCall>(&bytes)?;
            no_swap(&c.output.swapData, c.output.pendleSwap)?;
            if c.YT != address(&market.yt.address)?
                || c.output.tokenOut != token
                || c.output.tokenRedeemSy != token
            {
                return Err("redemption token mismatch".into());
            }
            (c.receiver, target, c.netPyIn, c.output.minTokenOut)
        }
    };
    if receiver != owner
        || bound_market != target
        || spent != exact
        || minimum < amount(&req.min_output)?
    {
        return Err(
            "calldata receiver, market, input amount or minimum output violates the intent".into(),
        );
    }
    Ok(minimum)
}

pub fn approval(token: &str, owner: &str, value: U256) -> Transaction {
    Transaction {
        to: token.into(),
        from: owner.into(),
        value: "0".into(),
        data: format!(
            "0x{}",
            hex::encode(
                approveCall {
                    spender: ROUTER.parse().expect("constant router"),
                    amount: value
                }
                .abi_encode()
            )
        ),
    }
}

pub fn raw_bytes(value: &str) -> Result<Bytes> {
    hex::decode(value.strip_prefix("0x").ok_or("expected 0x bytes")?)
        .map(Bytes::from)
        .map_err(|_| "invalid bytes".into())
}
