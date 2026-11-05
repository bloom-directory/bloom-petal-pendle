use crate::{API, Chain, Result, address, host::Host};
use petal::sdk::HttpRequest;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

const MAX_RESPONSE: usize = 512 * 1024;
/// Pendle allows 100 markets per page, but one market can take about 9 KB, so
/// pages of 40 stay well under `MAX_RESPONSE`.
pub const MARKET_PAGE: usize = 40;
pub const MAX_MARKET_PAGES: usize = 10;

fn fetch<H: Host>(host: &mut H, method: &str, url: String, body: Option<Value>) -> Result<Vec<u8>> {
    let body = body
        .map(|v| serde_json::to_vec(&v))
        .transpose()
        .map_err(|_| "encode API request")?
        .unwrap_or_default();
    let response = host.http(
        HttpRequest {
            method: method.into(),
            url,
            headers: vec![
                ("Content-Type".into(), "application/json".into()),
                ("Accept".into(), "application/json".into()),
                ("User-Agent".into(), "bloom-pendle/0.1.0".into()),
            ],
            body,
        },
        MAX_RESPONSE,
    )?;
    if response.body.len() > MAX_RESPONSE {
        return Err("Pendle response is too large".into());
    }
    if !(200..300).contains(&response.status) {
        return Err(format!(
            "Pendle returned HTTP {}; retry reads later; transaction outcomes require reconciliation",
            response.status
        ));
    }
    Ok(response.body)
}
pub fn http_json<H: Host>(
    host: &mut H,
    method: &str,
    url: String,
    body: Option<Value>,
) -> Result<Value> {
    serde_json::from_slice(&fetch(host, method, url, body)?)
        .map_err(|_| "invalid Pendle JSON response".into())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Token {
    pub address: String,
    pub symbol: String,
    pub decimals: u8,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Market {
    pub chain_id: u64,
    pub address: String,
    pub expiry: String,
    pub pt: Token,
    pub yt: Token,
    pub sy: Token,
    pub accounting_asset: Token,
    pub underlying_asset: Token,
    #[serde(default)]
    pub input_tokens: Vec<Token>,
    #[serde(default)]
    pub output_tokens: Vec<Token>,
}
pub fn market<H: Host>(host: &mut H, chain: Chain, market: &str) -> Result<Market> {
    let expected = address(market)?;
    let value = http_json(
        host,
        "GET",
        format!("{API}/v1/{}/markets/{expected:#x}", chain.id()),
        None,
    )?;
    let market: Market =
        serde_json::from_value(value).map_err(|_| "Pendle market schema changed")?;
    if market.chain_id != chain.id() || address(&market.address)? != expected {
        return Err("Pendle market identity mismatch".into());
    }
    Ok(market)
}

#[derive(Deserialize)]
struct MarketPage {
    total: usize,
    results: Vec<ListedMarket>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListedMarket {
    chain_id: u64,
    address: String,
    name: String,
    expiry: String,
    pt: String,
    yt: String,
    sy: String,
    underlying_asset: String,
    #[serde(default)]
    accounting_asset: Option<String>,
    #[serde(default)]
    input_tokens: Vec<String>,
    #[serde(default)]
    output_tokens: Vec<String>,
    #[serde(default)]
    details: ListedDetails,
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListedDetails {
    liquidity: Option<f64>,
    implied_apy: Option<f64>,
}
/// One active market, without Pendle's display and APY-breakdown fields.
/// Decimals and symbols are in `markets/<market>/info.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketSummary {
    pub address: String,
    pub name: String,
    pub expiry: String,
    pub pt: String,
    pub yt: String,
    pub sy: String,
    pub underlying_asset: String,
    pub accounting_asset: Option<String>,
    pub input_tokens: Vec<String>,
    pub output_tokens: Vec<String>,
    pub liquidity_usd: Option<f64>,
    pub implied_apy: Option<f64>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketList {
    pub chain: Chain,
    pub chain_id: u64,
    pub total: usize,
    pub count: usize,
    pub truncated: bool,
    pub results: Vec<MarketSummary>,
}
/// Pendle prefixes listed token ids with the chain id, as in `1-0xabc…`.
/// The zero address stands for the native token, which some SYs accept. It
/// is listed as Pendle reports it, although drafts refuse native inputs.
fn listed_token(chain: Chain, value: &str) -> Result<String> {
    let token = value
        .strip_prefix(&format!("{}-", chain.id()))
        .ok_or("Pendle listed a token from another chain")?;
    if token.len() != 42 || !token.starts_with("0x") {
        return Err("Pendle listed an invalid token address".into());
    }
    let token: alloy::primitives::Address = token
        .parse()
        .map_err(|_| "Pendle listed an invalid token address")?;
    Ok(format!("{token:#x}"))
}
fn summary(chain: Chain, m: ListedMarket) -> Result<MarketSummary> {
    if m.chain_id != chain.id() {
        return Err("Pendle listed a market from another chain".into());
    }
    let tokens = |list: &[String]| {
        list.iter()
            .map(|t| listed_token(chain, t))
            .collect::<Result<Vec<_>>>()
    };
    Ok(MarketSummary {
        address: format!("{:#x}", address(&m.address)?),
        name: m.name,
        expiry: m.expiry,
        pt: listed_token(chain, &m.pt)?,
        yt: listed_token(chain, &m.yt)?,
        sy: listed_token(chain, &m.sy)?,
        underlying_asset: listed_token(chain, &m.underlying_asset)?,
        accounting_asset: m
            .accounting_asset
            .as_deref()
            .map(|t| listed_token(chain, t))
            .transpose()?,
        input_tokens: tokens(&m.input_tokens)?,
        output_tokens: tokens(&m.output_tokens)?,
        liquidity_usd: m.details.liquidity,
        implied_apy: m.details.implied_apy,
    })
}
/// Every active market on `chain`, read page by page up to `MAX_MARKET_PAGES`.
pub fn active_markets<H: Host>(host: &mut H, chain: Chain) -> Result<MarketList> {
    let mut seen = BTreeSet::new();
    let mut results = Vec::new();
    let mut total = 0;
    let mut skip = 0;
    for _ in 0..MAX_MARKET_PAGES {
        let url = format!(
            "{API}/v2/markets/all?chainId={}&isActive=true&skip={skip}&limit={MARKET_PAGE}",
            chain.id()
        );
        let page: MarketPage = serde_json::from_slice(&fetch(host, "GET", url, None)?)
            .map_err(|_| "Pendle market list schema changed")?;
        total = page.total;
        let returned = page.results.len();
        let before = results.len();
        for market in page.results {
            let market = summary(chain, market)?;
            if seen.insert(market.address.clone()) {
                results.push(market);
            }
        }
        skip += returned;
        // Stop at the end, or when a page adds nothing new: an empty page, or
        // Pendle repeating rows it already sent.
        if skip >= total || results.len() == before {
            break;
        }
    }
    Ok(MarketList {
        chain,
        chain_id: chain.id(),
        total,
        count: results.len(),
        truncated: results.len() < total,
        results,
    })
}

/// Pendle's indexed positions for `owner`, keeping only `chain`.
pub fn portfolio<H: Host>(host: &mut H, chain: Chain, owner: &str) -> Result<Value> {
    let owner = address(owner)?;
    let mut value = http_json(
        host,
        "GET",
        format!("{API}/v1/dashboard/positions/database/{owner:#x}"),
        None,
    )?;
    let positions = value["positions"]
        .as_array_mut()
        .ok_or("Pendle portfolio schema changed")?;
    positions.retain(|p| p["chainId"].as_u64() == Some(chain.id()));
    Ok(value)
}
