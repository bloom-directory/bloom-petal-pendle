//! Account-bound Pendle V2 operations. All signing remains in Bloom's triad.
pub mod api;
pub mod evm;
pub mod host;
pub mod workflow;

use alloy::primitives::{Address, U256};
use petal::{DispatchResponse, WalletAccount};
use serde::{Deserialize, Serialize};

pub type Result<T> = std::result::Result<T, String>;
/// Pendle's router has the same address on every supported chain.
pub const ROUTER: &str = "0x888888888889758f76e7103c6cbf23abbf58f946";
pub const API: &str = "https://api-v2.pendle.finance/core";
pub const SDK_REV: &str = "7dd2722db5de305dd63ca4ed03591f3ab82fa40b";

/// A supported chain. The name is Bloom's chain name. The id is pinned here;
/// live `eth_chainId` reads only confirm it.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Chain {
    Ethereum,
    Arbitrum,
}
impl Chain {
    pub const ALL: [Self; 2] = [Self::Ethereum, Self::Arbitrum];
    pub fn parse(value: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|chain| chain.name() == value)
            .ok_or_else(|| "supported chains are ethereum and arbitrum".into())
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Ethereum => "ethereum",
            Self::Arbitrum => "arbitrum",
        }
    }
    pub fn id(self) -> u64 {
        match self {
            Self::Ethereum => 1,
            Self::Arbitrum => 42161,
        }
    }
}
pub fn address(value: &str) -> Result<Address> {
    if value.len() != 42 || !value.starts_with("0x") {
        return Err("expected a 20-byte 0x address".into());
    }
    let addr: Address = value.parse().map_err(|_| "invalid address")?;
    if addr.is_zero() {
        return Err("native-token and zero addresses are not supported".into());
    }
    Ok(addr)
}
pub fn amount(value: &str) -> Result<U256> {
    if value.is_empty()
        || value.len() > 78
        || value.starts_with('0')
        || !value.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(
            "amount must be a positive canonical uint256 decimal string in base units".into(),
        );
    }
    U256::from_str_radix(value, 10).map_err(|_| "amount exceeds uint256".into())
}
pub fn operation_id(value: &str) -> Result<&str> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("id must be 1-64 ASCII letters, digits, hyphens or underscores".into());
    }
    Ok(value)
}
pub fn route_chain(ctx: &petal::Ctx) -> std::result::Result<Chain, DispatchResponse> {
    Chain::parse(petal::param(ctx, "chain")?).map_err(petal::route_invalid)
}
pub fn selected(ctx: &petal::Ctx) -> std::result::Result<(Chain, WalletAccount), DispatchResponse> {
    Ok((route_chain(ctx)?, petal::wallet_account(ctx)?))
}
pub fn response<T: Serialize>(result: Result<T>) -> DispatchResponse {
    match result {
        Ok(v) => petal::read_json_value(&v),
        Err(e) => petal::error(-4, e),
    }
}
pub fn written(result: Result<()>) -> DispatchResponse {
    match result {
        Ok(()) => DispatchResponse::Write,
        Err(e) => petal::error(-4, e),
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    BuyPt,
    SellPt,
    RedeemPt,
}
impl Operation {
    pub fn name(self) -> &'static str {
        match self {
            Self::BuyPt => "buy-pt",
            Self::SellPt => "sell-pt",
            Self::RedeemPt => "redeem-pt",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DraftInput {
    pub market: String,
    /// Input asset for buy; output asset for sell/redeem. SY is supported too.
    pub token: String,
    pub amount: String,
    pub min_output: String,
    pub slippage_bps: u16,
    pub max_price_impact_bps: u16,
}
impl DraftInput {
    pub fn parse(body: &[u8]) -> Result<Self> {
        if body.len() > 4096 {
            return Err("draft body exceeds 4096 bytes".into());
        }
        let mut value: Self =
            serde_json::from_slice(body).map_err(|_| "invalid draft JSON or unknown field")?;
        value.market = format!("{:#x}", address(&value.market)?);
        value.token = format!("{:#x}", address(&value.token)?);
        amount(&value.amount)?;
        amount(&value.min_output)?;
        if !(1..=100).contains(&value.slippage_bps) || value.max_price_impact_bps > 300 {
            return Err("slippage_bps must be 1-100 and max_price_impact_bps must be 0-300".into());
        }
        Ok(value)
    }
}
