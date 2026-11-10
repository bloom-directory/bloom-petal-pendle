use crate::{
    Chain, DraftInput, Operation, ROUTER, Result, address, amount, api, evm,
    host::{Host, account_address},
    operation_id,
};
use alloy::primitives::U256;
use petal::{WalletAccount, sdk::EvmTransaction};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Step {
    pub kind: String,
    pub transaction: evm::Transaction,
    pub state: String,
    pub outbox_id: Option<String>,
    pub tx_hash: Option<String>,
    /// The same-nonce cancel Bloom sent for this step, when it sent one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel_tx_hash: Option<String>,
    /// A same-nonce replacement Bloom sent for this step, when it sent one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement_tx_hash: Option<String>,
    /// When Bloom created this step's outbox entry. Outbox ids repeat after
    /// Bloom restarts, so this tells the entry apart from an older one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outbox_created_ms: Option<u64>,
    /// The owner approval Bloom asked for when this step was last confirmed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<Approval>,
}
/// Bloom's pending owner approval: its action id, when it expires, and what
/// it approves. `transaction` is this step's outbox entry; `wallet_policy` is
/// the wallet's pending policy update; `unknown` could not be told apart.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    pub action_id: String,
    pub expires_ms: u64,
    #[serde(default)]
    pub kind: String,
    /// For a `wallet_policy` approval, this package's hash. Bloom hands back
    /// any pending update, and a Petal can read neither the update nor the
    /// wallet policy, so the owner checks on the ceremony page what it changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
}
pub const SCHEMA: &str = "bloom.pendle.operation.v2";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Draft {
    pub schema: String,
    pub chain: Chain,
    pub id: String,
    pub wallet: String,
    pub account: u32,
    pub owner: String,
    pub operation: Operation,
    pub request: DraftInput,
    pub market: api::Market,
    pub input_token: String,
    pub output_token: String,
    /// Read from the tokens onchain when the draft is made.
    pub input_decimals: u8,
    pub output_decimals: u8,
    pub expected_output: String,
    pub encoded_minimum: String,
    pub price_impact: f64,
    pub created_ms: u64,
    pub expires_ms: u64,
    pub state: String,
    pub steps: Vec<Step>,
}
/// Store prefix of one chain's operations of one kind, in the account store.
pub fn prefix(chain: Chain, op: Operation) -> String {
    format!("operations/{}/{}/", chain.name(), op.name())
}
fn key(chain: Chain, op: Operation, id: &str) -> Result<String> {
    Ok(format!("{}{}.json", prefix(chain, op), operation_id(id)?))
}
pub fn lock_key(chain: Chain, op: Operation, id: &str) -> Result<String> {
    Ok(format!(
        "locks/{}/{}/{}",
        chain.name(),
        op.name(),
        operation_id(id)?
    ))
}
/// Builds before Ethereum support stored Arbitrum operations without a chain
/// segment. Such an operation may already have a Bloom outbox entry this build
/// cannot see, so a second draft under the same id could trade twice.
fn refuse_legacy<H: Host>(host: &mut H, chain: Chain, op: Operation, id: &str) -> Result<()> {
    let legacy = format!("operations/{}/{}.json", op.name(), operation_id(id)?);
    if chain == Chain::Arbitrum && host.get(&legacy)?.is_some() {
        return Err("an older Pendle build staged this id; finish it with that build".into());
    }
    Ok(())
}
fn save<H: Host>(host: &mut H, draft: &Draft) -> Result<()> {
    host.put(
        &key(draft.chain, draft.operation, &draft.id)?,
        &serde_json::to_vec(draft).map_err(|_| "encode operation")?,
    )
}
pub fn load<H: Host>(
    host: &mut H,
    chain: Chain,
    account: &WalletAccount,
    op: Operation,
    id: &str,
) -> Result<Draft> {
    let Some(data) = host.get(&key(chain, op, id)?)? else {
        refuse_legacy(host, chain, op, id)?;
        return Err("operation not found".into());
    };
    let draft: Draft = serde_json::from_slice(&data).map_err(|_| "invalid persisted operation")?;
    if draft.schema != SCHEMA || draft.chain != chain || draft.market.chain_id != chain.id() {
        return Err("persisted operation does not belong to the selected chain".into());
    }
    if draft.wallet != account.wallet()
        || draft.account != account.number()
        || draft.operation != op
        || draft.id != id
        || address(&draft.owner)? != address(&account_address(host, account)?)?
    {
        return Err("persisted operation does not belong to the selected account".into());
    }
    Ok(draft)
}

pub fn create<H: Host>(
    host: &mut H,
    chain: Chain,
    account: &WalletAccount,
    op: Operation,
    id: &str,
    body: &[u8],
) -> Result<()> {
    let key = key(chain, op, id)?;
    let request = DraftInput::parse(body)?;
    if host.get(&key)?.is_some() {
        let old = load(host, chain, account, op, id)?;
        return if old.request == request {
            Ok(())
        } else {
            Err("id already binds a different intent; choose a new id".into())
        };
    }
    refuse_legacy(host, chain, op, id)?;
    // Refused before anything else is read, so the refusal names the reason.
    api::curated(host, chain, &request.market)?;
    let owner = account_address(host, account)?;
    let market = api::market(host, chain, &request.market)?;
    evm::verify_market(host, chain, &market, op, address(&request.token)?)?;
    let (input_token, output_token) = if op == Operation::BuyPt {
        (request.token.clone(), market.pt.address.clone())
    } else {
        (market.pt.address.clone(), request.token.clone())
    };
    // Pendle fills limit orders inside swaps unless told not to, and the
    // calldata check refuses any embedded fill.
    let quote = api::http_json(
        host,
        "POST",
        format!("{}/v3/sdk/{}/convert", crate::API, chain.id()),
        Some(json!({
            "inputs":[{"token":input_token,"amount":request.amount}], "outputs":[output_token],
            "receiver":owner,"slippage":f64::from(request.slippage_bps)/10000.0,
            "enableAggregator":false,"useLimitOrder":false
        })),
    )?;
    let inputs = quote["inputs"].as_array().ok_or("quote omitted inputs")?;
    if inputs.len() != 1
        || inputs[0]["amount"] != request.amount
        || address(
            inputs[0]["token"]
                .as_str()
                .ok_or("quote input token missing")?,
        )? != address(&input_token)?
    {
        return Err("quote input differs from intent".into());
    }
    let route = quote["routes"]
        .as_array()
        .and_then(|routes| routes.first())
        .ok_or("Pendle returned no route")?;
    if route["data"]["aggregatorType"] != "VOID" {
        return Err("quote uses an external aggregator".into());
    }
    let impact = route["data"]["priceImpact"]
        .as_f64()
        .ok_or("quote omitted price impact")?;
    // Pendle reports an adverse impact as a negative fraction, so the ceiling
    // bounds its size, whichever its sign.
    if !impact.is_finite() || impact.abs() > f64::from(request.max_price_impact_bps) / 10000.0 {
        return Err("price impact exceeds the requested ceiling".into());
    }
    let outputs = route["outputs"].as_array().ok_or("quote omitted outputs")?;
    if outputs.len() != 1
        || address(
            outputs[0]["token"]
                .as_str()
                .ok_or("quote output token missing")?,
        )? != address(&output_token)?
    {
        return Err("quote output differs from intent".into());
    }
    let expected_output = outputs[0]["amount"]
        .as_str()
        .ok_or("quote output amount missing")?
        .to_owned();
    amount(&expected_output)?;
    let transaction: evm::Transaction = serde_json::from_value(route["tx"].clone())
        .map_err(|_| "quote transaction schema changed")?;
    let minimum = evm::validate_transaction(&transaction, &owner, op, &request, &market)?;
    let expected = amount(&expected_output)?;
    if minimum > expected {
        return Err("quote minimum exceeds expected output".into());
    }
    // Pendle encodes floor(expected × (10000 − slippage_bps) / 10000). A lower
    // minimum allows more slippage than the intent asked for.
    let floor = expected
        .checked_mul(U256::from(10_000 - request.slippage_bps))
        .ok_or("expected output is too large")?
        / U256::from(10_000);
    if minimum < floor {
        return Err("quote minimum allows more slippage than slippage_bps".into());
    }
    // plan.md shows decimals, so they come from the tokens, not the API.
    let input_decimals = evm::call(host, chain, address(&input_token)?, evm::decimalsCall {})?;
    let output_decimals = evm::call(host, chain, address(&output_token)?, evm::decimalsCall {})?;
    let current_allowance = evm::call(
        host,
        chain,
        address(&input_token)?,
        evm::allowanceCall {
            owner: address(&owner)?,
            spender: address(ROUTER)?,
        },
    )?;
    let mut steps = Vec::new();
    let mut add = |kind: &str, transaction: evm::Transaction| {
        steps.push(Step {
            kind: kind.into(),
            transaction,
            state: "prepared".into(),
            outbox_id: None,
            tx_hash: None,
            cancel_tx_hash: None,
            replacement_tx_hash: None,
            outbox_created_ms: None,
            approval: None,
        })
    };
    if current_allowance < amount(&request.amount)? {
        if !current_allowance.is_zero() {
            add(
                "reset_allowance",
                evm::approval(&input_token, &owner, U256::ZERO),
            );
        }
        add(
            "approve_input",
            evm::approval(&input_token, &owner, amount(&request.amount)?),
        );
    }
    add("pendle_execution", transaction);
    let created_ms = host.now_ms();
    let draft = Draft {
        schema: SCHEMA.into(),
        chain,
        id: id.into(),
        wallet: account.wallet().into(),
        account: account.number(),
        owner,
        operation: op,
        request,
        market,
        input_token,
        output_token,
        input_decimals,
        output_decimals,
        expected_output,
        encoded_minimum: minimum.to_string(),
        price_impact: impact,
        created_ms,
        expires_ms: created_ms.saturating_add(300_000),
        state: "prepared".into(),
        steps,
    };
    host.put_new(
        &key,
        &serde_json::to_vec(&draft).map_err(|_| "encode draft")?,
    )
}

/// An outbox id Bloom returned, checked before it goes into a path or key.
fn outbox_id(id: &str) -> Result<&str> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err("Bloom returned an unexpected outbox id".into());
    }
    Ok(id)
}
/// `wallets/<wallet>/<index>/chains/<chain>/outbox/<dir>/<id>`.
fn outbox_entry(account: &WalletAccount, chain: Chain, dir: &str, id: &str) -> Result<String> {
    Ok(format!(
        "{}/chains/{}/outbox/{dir}/{}",
        account.dir(),
        chain.name(),
        outbox_id(id)?
    ))
}

/// The transaction hash in a Bloom intent or broadcast marker.
fn recorded_hash(bytes: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(bytes)
        .ok()
        .and_then(|value| value["tx_hash"].as_str().map(String::from))
}

/// A hash Bloom writes as a bare text file, such as `cancel_tx_hash`.
fn text_hash(bytes: Option<&[u8]>) -> Option<String> {
    bytes
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .map(|hash| hash.trim().to_owned())
        .filter(|hash| !hash.is_empty())
}

/// An entry Bloom moved to `outbox/failed/` and no longer sends.
struct Discarded {
    /// Bloom recorded a hash, a broadcast marker or a cancel for it.
    maybe_sent: bool,
    tx_hash: Option<String>,
    cancel_tx_hash: Option<String>,
    replacement_tx_hash: Option<String>,
}
/// Bloom 0.4.1 moves an entry to `failed/` when it expires, when policy
/// refuses it and when a send of it is cancelled, and leaves the status in
/// its intent as it was. So the directory decides, not the status Bloom
/// reports. Bloom
/// writes a broadcast marker before each submit and never removes it, and a
/// lost submit reply leaves the hash only there, so an entry was never sent
/// only when it has no hash, no marker and no cancel.
fn discarded<H: Host>(
    host: &mut H,
    account: &WalletAccount,
    chain: Chain,
    id: &str,
    created_ms: Option<u64>,
) -> Result<Option<Discarded>> {
    let dir = outbox_entry(account, chain, "failed", id)?;
    let mut read = |file: &str| host.vfs_file(&format!("{dir}/{file}"), 256 * 1024);
    let Some(intent) = read("intent.json")? else {
        return Ok(None);
    };
    let parsed = serde_json::from_slice::<Value>(&intent).ok();
    // An older entry under a reused id is not this step's.
    if let (Some(intent), Some(created_ms)) = (&parsed, created_ms)
        && intent["created_ms"].as_u64() != Some(created_ms)
    {
        return Ok(None);
    }
    let attempt = read("broadcast_attempted.json")?;
    let replacement = read("replacement_broadcast_attempted.json")?;
    let replacement_hash = read("replacement_tx_hash")?;
    let cancel = read("cancel_tx_hash")?;
    let cancel_attempt = read("cancel_broadcast_attempted.json")?;
    // Bloom after 0.4.1 marks an entry an older Bloom released although it
    // may have been signed.
    let signing_unknown = read("signing_unknown.json")?;
    // An intent the Petal cannot read may record a hash.
    let readable = parsed.is_some();
    let tx_hash = recorded_hash(&intent).or_else(|| attempt.as_deref().and_then(recorded_hash));
    let cancel_tx_hash =
        text_hash(cancel.as_deref()).or_else(|| cancel_attempt.as_deref().and_then(recorded_hash));
    let replacement_tx_hash = text_hash(replacement_hash.as_deref())
        .or_else(|| replacement.as_deref().and_then(recorded_hash));
    Ok(Some(Discarded {
        maybe_sent: !readable
            || tx_hash.is_some()
            || attempt.is_some()
            || replacement.is_some()
            || replacement_hash.is_some()
            || cancel.is_some()
            || cancel_attempt.is_some()
            || signing_unknown.is_some(),
        tx_hash,
        cancel_tx_hash,
        replacement_tx_hash,
    }))
}

/// The intent of the entry Bloom finds under `id`, `Null` when unreadable.
/// Bloom looks in `pending/`, then `sent/`, then `failed/`, and so does this.
fn reported_intent<H: Host>(
    host: &mut H,
    account: &WalletAccount,
    chain: Chain,
    id: &str,
) -> Result<Option<Value>> {
    for dir in ["pending", "sent", "failed"] {
        let path = format!("{}/intent.json", outbox_entry(account, chain, dir, id)?);
        if let Some(intent) = host.vfs_file(&path, 256 * 1024)? {
            return Ok(Some(serde_json::from_slice(&intent).unwrap_or(Value::Null)));
        }
    }
    Ok(None)
}
/// Whether a Bloom intent holds exactly this transaction.
fn holds(intent: &Value, tx: &evm::Transaction) -> bool {
    let field = |name: &str| intent[name].as_str().map(str::to_ascii_lowercase);
    field("to") == Some(tx.to.to_ascii_lowercase())
        && field("value_wei") == Some(tx.value.to_ascii_lowercase())
        && field("data_hex") == Some(tx.data.to_ascii_lowercase())
}
/// A fee bump or cancel sent for a sent entry that Bloom does not follow.
/// Bloom 0.4.1 follows only the entry's original hash, so a bump sent with
/// `replace`, or a cancel it failed to move to `failed/`, may mine unseen.
/// Bloom after 0.4.1 makes a fee repair the entry's own hash, and reports a
/// cancel it follows as `cancelling`.
struct Unfollowed {
    replacement_tx_hash: Option<String>,
    cancel_tx_hash: Option<String>,
}
fn sent_unfollowed<H: Host>(
    host: &mut H,
    account: &WalletAccount,
    chain: Chain,
    id: &str,
    tx_hash: Option<&str>,
    cancelling: bool,
) -> Result<Option<Unfollowed>> {
    let dir = outbox_entry(account, chain, "sent", id)?;
    let mut read = |file: &str| host.vfs_file(&format!("{dir}/{file}"), 64 * 1024);
    let bumped = read("replacement_tx_hash")?;
    let bump_marker = read("replacement_broadcast_attempted.json")?;
    let cancel = read("cancel_tx_hash")?;
    let cancel_marker = read("cancel_broadcast_attempted.json")?;
    let replacement_tx_hash =
        text_hash(bumped.as_deref()).or_else(|| bump_marker.as_deref().and_then(recorded_hash));
    let cancel_tx_hash =
        text_hash(cancel.as_deref()).or_else(|| cancel_marker.as_deref().and_then(recorded_hash));
    let bump_unfollowed = (bumped.is_some() || bump_marker.is_some())
        && (replacement_tx_hash.is_none() || replacement_tx_hash.as_deref() != tx_hash);
    let cancel_unfollowed = (cancel.is_some() || cancel_marker.is_some()) && !cancelling;
    Ok(
        (bump_unfollowed || cancel_unfollowed).then_some(Unfollowed {
            replacement_tx_hash,
            cancel_tx_hash,
        }),
    )
}
/// A sent entry read from its own files, when it is this step's: Bloom
/// reports the first entry under a reused id, which may be another one.
struct OwnSent {
    state: &'static str,
    tx_hash: Option<String>,
    unfollowed: Option<Unfollowed>,
}
fn own_sent_entry<H: Host>(
    host: &mut H,
    account: &WalletAccount,
    chain: Chain,
    id: &str,
    created_ms: u64,
) -> Result<Option<OwnSent>> {
    let dir = outbox_entry(account, chain, "sent", id)?;
    let Some(intent) = host.vfs_file(&format!("{dir}/intent.json"), 256 * 1024)? else {
        return Ok(None);
    };
    let intent: Value = serde_json::from_slice(&intent).unwrap_or(Value::Null);
    if intent["created_ms"].as_u64() != Some(created_ms) {
        return Ok(None);
    }
    let tx_hash = intent["tx_hash"].as_str().map(String::from);
    let receipt = host.vfs_file(&format!("{dir}/receipt.json"), 64 * 1024)?;
    if let Some(receipt) = receipt.and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok()) {
        return Ok(Some(OwnSent {
            state: receipt_state(&receipt),
            tx_hash,
            unfollowed: None,
        }));
    }
    let unfollowed = sent_unfollowed(host, account, chain, id, tx_hash.as_deref(), false)?;
    Ok(Some(OwnSent {
        state: if unfollowed.is_some() {
            "broadcast_unknown"
        } else {
            "submitted"
        },
        tx_hash,
        unfollowed,
    }))
}
/// Reads a step's own entry in `sent/` or `failed/` when Bloom reports
/// another one or refuses to report it. Returns whether it found one.
fn read_own_entry<H: Host>(
    host: &mut H,
    account: &WalletAccount,
    chain: Chain,
    id: &str,
    created_ms: u64,
    step: &mut Step,
) -> Result<bool> {
    if let Some(own) = own_sent_entry(host, account, chain, id, created_ms)? {
        step.state = own.state.into();
        step.tx_hash = own.tx_hash;
        if let Some(unfollowed) = own.unfollowed {
            step.replacement_tx_hash = unfollowed.replacement_tx_hash;
            step.cancel_tx_hash = unfollowed.cancel_tx_hash;
        }
        return Ok(true);
    }
    if let Some(gone) = discarded(host, account, chain, id, Some(created_ms))? {
        mark_discarded(step, gone, false);
        return Ok(true);
    }
    Ok(false)
}
/// A step's state from Bloom's normalized receipt, which records its
/// outcome explicitly.
fn receipt_state(receipt: &Value) -> &'static str {
    match receipt["outcome"]
        .as_str()
        .or_else(|| receipt["status"].as_str())
    {
        Some("success") | Some("0x1") | Some("1") => "confirmed",
        Some("reverted") | Some("0x0") | Some("0") => "reverted",
        // Bloom after 0.4.1: the same-nonce cancel mined, so this
        // transaction can never land.
        Some("cancelled") => "failed",
        _ => "submitted",
    }
}
/// A step whose entry Bloom moved to `failed/`.
fn mark_discarded(step: &mut Step, gone: Discarded, signing_unknown: bool) {
    if !gone.maybe_sent && step.tx_hash.is_none() && !signing_unknown {
        // Only an entry Bloom never tried to send is safe to abandon for a
        // new intent.
        step.state = "failed".into();
    } else {
        // Bloom never reconciles an entry in `failed/`, so even a cancelled
        // send may have mined: check the hashes on chain.
        step.tx_hash = step.tx_hash.take().or(gone.tx_hash);
        step.cancel_tx_hash = gone.cancel_tx_hash;
        step.replacement_tx_hash = gone.replacement_tx_hash;
        step.state = "broadcast_unknown".into();
    }
}

/// What a pending entry records of attempts to send it.
struct Attempts {
    /// Bloom submitted it, under this hash when one is readable. A lost
    /// submit reply leaves the hash only in Bloom's broadcast marker.
    sent: Option<Option<String>>,
    /// Bloom also sent a same-nonce replacement or cancel for it.
    replaced: bool,
    replacement_tx_hash: Option<String>,
    cancel_tx_hash: Option<String>,
    /// Bloom could not settle a lost submit reply.
    ambiguous: bool,
    /// Bloom recorded why, and it resends the same signed transaction only
    /// after a failed public resubmit. For any other reason (the nonce moved
    /// past it, another transaction holds the nonce, a private relay, or a
    /// policy now asking for private routing) it refuses every confirm until
    /// the hash shows on chain.
    stuck: bool,
}
impl Attempts {
    fn unsent(&self) -> bool {
        self.sent.is_none() && !self.replaced && !self.ambiguous
    }
}
/// The send attempts recorded in `pending/<id>/`, or `None` when the entry
/// is not pending.
fn pending_attempts<H: Host>(
    host: &mut H,
    account: &WalletAccount,
    chain: Chain,
    id: &str,
) -> Result<Option<Attempts>> {
    let dir = outbox_entry(account, chain, "pending", id)?;
    let mut read = |file: &str| host.vfs_file(&format!("{dir}/{file}"), 256 * 1024);
    let Some(intent) = read("intent.json")? else {
        return Ok(None);
    };
    let attempt = read("broadcast_attempted.json")?;
    let replacement = read("replacement_broadcast_attempted.json")?;
    let replacement_hash = read("replacement_tx_hash")?;
    let cancel = read("cancel_broadcast_attempted.json")?;
    let cancel_hash = read("cancel_tx_hash")?;
    let ambiguous = read("reconcile_ambiguous.json")?;
    Ok(Some(Attempts {
        sent: match (recorded_hash(&intent), attempt) {
            (Some(hash), _) => Some(Some(hash)),
            (None, Some(marker)) => Some(recorded_hash(&marker)),
            (None, None) => None,
        },
        replaced: replacement.is_some()
            || replacement_hash.is_some()
            || cancel.is_some()
            || cancel_hash.is_some(),
        replacement_tx_hash: text_hash(replacement_hash.as_deref())
            .or_else(|| replacement.as_deref().and_then(recorded_hash)),
        cancel_tx_hash: text_hash(cancel_hash.as_deref())
            .or_else(|| cancel.as_deref().and_then(recorded_hash)),
        stuck: ambiguous.as_deref().is_some_and(|ambiguous| {
            !serde_json::from_slice::<Value>(ambiguous)
                .ok()
                .and_then(|ambiguous| {
                    ambiguous["reason"]
                        .as_str()
                        .map(|reason| reason.starts_with("public resubmit failed:"))
                })
                .unwrap_or(false)
        }),
        ambiguous: ambiguous.is_some(),
    }))
}
/// The file that discards a pending entry Bloom cannot have sent or signed.
/// Bloom 0.4.1 discards whatever is written `cancel` there, and its signing
/// state is private, so the Petal names it only for an entry with no send
/// attempt whose owner approval, if any, still awaits its ceremony: once
/// the owner approves, Bloom may be signing it. `approved` says this
/// operation recorded a transaction approval for it.
fn unsent_entry<H: Host>(
    host: &mut H,
    account: &WalletAccount,
    chain: Chain,
    id: &str,
    approved: bool,
) -> Result<Option<String>> {
    if !pending_attempts(host, account, chain, id)?.is_some_and(|attempts| attempts.unsent()) {
        return Ok(None);
    }
    let awaiting = match approval_state(host, account, chain, id)? {
        None => !approved,
        Some(state) => state == "awaiting_ceremony",
    };
    Ok(awaiting
        .then(|| outbox_entry(account, chain, "pending", id))
        .transpose()?
        .map(|dir| format!("/{dir}/confirm")))
}
/// The live state of a pending entry's owner approval, from
/// `approval_challenge.json`, which Bloom shows while the entry has one:
/// `None` without one, `unreadable` when it cannot be read.
fn approval_state<H: Host>(
    host: &mut H,
    account: &WalletAccount,
    chain: Chain,
    id: &str,
) -> Result<Option<String>> {
    let dir = outbox_entry(account, chain, "pending", id)?;
    Ok(
        match host.vfs_file(&format!("{dir}/approval_challenge.json"), 64 * 1024) {
            Ok(None) => None,
            Ok(Some(challenge)) => Some(
                serde_json::from_slice::<Value>(&challenge)
                    .ok()
                    .and_then(|challenge| challenge["state"].as_str().map(String::from))
                    .unwrap_or_else(|| "unreadable".into()),
            ),
            Err(_) => Some("unreadable".into()),
        },
    )
}
/// The advice to discard a step's unsent entry first: until then it holds
/// its nonce, and Bloom stages later transactions behind it.
fn discard_first<H: Host>(
    host: &mut H,
    account: &WalletAccount,
    chain: Chain,
    step: Option<&Step>,
) -> Result<String> {
    let Some(id) = step.and_then(|step| step.outbox_id.clone()) else {
        return Ok(String::new());
    };
    let approved = step
        .and_then(|step| step.approval.as_ref())
        .is_some_and(|approval| approval.kind == "transaction");
    Ok(unsent_entry(host, account, chain, &id, approved)?
        .map(|path| {
            format!("; first discard its unsent outbox entry by writing `cancel` to {path}")
        })
        .unwrap_or_default())
}

/// The operation's state follows from its steps, so it never goes stale.
/// The first step that is not confirmed says what happens next.
fn settle(draft: &mut Draft, now_ms: u64) {
    let any = |state: &str| draft.steps.iter().any(|step| step.state == state);
    let state = if any("reverted") {
        "reverted".to_owned()
    } else if any("broadcast_unknown") {
        "broadcast_unknown".into()
    } else if any("failed") {
        "failed".into()
    } else if any("staging_unknown") {
        "staging_unknown".into()
    } else if any("submitted") {
        "submitted".into()
    } else if let Some(next) = draft.steps.iter().find(|step| step.state != "confirmed") {
        // After the quote expires, only a step whose transaction approval
        // Bloom already requested, or that Bloom may already have sent, may
        // continue: confirming it stages and approves nothing new.
        let requested = next.tx_hash.is_some()
            || next
                .approval
                .as_ref()
                .is_some_and(|approval| approval.kind == "transaction");
        if now_ms > draft.expires_ms && !requested {
            "expired".into()
        } else if next.approval.is_some() {
            "approval_required".into()
        } else {
            // `prepared` or `staged`: the next confirm stages or sends it.
            next.state.clone()
        }
    } else {
        // A successful receipt proves execution; independent protocol/venue
        // output accounting is deliberately not inferred from an API quote.
        "execution_confirmed".into()
    };
    draft.state = state;
}

/// A transaction approval carries its outbox entry's action id. The only
/// other approval a confirm asks for is the wallet's pending policy update.
fn approval_kind<H: Host>(
    host: &mut H,
    account: &WalletAccount,
    chain: Chain,
    id: &str,
    action_id: &str,
) -> &'static str {
    let intent = outbox_entry(account, chain, "pending", id)
        .and_then(|dir| host.vfs_file(&format!("{dir}/intent.json"), 256 * 1024));
    match intent.map(|bytes| bytes.and_then(|b| serde_json::from_slice::<Value>(&b).ok())) {
        Ok(Some(intent)) if intent["action_id"].as_str() == Some(action_id) => "transaction",
        Ok(Some(_)) => "wallet_policy",
        _ => "unknown",
    }
}

/// Reconcile only: status reads never stage, sign or broadcast transactions.
pub fn reconcile<H: Host>(
    host: &mut H,
    chain: Chain,
    account: &WalletAccount,
    op: Operation,
    id: &str,
) -> Result<Draft> {
    let mut draft = load(host, chain, account, op, id)?;
    for step in &mut draft.steps {
        let Some(id) = step.outbox_id.clone() else {
            // Nothing of this step was staged, so nothing of it was sent.
            step.tx_hash = None;
            step.cancel_tx_hash = None;
            step.replacement_tx_hash = None;
            step.approval = None;
            continue;
        };
        // An entry still being claimed is not this operation's yet.
        if matches!(
            step.state.as_str(),
            "confirmed" | "reverted" | "staging_unknown"
        ) {
            continue;
        }
        // Outbox ids repeat after Bloom restarts, and Bloom reports the entry
        // it finds first, which may be another one: then read this step's.
        if let Some(created) = step.outbox_created_ms
            && reported_intent(host, account, chain, &id)?
                .is_some_and(|intent| intent["created_ms"].as_u64() != Some(created))
        {
            if !read_own_entry(host, account, chain, &id, created, step)? {
                step.state = "broadcast_unknown".into();
            }
            step.approval = None;
            continue;
        }
        let observed = match host.inspect(account.wallet(), chain, &id) {
            Ok(observed) => observed,
            // Bloom refuses to report an entry another account holds under
            // a reused id: read this step's own instead.
            Err(error) => {
                let created = step.outbox_created_ms;
                match created {
                    Some(created) if read_own_entry(host, account, chain, &id, created, step)? => {
                        step.approval = None;
                        continue;
                    }
                    _ => return Err(error),
                }
            }
        };
        step.tx_hash = observed.tx_hash;
        if let Some(receipt) = observed.receipt_json {
            let receipt: Value =
                serde_json::from_str(&receipt).map_err(|_| "invalid outbox receipt")?;
            step.state = receipt_state(&receipt).into();
        } else if let Some(gone) = discarded(host, account, chain, &id, step.outbox_created_ms)? {
            mark_discarded(step, gone, observed.state == "signing_unknown");
        } else if let Some(attempts) = pending_attempts(host, account, chain, &id)? {
            step.tx_hash = step.tx_hash.take().or(attempts.sent.clone().flatten());
            if attempts.replaced || attempts.stuck || observed.state == "cancelling" {
                // A same-nonce replacement or cancel went out, or Bloom will
                // not resend a lost reply. Bloom keeps the entry pending and
                // never settles it by itself: check the hashes on chain.
                step.cancel_tx_hash = attempts.cancel_tx_hash;
                step.replacement_tx_hash = attempts.replacement_tx_hash;
                step.state = "broadcast_unknown".into();
            } else if attempts.sent.is_some() || step.tx_hash.is_some() {
                // A lost submit reply, or a resend that failed. Bloom settles
                // or resends that attempt when confirm is written again, so
                // the step stays confirmable, quote or not; without a
                // readable hash it may simply have gone out.
                step.state = if step.tx_hash.is_some() {
                    "staged"
                } else {
                    "broadcast_unknown"
                }
                .into();
            }
        } else if step.tx_hash.is_some() || observed.state == "cancelling" {
            // Sent: Bloom follows it, and after 0.4.1 its cancel or fee
            // repair too, until one of them mines. Anything sent that Bloom
            // cannot follow leaves its hashes to check on chain.
            let cancelling = observed.state == "cancelling";
            match sent_unfollowed(
                host,
                account,
                chain,
                &id,
                step.tx_hash.as_deref(),
                cancelling,
            )? {
                Some(unfollowed) => {
                    step.replacement_tx_hash = unfollowed.replacement_tx_hash;
                    step.cancel_tx_hash = unfollowed.cancel_tx_hash;
                    step.state = "broadcast_unknown".into();
                }
                None => step.state = "submitted".into(),
            }
        } else if matches!(
            observed.state.as_str(),
            "failed" | "cancelled" | "signing_unknown"
        ) {
            // Reported discarded, but not where Bloom keeps those.
            step.state = "broadcast_unknown".into();
        }
        // A transaction approval is pending only while an unsent entry
        // waits for it. A wallet-policy approval may still be needed to let
        // Bloom settle a lost reply.
        let transaction = step
            .approval
            .as_ref()
            .is_some_and(|approval| approval.kind == "transaction");
        if step.state != "staged" || (step.tx_hash.is_some() && transaction) {
            step.approval = None;
        }
    }
    settle(&mut draft, host.now_ms());
    Ok(draft)
}

pub fn confirm<H: Host>(
    host: &mut H,
    chain: Chain,
    account: &WalletAccount,
    op: Operation,
    id: &str,
    body: &[u8],
    package: &str,
) -> Result<()> {
    if body != b"confirm" {
        return Err("write exactly 'confirm' to advance one reviewed outbox step".into());
    }
    let lock = lock_key(chain, op, id)?;
    let token = format!("{}:{}", account.wallet(), host.now_ms()).into_bytes();
    host.put_new(&lock, &token)?;
    let result = advance(host, chain, account, op, id, package);
    let released = host.delete_if(&lock, &token);
    result.and(released)
}
fn advance<H: Host>(
    host: &mut H,
    chain: Chain,
    account: &WalletAccount,
    op: Operation,
    id: &str,
    package: &str,
) -> Result<()> {
    let mut draft = reconcile(host, chain, account, op, id)?;
    match draft.state.as_str() {
        "execution_confirmed" => return save(host, &draft),
        "reverted" => {
            return Err(
                "operation requires reconciliation or manual recovery; it cannot be resubmitted"
                    .into(),
            );
        }
        // A lost stage reply leaves no outbox id: never stage again.
        "staging_unknown"
            if draft
                .steps
                .iter()
                .any(|step| step.state == "staging_unknown" && step.outbox_id.is_none()) =>
        {
            return Err(
                "staging outcome unknown; inspect the account outbox before recovery".into(),
            );
        }
        "broadcast_unknown" => {
            return Err(
                "Bloom no longer sends an outbox step but may have broadcast it; check its hash on chain before creating a new intent"
                    .into(),
            );
        }
        "failed" => {
            let refused = draft
                .steps
                .iter()
                .find(|step| step.state == "failed")
                .and_then(|step| step.outbox_id.clone())
                .and_then(|id| policy_refusals(host, account, chain, &id))
                .map(|reasons| format!(". Bloom's policy refused it: {reasons}"))
                .unwrap_or_default();
            return Err(format!(
                "Bloom refused, cancelled or expired an outbox step that can no longer land; reconcile prior steps and create a new reviewed intent{refused}"
            ));
        }
        "submitted" => return Err("transaction submitted; poll status before advancing".into()),
        // An expired quote stages nothing new. Only a step whose transaction
        // approval Bloom already requested, or that Bloom may already have
        // sent, may continue, so a slow ceremony does not strand an approved
        // outbox entry. A step that has only asked for the wallet-policy
        // approval stops with the quote.
        "expired" => {
            let staged = draft
                .steps
                .iter()
                .find(|step| step.state == "staged")
                .cloned();
            let discard = discard_first(host, account, chain, staged.as_ref())?;
            return Err(format!(
                "quote expired; reconcile prior steps and create a new reviewed intent{discard}"
            ));
        }
        _ => {}
    }
    let index = draft
        .steps
        .iter()
        .position(|step| step.state != "confirmed")
        .ok_or("no executable step")?;
    let (execution, approvals) = draft.steps.split_last().ok_or("execution step missing")?;
    evm::validate_transaction(
        &execution.transaction,
        &draft.owner,
        op,
        &draft.request,
        &draft.market,
    )?;
    // The whole sequence is rebuilt from the intent rather than trusted from
    // the store: at most a reset to zero, then the exact input, both to the
    // router for the token being spent, then the one Pendle execution.
    let exact = amount(&draft.request.amount)?;
    let expected = match approvals.len() {
        0 => vec![],
        1 => vec![("approve_input", exact)],
        2 => vec![("reset_allowance", U256::ZERO), ("approve_input", exact)],
        _ => return Err("operation steps do not match the reviewed intent".into()),
    };
    let token = if op == Operation::BuyPt {
        draft.request.token.clone()
    } else {
        draft.market.pt.address.clone()
    };
    if execution.kind != "pendle_execution"
        || approvals
            .iter()
            .zip(&expected)
            .any(|(step, (kind, value))| {
                step.kind != *kind
                    || step.transaction != evm::approval(&token, &draft.owner, *value)
            })
    {
        return Err("operation steps do not match the reviewed intent".into());
    }
    // Bloom staged this step's entry, but the claim was not recorded.
    if draft.steps[index].state == "staging_unknown" {
        return claim(host, &mut draft, account, chain, op, id, index);
    }
    // Pendle may drop a market after an incident, so the curated list gates
    // every buy up to the moment the Petal asks Bloom to send it. A sell or
    // redemption drafted before the drop can still finish: it deposits
    // nothing, and its encoded minimum bounds what it pays.
    let deposit = op == Operation::BuyPt;
    // A step Bloom may already have sent is Bloom's to settle: confirming it
    // stages, approves and signs nothing new, so nothing else is checked,
    // except that a buy is not sent again into a market Pendle has dropped.
    if let Some(hash) = draft.steps[index].tx_hash.clone() {
        if deposit
            && index + 1 == draft.steps.len()
            && let Some(refusal) = api::uncurated(host, chain, &draft.request.market)?
        {
            let cancel = draft.steps[index]
                .outbox_id
                .as_deref()
                .map(|id| outbox_entry(account, chain, "pending", id))
                .transpose()?
                .map(|dir| {
                    format!(
                        "; if it is not, the owner can replace it with a same-nonce cancel by writing `y` to /{dir}/cancel"
                    )
                })
                .unwrap_or_default();
            return Err(format!(
                "{refusal}, and Bloom may already have sent this step's transaction {hash}: check it on chain. Until Pendle lists the market again, the Petal does not ask Bloom to send it again{cancel}"
            ));
        }
    } else {
        // Recheck the chain, factory membership and maturity on every write,
        // and the curated list for a buy. Until Pendle lists a dropped
        // market again, a staged entry for this step holds its nonce, and
        // Bloom stages later transactions behind it. A failed lookup is not
        // Pendle's answer, so it gets no discard advice.
        if deposit && let Some(refusal) = api::uncurated(host, chain, &draft.request.market)? {
            let discard = discard_first(host, account, chain, draft.steps.get(index))?;
            return Err(format!(
                "{refusal}; reconcile prior steps and create a new reviewed intent for a listed market{discard}"
            ));
        }
        evm::verify_market(
            host,
            chain,
            &draft.market,
            op,
            address(&draft.request.token)?,
        )?;
        // Operations spending the same token share the router's allowance,
        // and one exact approval replaces another. Never stage or send a
        // Pendle transaction the mined allowance cannot cover, since it would
        // revert. A transaction still in flight is not counted.
        if index + 1 == draft.steps.len() {
            let allowance = evm::call(
                host,
                chain,
                address(&token)?,
                evm::allowanceCall {
                    owner: address(&draft.owner)?,
                    spender: address(ROUTER)?,
                },
            )?;
            if allowance < exact {
                let discard = discard_first(host, account, chain, draft.steps.get(index))?;
                return Err(format!(
                    "the router may spend {allowance} of the input token, less than this operation's {exact}: another operation spent or replaced its approval; create a new reviewed intent{discard}"
                ));
            }
        }
    }
    if let Some(outbox_id) = draft.steps[index].outbox_id.clone() {
        // tx_confirm retains the exact existing outbox id across ceremonies.
        match host.confirm(account.wallet(), chain, &outbox_id) {
            Ok(outcome) => {
                // ApprovalPending is a successful host response, not a
                // broadcast. Keep the same step confirmable after ceremony.
                if let Some(approval) = outcome.approval {
                    let kind = approval_kind(host, account, chain, &outbox_id, &approval.action_id);
                    // Bloom hands back a pending wallet-policy update before
                    // it looks at the entry. The step keeps the transaction
                    // approval Bloom asked for, which lets it continue after
                    // the quote expires.
                    let requested = draft.steps[index]
                        .approval
                        .as_ref()
                        .is_some_and(|approval| approval.kind == "transaction");
                    if kind == "wallet_policy" && requested {
                        settle(&mut draft, host.now_ms());
                        save(host, &draft)?;
                        return Err(format!(
                            "Bloom asks for wallet-policy update {}, under /wallets/{}/policy-updates/pending/, before it uses this step's transaction approval; approve or cancel that update on its page, wait a few seconds after a cancel, then confirm again",
                            approval.action_id,
                            account.wallet()
                        ));
                    }
                    draft.steps[index].approval = Some(Approval {
                        action_id: approval.action_id,
                        expires_ms: approval.expires_ms,
                        kind: kind.into(),
                        package: (kind == "wallet_policy").then(|| package.to_owned()),
                    });
                    draft.steps[index].state = "staged".into();
                } else {
                    draft.steps[index].approval = None;
                    draft.steps[index].state = "submitted".into();
                }
                settle(&mut draft, host.now_ms());
                save(host, &draft)
            }
            Err(error) => {
                // A refusal is no evidence that Bloom asked for an approval.
                // Keep the approval Bloom last reported, so a retry after a
                // granted ceremony still works once the quote has expired.
                settle(&mut draft, host.now_ms());
                save(host, &draft)?;
                let step = draft.steps[index].clone();
                match refusal_detail(host, account, chain, &step, &error) {
                    Some(detail) => Err(format!("{error}: {detail}")),
                    // No pending entry holds the missing nonce, so this one
                    // can never be sent, and it holds its own nonce for
                    // every later transaction until it is discarded.
                    None if crate::host::unheld_nonce(&error).is_some() => {
                        let discard = discard_first(host, account, chain, Some(&step))?;
                        Err(format!("{error}{discard}"))
                    }
                    None => Err(error),
                }
            }
        }
    } else {
        // Persist ambiguity before crossing the host boundary. A lost reply
        // must never create a second approval or execution on retry.
        draft.steps[index].state = "staging_unknown".into();
        settle(&mut draft, host.now_ms());
        save(host, &draft)?;
        let tx = &draft.steps[index].transaction;
        let staged = host.stage(&EvmTransaction {
            wallet: account.wallet().into(),
            chain: chain.name().into(),
            to: tx.to.clone(),
            value_wei: tx.value.clone(),
            data_hex: tx.data.clone(),
            nonce: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
        })?;
        // Record the entry before claiming it, so a failed claim is retried
        // rather than leaving the step for manual recovery.
        draft.steps[index].outbox_id = Some(outbox_id(&staged.outbox_id)?.to_owned());
        save(host, &draft)?;
        claim(host, &mut draft, account, chain, op, id, index)
    }
}

/// What to add to a confirm Bloom refused. A refusal must never read as
/// "not sent" when Bloom may have sent the step. Otherwise Bloom's own
/// reason stands, unless the SDK reduced it to a bare status ("denied",
/// "not found", "invalid"): then the step's own outbox entry says what
/// happened. A file that cannot be read says nothing.
fn refusal_detail<H: Host>(
    host: &mut H,
    account: &WalletAccount,
    chain: Chain,
    step: &Step,
    error: &str,
) -> Option<String> {
    let id = step.outbox_id.as_deref()?;
    let attempts = pending_attempts(host, account, chain, id).ok().flatten();
    let gone = discarded(host, account, chain, id, step.outbox_created_ms)
        .ok()
        .flatten();
    // Bloom may move an entry to `sent/` and still report an error.
    let sent = outbox_entry(account, chain, "sent", id)
        .and_then(|dir| host.vfs_file(&format!("{dir}/intent.json"), 256 * 1024))
        .ok()
        .flatten()
        .is_some_and(|intent| {
            let created = serde_json::from_slice::<Value>(&intent)
                .ok()
                .and_then(|intent| intent["created_ms"].as_u64());
            step.outbox_created_ms.is_none() || created == step.outbox_created_ms
        });
    if sent
        || step.tx_hash.is_some()
        || attempts.as_ref().is_some_and(|attempts| !attempts.unsent())
        || gone.as_ref().is_some_and(|gone| gone.maybe_sent)
    {
        return Some(
            "Bloom may already have sent it, so read status and check its hash on chain before creating a new intent".into(),
        );
    }
    let denied = error == "Bloom denied the transaction";
    if !denied && error != "Bloom has not confirmed the transaction" {
        return None;
    }
    if gone.is_some() {
        return Some(match policy_refusals(host, account, chain, id) {
            Some(reasons) => format!(
                "Bloom's policy refused it and discarded the entry unsent: {reasons}; status shows the operation failed"
            ),
            None => "Bloom discarded the entry unsent, as it does after a policy refusal, a refused approval or a ceremony limit; status shows the operation failed".into(),
        });
    }
    Some(
        if denied {
            "Bloom kept the entry. It denies without discarding when this route may not use it (an entry staged by another build or another account, a missing write permit, an unavailable outbox, or a Broker eligibility failure), when a wallet-policy approval is pending, or when an owner approval cannot be used; inspect the outbox entry and the wallet's policy updates"
        } else {
            "Bloom gave no reason the Petal can show; inspect the outbox entry"
        }
        .into(),
    )
}

/// The hard policy refusals Bloom recorded for an entry it discarded, such
/// as an account with no native token for gas.
fn policy_refusals<H: Host>(
    host: &mut H,
    account: &WalletAccount,
    chain: Chain,
    id: &str,
) -> Option<String> {
    let dir = outbox_entry(account, chain, "failed", id).ok()?;
    let checks = host
        .vfs_file(&format!("{dir}/policy_check.json"), 64 * 1024)
        .ok()
        .flatten()?;
    let checks: Value = serde_json::from_slice(&checks).ok()?;
    let refused: Vec<String> = checks
        .as_array()?
        .iter()
        .filter(|check| check["outcome"] == "deny")
        .filter_map(|check| {
            Some(format!(
                "{}: {}",
                check["rule"].as_str()?,
                check["message"].as_str()?
            ))
        })
        .collect();
    (!refused.is_empty()).then(|| crate::host::bloom_reason(&refused.join("; ")))
}

/// Claims the outbox entry Bloom staged for step `index`. While an entry is
/// pending, Bloom returns it instead of staging another for the same
/// package, route, sender, target, value and calldata, and two drafts of one
/// intent, approvals of the same token and amount, and every allowance reset
/// have the same calldata. Each entry belongs to one operation, so one
/// approval and one swap never complete two. Outbox ids repeat after Bloom
/// restarts, so the claim also names when Bloom created the entry.
fn claim<H: Host>(
    host: &mut H,
    draft: &mut Draft,
    account: &WalletAccount,
    chain: Chain,
    op: Operation,
    id: &str,
    index: usize,
) -> Result<()> {
    let entry = draft.steps[index]
        .outbox_id
        .clone()
        .ok_or("outbox entry missing")?;
    // An id Bloom reused after a restart may name another entry first, so
    // the claim takes the first entry under it that holds this transaction.
    let mut intent = None;
    let mut seen = false;
    for dir in ["pending", "sent", "failed"] {
        let path = format!("{}/intent.json", outbox_entry(account, chain, dir, &entry)?);
        let Some(bytes) = host.vfs_file(&path, 256 * 1024)? else {
            continue;
        };
        seen = true;
        let found = serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null);
        if holds(&found, &draft.steps[index].transaction) {
            intent = Some(found);
            break;
        }
    }
    let Some(intent) = intent else {
        return Err(if seen {
            format!(
                "Bloom's outbox entry {entry} holds another transaction; inspect the account outbox before recovery"
            )
        } else {
            "Bloom's outbox entry cannot be read".into()
        });
    };
    let created = intent["created_ms"]
        .as_u64()
        .ok_or("Bloom's outbox entry has no readable creation time")?;
    let key = format!(
        "outbox-claims/{}/{}-{created}",
        chain.name(),
        outbox_id(&entry)?
    );
    let mine = format!("{}/{}/{index}", op.name(), operation_id(id)?);
    if let Err(error) = host.put_new(&key, mine.as_bytes()) {
        match host.get(&key)? {
            Some(holder) if holder == mine.as_bytes() => {}
            Some(holder) => {
                let holder = String::from_utf8_lossy(&holder).into_owned();
                // The claim names `<op>/<id>/<step>`: confirm that operation.
                let route = match holder.split('/').collect::<Vec<_>>()[..] {
                    [kind, holder_id, _] => format!(
                        "/petals/pendle/{}/{kind}/{}/{}/{holder_id}/confirm.json",
                        chain.name(),
                        account.wallet(),
                        account.number()
                    ),
                    _ => holder.clone(),
                };
                let attempts = pending_attempts(host, account, chain, &entry)?;
                let unsent = attempts.as_ref().is_some_and(Attempts::unsent);
                let approval = if unsent {
                    approval_state(host, account, chain, &entry)?
                } else {
                    None
                };
                let advice = match unsent_entry(host, account, chain, &entry, false)? {
                    Some(path) => format!(
                        "finish that operation, or discard the entry by writing `cancel` to {path}"
                    ),
                    None if attempts.is_some_and(|attempts| attempts.replaced || attempts.stuck) => {
                        "it may have been sent and Bloom no longer settles it: check its hashes on chain, as Bloom hands the entry out until it expires an hour after staging".into()
                    }
                    None if approval.as_deref() == Some("active") => format!(
                        "the owner approved it, so finish that operation by writing `confirm` to {route}"
                    ),
                    None if matches!(
                        approval.as_deref(),
                        Some("expired" | "cancelled" | "refused")
                    ) =>
                    {
                        "its owner approval has ended, and Bloom hands the entry out until it expires an hour after staging".into()
                    }
                    None => format!(
                        "it may have been sent or signed, so write `confirm` to {route} to let Bloom settle it"
                    ),
                };
                // Nothing of this operation is staged: try again once the
                // other operation has finished.
                draft.steps[index].outbox_id = None;
                draft.steps[index].state = "prepared".into();
                settle(draft, host.now_ms());
                save(host, draft)?;
                return Err(format!(
                    "Bloom returned pending outbox entry {entry} of operation {holder}; {advice}, then confirm this one again"
                ));
            }
            None => return Err(error),
        }
    }
    draft.steps[index].outbox_created_ms = Some(created);
    draft.steps[index].state = "staged".into();
    settle(draft, host.now_ms());
    save(host, draft)
}

/// A Pendle API string made safe to print in plan.md: at most 64 characters,
/// with no line breaks, parentheses or other markup.
fn plain(value: &str) -> String {
    value
        .chars()
        .take(64)
        .map(|c| {
            if c.is_alphanumeric() || " -_.:+/".contains(c) {
                c
            } else {
                '?'
            }
        })
        .collect()
}
/// A token's address, Pendle's symbol for it and its onchain decimals, so
/// base units can be read.
fn describe(market: &api::Market, token: &str, decimals: u8) -> String {
    let symbol = [
        &market.pt,
        &market.yt,
        &market.sy,
        &market.underlying_asset,
        &market.accounting_asset,
    ]
    .into_iter()
    .chain(&market.input_tokens)
    .chain(&market.output_tokens)
    .find(|t| t.address.eq_ignore_ascii_case(token))
    .map_or_else(|| "not named by the market".into(), |t| plain(&t.symbol));
    format!("{token} ({symbol}, {decimals} decimals)")
}

pub fn plan(draft: &Draft) -> String {
    format!(
        "# Pendle {}\n\nWallet/account: {}/{}\nChain: {} ({})\nOwner/receiver: {}\nMarket: {}\nMaturity: {}\nInput token: {}\nInput base units: {}\nOutput token: {}\nExpected output base units (API estimate): {}\nMinimum output enforced in calldata: {}\nPrice impact (API estimate): {}\nQuote expires at: {} ms\nAccounting asset: {} ({})\nUnderlying asset: {} ({})\n\nThe input and output tokens are checked onchain, and their decimals are read from them. Symbols, the maturity date and the accounting and underlying assets are as Pendle reports them.\n\n{} outbox steps; each requires its own Bloom authorization. Inspect status.json, then write `confirm` to confirm.json to advance one step. A successful receipt is execution evidence, not an independent APY or redemption-value guarantee. Router: {}.\n",
        draft.operation.name(),
        draft.wallet,
        draft.account,
        draft.chain.name(),
        draft.chain.id(),
        draft.owner,
        draft.request.market,
        plain(&draft.market.expiry),
        describe(&draft.market, &draft.input_token, draft.input_decimals),
        draft.request.amount,
        describe(&draft.market, &draft.output_token, draft.output_decimals),
        draft.expected_output,
        draft.encoded_minimum,
        draft.price_impact,
        draft.expires_ms,
        plain(&draft.market.accounting_asset.symbol),
        plain(&draft.market.accounting_asset.address),
        plain(&draft.market.underlying_asset.symbol),
        plain(&draft.market.underlying_asset.address),
        draft.steps.len(),
        ROUTER
    )
}
