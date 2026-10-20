use crate::{
    DraftInput, Operation, ROUTER, Result, address, amount, api, evm,
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
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Draft {
    pub schema: String,
    pub id: String,
    pub wallet: String,
    pub account: u32,
    pub owner: String,
    pub operation: Operation,
    pub request: DraftInput,
    pub market: api::Market,
    pub input_token: String,
    pub output_token: String,
    pub expected_output: String,
    pub encoded_minimum: String,
    pub price_impact: f64,
    pub created_ms: u64,
    pub expires_ms: u64,
    pub state: String,
    pub steps: Vec<Step>,
}
fn key(op: Operation, id: &str) -> Result<String> {
    Ok(format!(
        "operations/{}/{}.json",
        op.name(),
        operation_id(id)?
    ))
}
fn save<H: Host>(host: &mut H, draft: &Draft) -> Result<()> {
    host.put(
        &key(draft.operation, &draft.id)?,
        &serde_json::to_vec(draft).map_err(|_| "encode operation")?,
    )
}
pub fn load<H: Host>(
    host: &mut H,
    account: &WalletAccount,
    op: Operation,
    id: &str,
) -> Result<Draft> {
    let data = host.get(&key(op, id)?)?.ok_or("operation not found")?;
    let draft: Draft = serde_json::from_slice(&data).map_err(|_| "invalid persisted operation")?;
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
    account: &WalletAccount,
    op: Operation,
    id: &str,
    body: &[u8],
) -> Result<()> {
    let key = key(op, id)?;
    let request = DraftInput::parse(body)?;
    if host.get(&key)?.is_some() {
        let old = load(host, account, op, id)?;
        return if old.request == request {
            Ok(())
        } else {
            Err("id already binds a different intent; choose a new id".into())
        };
    }
    let owner = account_address(host, account)?;
    let market = api::market(host, &request.market)?;
    evm::verify_market(host, &market, op, address(&request.token)?)?;
    let (input_token, output_token) = if op == Operation::BuyPt {
        (request.token.clone(), market.pt.address.clone())
    } else {
        (market.pt.address.clone(), request.token.clone())
    };
    let quote = api::http_json(
        host,
        "POST",
        format!("{}/v3/sdk/{}/convert", crate::API, crate::CHAIN_ID),
        Some(json!({
            "inputs":[{"token":input_token,"amount":request.amount}], "outputs":[output_token],
            "receiver":owner,"slippage":f64::from(request.slippage_bps)/10000.0,"enableAggregator":false
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
    if !impact.is_finite() || impact > f64::from(request.max_price_impact_bps) / 10000.0 {
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
    if minimum > amount(&expected_output)? {
        return Err("quote minimum exceeds expected output".into());
    }
    let current_allowance = evm::call(
        host,
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
        schema: "bloom.pendle.operation.v1".into(),
        id: id.into(),
        wallet: account.wallet().into(),
        account: account.number(),
        owner,
        operation: op,
        request,
        market,
        input_token,
        output_token,
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

/// Reconcile only: status reads never stage, sign or broadcast transactions.
pub fn reconcile<H: Host>(
    host: &mut H,
    account: &WalletAccount,
    op: Operation,
    id: &str,
) -> Result<Draft> {
    let mut draft = load(host, account, op, id)?;
    for step in &mut draft.steps {
        let Some(id) = &step.outbox_id else {
            continue;
        };
        if matches!(step.state.as_str(), "confirmed" | "reverted") {
            continue;
        }
        let observed = host.inspect(account.wallet(), id)?;
        step.tx_hash = observed.tx_hash;
        if let Some(receipt) = observed.receipt_json {
            let receipt: Value =
                serde_json::from_str(&receipt).map_err(|_| "invalid outbox receipt")?;
            // Bloom's normalized receipt records its outcome explicitly.
            match receipt["outcome"]
                .as_str()
                .or_else(|| receipt["status"].as_str())
            {
                Some("success") | Some("0x1") | Some("1") => step.state = "confirmed".into(),
                Some("reverted") | Some("0x0") | Some("0") => step.state = "reverted".into(),
                _ => step.state = "submitted".into(),
            }
        } else if step.tx_hash.is_some() {
            step.state = "submitted".into();
        }
    }
    if draft.steps.iter().any(|s| s.state == "reverted") {
        draft.state = "reverted".into();
    } else if draft.steps.iter().all(|s| s.state == "confirmed") {
        // A successful receipt proves execution; independent protocol/venue
        // output accounting is deliberately not inferred from an API quote.
        draft.state = "execution_confirmed".into();
    }
    Ok(draft)
}

pub fn confirm<H: Host>(
    host: &mut H,
    account: &WalletAccount,
    op: Operation,
    id: &str,
    body: &[u8],
) -> Result<()> {
    if body != b"confirm" {
        return Err("write exactly 'confirm' to advance one reviewed outbox step".into());
    }
    let lock = format!("locks/{}/{}", op.name(), operation_id(id)?);
    let token = format!("{}:{}", account.wallet(), host.now_ms()).into_bytes();
    host.put_new(&lock, &token)?;
    let result = advance(host, account, op, id);
    let released = host.delete_if(&lock, &token);
    result.and(released)
}
fn advance<H: Host>(host: &mut H, account: &WalletAccount, op: Operation, id: &str) -> Result<()> {
    let mut draft = reconcile(host, account, op, id)?;
    if draft.state == "execution_confirmed" {
        return save(host, &draft);
    }
    if matches!(draft.state.as_str(), "reverted" | "staging_unknown") {
        return Err(
            "operation requires reconciliation or manual recovery; it cannot be resubmitted".into(),
        );
    }
    let index = draft
        .steps
        .iter()
        .position(|step| step.state != "confirmed")
        .ok_or("no executable step")?;
    if draft.steps[index].state == "submitted" {
        return Err("transaction submitted; poll status before advancing".into());
    }
    if draft.steps[index].state == "staging_unknown" {
        return Err("staging outcome unknown; inspect the account outbox before recovery".into());
    }
    if host.now_ms() > draft.expires_ms {
        return Err("quote expired; reconcile prior steps and create a new reviewed intent".into());
    }
    // Recheck factory membership, maturity and exact calldata on every write.
    evm::verify_market(host, &draft.market, op, address(&draft.request.token)?)?;
    let execution = &draft
        .steps
        .last()
        .ok_or("execution step missing")?
        .transaction;
    evm::validate_transaction(execution, &draft.owner, op, &draft.request, &draft.market)?;
    if let Some(outbox_id) = draft.steps[index].outbox_id.clone() {
        // tx_confirm retains the exact existing outbox id across ceremonies.
        match host.confirm(account.wallet(), &outbox_id) {
            Ok(outcome) => {
                // ApprovalPending is a successful host response, not a
                // broadcast. Keep the same step confirmable after ceremony.
                if outcome.approval.is_some() {
                    draft.steps[index].state = "staged".into();
                    draft.state = "approval_required".into();
                } else {
                    draft.steps[index].state = "submitted".into();
                    draft.state = "submitted".into();
                }
                save(host, &draft)
            }
            Err(error) => {
                draft.state = "approval_required".into();
                save(host, &draft)?;
                Err(error)
            }
        }
    } else {
        // Persist ambiguity before crossing the host boundary. A lost reply
        // must never create a second approval or execution on retry.
        draft.steps[index].state = "staging_unknown".into();
        draft.state = "staging_unknown".into();
        save(host, &draft)?;
        let tx = &draft.steps[index].transaction;
        let staged = host.stage(&EvmTransaction {
            wallet: account.wallet().into(),
            chain: crate::CHAIN.into(),
            to: tx.to.clone(),
            value_wei: tx.value.clone(),
            data_hex: tx.data.clone(),
            nonce: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
        })?;
        draft.steps[index].outbox_id = Some(staged.outbox_id);
        draft.steps[index].state = "staged".into();
        draft.state = "staged".into();
        save(host, &draft)
    }
}

pub fn plan(draft: &Draft) -> String {
    format!(
        "# Pendle {}\n\nWallet/account: {}/{}\nChain: arbitrum (42161)\nOwner/receiver: {}\nMarket: {}\nMaturity: {}\nInput token: {}\nInput base units: {}\nOutput token: {}\nExpected output base units (API estimate): {}\nMinimum output enforced in calldata: {}\nPrice impact (API estimate): {}\nQuote expires at: {} ms\nAccounting asset: {} ({})\nUnderlying asset: {} ({})\n\n{} outbox steps; each requires its own Bloom authorization. Inspect status.json, then write `confirm` to confirm.json to advance one step. A successful receipt is execution evidence, not an independent APY or redemption-value guarantee. Router: {}.\n",
        draft.operation.name(),
        draft.wallet,
        draft.account,
        draft.owner,
        draft.request.market,
        draft.market.expiry,
        draft.input_token,
        draft.request.amount,
        draft.output_token,
        draft.expected_output,
        draft.encoded_minimum,
        draft.price_impact,
        draft.expires_ms,
        draft.market.accounting_asset.symbol,
        draft.market.accounting_asset.address,
        draft.market.underlying_asset.symbol,
        draft.market.underlying_asset.address,
        draft.steps.len(),
        ROUTER
    )
}
