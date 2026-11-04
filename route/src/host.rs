use crate::{Chain, Result};
use petal::sdk::{EvmTransaction, HttpRequest, HttpResponse, OutboxInspection, StagedTransaction};

pub trait Host {
    fn now_ms(&mut self) -> u64;
    fn http(&mut self, request: HttpRequest, max: usize) -> Result<HttpResponse>;
    fn vfs(&mut self, path: &str) -> Result<Vec<u8>>;
    /// A VFS file's bytes, or `None` when it does not exist. A file longer
    /// than `max` exists but comes back empty.
    fn vfs_file(&mut self, path: &str, max: usize) -> Result<Option<Vec<u8>>>;
    fn chain(&mut self, chain: Chain, method: &str, params: &str) -> Result<String>;
    fn get(&mut self, key: &str) -> Result<Option<Vec<u8>>>;
    fn put(&mut self, key: &str, value: &[u8]) -> Result<()>;
    fn put_new(&mut self, key: &str, value: &[u8]) -> Result<()>;
    fn delete_if(&mut self, key: &str, value: &[u8]) -> Result<()>;
    fn stage(&mut self, tx: &EvmTransaction) -> Result<StagedTransaction>;
    fn confirm(&mut self, wallet: &str, chain: Chain, id: &str) -> Result<StagedTransaction>;
    fn inspect(&mut self, wallet: &str, chain: Chain, id: &str) -> Result<OutboxInspection>;
}
pub struct BloomHost;
impl Host for BloomHost {
    fn now_ms(&mut self) -> u64 {
        petal::sdk::now_ms()
    }
    fn http(&mut self, req: HttpRequest, max: usize) -> Result<HttpResponse> {
        petal::sdk::http_fetch(&req, max).map_err(|_| "Pendle HTTP transport failed".into())
    }
    fn vfs(&mut self, path: &str) -> Result<Vec<u8>> {
        petal::sdk::vfs_read(path, 128)
            .map_err(|_| "selected account address is unavailable".into())
    }
    fn vfs_file(&mut self, path: &str, max: usize) -> Result<Option<Vec<u8>>> {
        match petal::sdk::vfs_read(path, max) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(petal::SdkError::Host(petal::HostStatus::NotFound)) => Ok(None),
            Err(petal::SdkError::Host(petal::HostStatus::BufferTooSmall { .. })) => {
                Ok(Some(Vec::new()))
            }
            Err(_) => Err("outbox entry read failed".into()),
        }
    }
    fn chain(&mut self, chain: Chain, method: &str, params: &str) -> Result<String> {
        petal::sdk::chain_read(chain.name(), method, params)
            .map_err(|_| format!("{} chain read failed", chain.name()))
    }
    fn get(&mut self, key: &str) -> Result<Option<Vec<u8>>> {
        match petal::sdk::store_get(key, 128 * 1024) {
            Ok(v) => Ok(Some(v)),
            Err(petal::SdkError::Host(petal::HostStatus::NotFound)) => Ok(None),
            Err(_) => Err("operation store read failed".into()),
        }
    }
    fn put(&mut self, key: &str, value: &[u8]) -> Result<()> {
        petal::sdk::store_put(key, value, false).map_err(|_| "operation store write failed".into())
    }
    fn put_new(&mut self, key: &str, value: &[u8]) -> Result<()> {
        petal::sdk::store_put_new(key, value, false).map_err(|_| {
            "operation already exists or is locked; inspect status before retrying".into()
        })
    }
    fn delete_if(&mut self, key: &str, value: &[u8]) -> Result<()> {
        petal::sdk::store_del_if_value(key, value)
            .map_err(|_| "operation lock could not be released".into())
    }
    fn stage(&mut self, tx: &EvmTransaction) -> Result<StagedTransaction> {
        petal::sdk::tx_stage(tx).map_err(|_| {
            "outbox stage did not return an id; inspect the account outbox before recovery".into()
        })
    }
    fn confirm(&mut self, wallet: &str, chain: Chain, id: &str) -> Result<StagedTransaction> {
        petal::sdk::tx_confirm(wallet, chain.name(), id, false)
            .map_err(|error| confirm_refusal(&error))
    }
    fn inspect(&mut self, wallet: &str, chain: Chain, id: &str) -> Result<OutboxInspection> {
        petal::sdk::tx_inspect(wallet, chain.name(), id)
            .map_err(|_| "outbox inspection failed".into())
    }
}

/// The error the Petal shows for a confirm Bloom refused. The SDK passes
/// Bloom's own text unless it says "not found", "denied", "permission" or
/// "invalid", which it reduces to a bare status.
pub fn confirm_refusal(error: &petal::SdkError) -> String {
    match error {
        petal::SdkError::Message(reason) => format!(
            "Bloom has not confirmed the transaction: {}",
            bloom_reason(reason)
        ),
        petal::SdkError::Host(petal::HostStatus::Denied) => "Bloom denied the transaction".into(),
        _ => "Bloom has not confirmed the transaction".into(),
    }
}

/// Bloom's reason, fit to show from this route: no URLs, which may carry a
/// provider key; no control, format or separator characters, which a revert
/// string may use to hide or fake text; and none of Bloom's advice the Petal
/// cannot follow, which is written for Bloom's own outbox.
pub fn bloom_reason(raw: &str) -> String {
    let mut reason: String = raw
        .strip_prefix("backend: ")
        .unwrap_or(raw)
        .chars()
        .map(|c| {
            if c.is_control() || invisible(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    // Writing `override` would broadcast a transaction that reverts.
    if let Some(cut) = reason.find(" — write 'override'") {
        reason.truncate(cut);
    }
    // The Petal cannot restage with an explicit nonce. Discarding the entry
    // that holds the missing nonce leaves this one unsendable, and when none
    // holds it, this one can never be sent.
    if let Some(cut) = reason.find(" Broadcast the earlier transaction") {
        reason.truncate(cut);
        match unheld_nonce(&reason) {
            Some(nonce) => reason.push_str(&format!(
                " No pending entry holds nonce {nonce}, so this entry can never be sent; create a new reviewed intent"
            )),
            None => reason.push_str(" Finish the operation that staged it first; if it is discarded instead, this entry can never be sent and needs a new intent."),
        }
    }
    // A signature may exist, so the entry must not be discarded, and Bloom
    // meets the same conflict on every confirm.
    if let Some(cut) =
        reason.find(" — reconcile the recorded signing operation or discard the entry")
    {
        reason.truncate(cut);
        reason.push_str(": do not discard it. Confirming again repeats this refusal, so inspect the outbox entry with the wallet's owner or operator.");
    }
    hide_urls(&reason).chars().take(600).collect()
}

/// For Bloom's nonce-gap refusal, the account's next nonce when no pending
/// entry holds it: then nothing can fill the gap. Bloom names the entries
/// holding the missing nonces, lowest first, and none when there are none.
pub fn unheld_nonce(reason: &str) -> Option<u64> {
    let gap = &reason[reason.find("nonce gap: ")?..];
    let next = &gap[gap.find("next on-chain nonce is ")? + "next on-chain nonce is ".len()..];
    let next: u64 = next[..next.find(|c: char| !c.is_ascii_digit())?]
        .parse()
        .ok()?;
    let lowest = [" Nonce ", " Nonces "]
        .iter()
        .filter_map(|lead| {
            let held = &gap[gap.find(lead)? + lead.len()..];
            held[..held.find(|c: char| !c.is_ascii_digit())?]
                .parse::<u64>()
                .ok()
        })
        .next();
    (lowest != Some(next)).then_some(next)
}

/// Unicode format, line separator and paragraph separator characters
/// (categories Cf, Zl and Zp): invisible, or able to reorder or break text.
fn invisible(c: char) -> bool {
    matches!(
        c,
        '\u{AD}'
            | '\u{600}'..='\u{605}'
            | '\u{61C}'
            | '\u{6DD}'
            | '\u{70F}'
            | '\u{890}'..='\u{891}'
            | '\u{8E2}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{2028}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{110BD}'
            | '\u{110CD}'
            | '\u{13430}'..='\u{1343F}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0001}'
            | '\u{E0020}'..='\u{E007F}'
    )
}

/// Every URL becomes `<rpc url>`, whatever its scheme or case, including a
/// JSON-escaped one. A URL runs from its scheme to whitespace or a closing
/// parenthesis or quote.
fn hide_urls(text: &str) -> String {
    let mut shown = String::new();
    let mut kept = 0;
    let mut from = 0;
    while let Some(at) = ["://", r":\/\/"]
        .iter()
        .filter_map(|separator| text[from..].find(separator).map(|at| from + at))
        .min()
    {
        let scheme = text[kept..at]
            .char_indices()
            .rev()
            .take_while(|(_, c)| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
            .last()
            .map_or(at, |(start, _)| kept + start);
        let end = text[at..]
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | '"' | '\''))
            .map_or(text.len(), |end| at + end);
        if scheme < at {
            shown.push_str(&text[kept..scheme]);
            shown.push_str("<rpc url>");
            kept = end;
            from = end;
        } else {
            from = at + 1;
        }
    }
    shown.push_str(&text[kept..]);
    shown
}

pub fn account_address<H: Host>(host: &mut H, account: &petal::WalletAccount) -> Result<String> {
    petal::parse_evm_address(&host.vfs(&account.address_evm_path())?)
}
