use crate::{API, CHAIN_ID, Result, address, host::Host};
use petal::sdk::HttpRequest;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const MAX_RESPONSE: usize = 512 * 1024;
pub fn http_json<H: Host>(
    host: &mut H,
    method: &str,
    url: String,
    body: Option<Value>,
) -> Result<Value> {
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
    serde_json::from_slice(&response.body).map_err(|_| "invalid Pendle JSON response".into())
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
pub fn market<H: Host>(host: &mut H, market: &str) -> Result<Market> {
    let expected = address(market)?;
    let value = http_json(
        host,
        "GET",
        format!("{API}/v1/{CHAIN_ID}/markets/{expected:#x}"),
        None,
    )?;
    let market: Market =
        serde_json::from_value(value).map_err(|_| "Pendle market schema changed")?;
    if market.chain_id != CHAIN_ID || address(&market.address)? != expected {
        return Err("Pendle market identity mismatch".into());
    }
    Ok(market)
}
