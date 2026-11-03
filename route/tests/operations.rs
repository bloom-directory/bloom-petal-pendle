use alloy::{
    primitives::{Address, U256},
    sol_types::{SolCall, SolValue},
};
use pendle_route::{api::Market, evm::*, host::Host, workflow, *};
use petal::sdk::{EvmTransaction, HttpRequest, HttpResponse, OutboxInspection, StagedTransaction};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

const OWNER: &str = "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf";
const ARB: Chain = Chain::Arbitrum;
const ETH: Chain = Chain::Ethereum;
const BUY: Operation = Operation::BuyPt;

fn market() -> Market {
    serde_json::from_str(include_str!("fixtures/market.json")).unwrap()
}
fn quote() -> Value {
    serde_json::from_str(include_str!("fixtures/quote.json")).unwrap()
}
fn eth_market() -> Market {
    serde_json::from_str(include_str!("fixtures/market-ethereum-reusd.json")).unwrap()
}
fn eth_quote() -> Value {
    serde_json::from_str(include_str!("fixtures/quote-ethereum-reusd.json")).unwrap()
}
fn eth_limit_fill_quote() -> Value {
    serde_json::from_str(include_str!(
        "fixtures/quote-ethereum-reusd-limit-fill.json"
    ))
    .unwrap()
}
fn request() -> DraftInput {
    DraftInput {
        market: market().address,
        token: market().sy.address,
        amount: "1000000000000000000".into(),
        min_output: "1".into(),
        slippage_bps: 10,
        max_price_impact_bps: 100,
    }
}
/// Buys PT-reUSD with reUSD, the only token its SY accepts.
fn eth_request() -> DraftInput {
    DraftInput {
        market: eth_market().address,
        token: eth_market().underlying_asset.address,
        amount: "1000000000000000000".into(),
        min_output: "1".into(),
        slippage_bps: 10,
        max_price_impact_bps: 100,
    }
}
fn body() -> Vec<u8> {
    serde_json::to_vec(&request()).unwrap()
}
fn eth_body() -> Vec<u8> {
    serde_json::to_vec(&eth_request()).unwrap()
}
fn account() -> petal::WalletAccount {
    petal::testing::wallet_account("alice", 0)
}
fn tx() -> Transaction {
    serde_json::from_value(quote()["routes"][0]["tx"].clone()).unwrap()
}
fn eth_tx() -> Transaction {
    serde_json::from_value(eth_quote()["routes"][0]["tx"].clone()).unwrap()
}

struct Fake {
    store: BTreeMap<String, Vec<u8>>,
    now: u64,
    /// Served for market reads and used for on-chain token answers.
    market: Market,
    quote: Value,
    /// Answer to `eth_chainId`; `None` reports the chain that was asked.
    chain_id: Option<u64>,
    expired: bool,
    factory: bool,
    allowance: U256,
    http_calls: usize,
    http_response: Option<HttpResponse>,
    pages: VecDeque<Value>,
    portfolio: Value,
    requests: Vec<HttpRequest>,
    reads: Vec<(Chain, String)>,
    factory_calls: Vec<Address>,
    /// Onchain `decimals()` answers that differ from the market's token list.
    decimals: BTreeMap<Address, u8>,
    staged: Vec<EvmTransaction>,
    confirms: Vec<Chain>,
    inspects: Vec<Chain>,
    lose_stage: bool,
    approved: bool,
    /// Outbox ids that were broadcast; only these have receipts.
    mined: BTreeSet<String>,
    /// What Bloom reports for an unmined entry: its state and any hash.
    outbox: BTreeMap<String, (String, Option<String>)>,
    /// Receipts Bloom reports, by outbox id, in place of a mined success.
    receipts: BTreeMap<String, Value>,
    /// Files the Petal can read through the VFS, by path.
    files: BTreeMap<String, Vec<u8>>,
    /// The action id of the approval `confirm` asks for; staged entries carry
    /// `pending-action`, so any other id is a wallet-policy approval.
    approval_action: String,
    /// Fail the next confirm with a host error.
    fail_confirm: bool,
    /// Return an unmined entry with the same target, value and calldata
    /// instead of staging another, as Bloom does for one route.
    dedupe: bool,
    /// A VFS path whose read fails.
    unreadable: Option<String>,
    /// What a mined Pendle transaction takes from the router's allowance.
    spend: U256,
    /// Break the next outbox claim: `lost` writes it but reports an error,
    /// `fail` writes nothing.
    claim_fault: Option<&'static str>,
    /// An outbox id `stage` returns in place of its own.
    stage_id: Option<String>,
    /// Each `allowance` call: the token asked, then its owner and spender.
    allowance_calls: Vec<(Address, Address, Address)>,
}
impl Default for Fake {
    fn default() -> Self {
        Self {
            store: BTreeMap::new(),
            now: 1_000_000,
            market: market(),
            quote: quote(),
            chain_id: None,
            expired: false,
            factory: true,
            allowance: U256::ZERO,
            http_calls: 0,
            http_response: None,
            pages: VecDeque::new(),
            portfolio: json!({"positions": []}),
            requests: vec![],
            reads: vec![],
            factory_calls: vec![],
            decimals: BTreeMap::new(),
            staged: vec![],
            confirms: vec![],
            inspects: vec![],
            lose_stage: false,
            approved: false,
            mined: BTreeSet::new(),
            outbox: BTreeMap::new(),
            receipts: BTreeMap::new(),
            files: BTreeMap::new(),
            approval_action: "pending-action".into(),
            fail_confirm: false,
            dedupe: false,
            unreadable: None,
            spend: U256::from(10u64.pow(18)),
            claim_fault: None,
            stage_id: None,
            allowance_calls: vec![],
        }
    }
}
impl Fake {
    fn ethereum() -> Self {
        let mut fake = Self::default();
        fake.use_ethereum();
        fake
    }
    fn use_ethereum(&mut self) {
        self.market = eth_market();
        self.quote = eth_quote();
    }
    /// Bloom moves an entry from `pending/` to `failed/` and reports `state`
    /// for it.
    fn fail_entry(&mut self, chain: Chain, id: &str, state: &str) {
        self.outbox.insert(id.into(), (state.into(), None));
        let created = self.created(chain, id);
        self.files
            .remove(&outbox_path(chain, "pending", id, "intent.json"));
        self.files.insert(
            outbox_path(chain, "failed", id, "intent.json"),
            json!({"action_id": "pending-action", "status": state, "created_ms": created})
                .to_string()
                .into_bytes(),
        );
    }
    /// Bloom sends an entry: it leaves `pending/` and reports `state`.
    fn send_entry(&mut self, chain: Chain, id: &str, state: &str, hash: &str) {
        self.outbox
            .insert(id.into(), (state.into(), Some(hash.into())));
        let intent = self
            .files
            .remove(&outbox_path(chain, "pending", id, "intent.json"))
            .unwrap();
        let mut intent: Value = serde_json::from_slice(&intent).unwrap();
        intent["status"] = json!("sent");
        intent["tx_hash"] = json!(hash);
        self.files.insert(
            outbox_path(chain, "sent", id, "intent.json"),
            intent.to_string().into_bytes(),
        );
    }
    /// When Bloom created a pending entry.
    fn created(&self, chain: Chain, id: &str) -> u64 {
        let intent = &self.files[&outbox_path(chain, "pending", id, "intent.json")];
        serde_json::from_slice::<Value>(intent).unwrap()["created_ms"]
            .as_u64()
            .unwrap()
    }
}
/// `account()`'s outbox file, as the Petal reads it.
fn outbox_path(chain: Chain, dir: &str, id: &str, file: &str) -> String {
    format!(
        "wallets/alice/0/chains/{}/outbox/{dir}/{id}/{file}",
        chain.name()
    )
}
impl Host for Fake {
    fn now_ms(&mut self) -> u64 {
        self.now
    }
    fn http(&mut self, req: HttpRequest, _max: usize) -> Result<HttpResponse> {
        self.http_calls += 1;
        self.requests.push(req.clone());
        if let Some(response) = self.http_response.take() {
            return Ok(response);
        }
        let payload = if req.method == "POST" {
            self.quote.clone()
        } else if req.url.contains("/v2/markets/all") {
            self.pages.pop_front().expect("unexpected market page")
        } else if req.url.contains("/dashboard/positions/") {
            self.portfolio.clone()
        } else {
            serde_json::to_value(&self.market).unwrap()
        };
        Ok(HttpResponse {
            status: if req.method == "POST" { 201 } else { 200 },
            headers: vec![],
            body: serde_json::to_vec(&payload).unwrap(),
        })
    }
    fn vfs(&mut self, _path: &str) -> Result<Vec<u8>> {
        Ok(OWNER.as_bytes().to_vec())
    }
    fn vfs_file(&mut self, path: &str, max: usize) -> Result<Option<Vec<u8>>> {
        if self.unreadable.as_deref() == Some(path) {
            return Err("outbox entry read failed".into());
        }
        Ok(self.files.get(path).map(|bytes| {
            if bytes.len() > max {
                vec![]
            } else {
                bytes.clone()
            }
        }))
    }
    fn chain(&mut self, chain: Chain, method: &str, params: &str) -> Result<String> {
        self.reads.push((chain, method.into()));
        if method == "eth_chainId" {
            assert_eq!(params, "[]");
            let id = self.chain_id.unwrap_or(chain.id());
            return Ok(json!(format!("0x{id:x}")).to_string());
        }
        assert_eq!(method, "eth_call");
        let p: Value = serde_json::from_str(params).unwrap();
        let data = p[0]["data"].as_str().unwrap();
        let bytes = hex::decode(&data[2..]).unwrap();
        let m = &self.market;
        let response = if bytes.starts_with(&isValidMarketCall::SELECTOR) {
            self.factory_calls
                .push(address(p[0]["to"].as_str().unwrap())?);
            self.factory.abi_encode()
        } else if bytes.starts_with(&readTokensCall::SELECTOR) {
            (
                address(&m.sy.address)?,
                address(&m.pt.address)?,
                address(&m.yt.address)?,
            )
                .abi_encode()
        } else if bytes.starts_with(&expiryCall::SELECTOR) {
            U256::from(if self.expired { 0 } else { 2_000_000 }).abi_encode()
        } else if bytes.starts_with(&allowanceCall::SELECTOR) {
            let call = allowanceCall::abi_decode(&bytes).unwrap();
            self.allowance_calls.push((
                address(p[0]["to"].as_str().unwrap())?,
                call.owner,
                call.spender,
            ));
            self.allowance.abi_encode()
        } else if bytes.starts_with(&getTokensInCall::SELECTOR) {
            // The SY's own deposit tokens, as the market fixture lists them.
            m.input_tokens
                .iter()
                .map(|t| address(&t.address))
                .collect::<Result<Vec<_>>>()?
                .abi_encode()
        } else if bytes.starts_with(&getTokensOutCall::SELECTOR) {
            m.output_tokens
                .iter()
                .map(|t| address(&t.address))
                .collect::<Result<Vec<_>>>()?
                .abi_encode()
        } else if bytes.starts_with(&decimalsCall::SELECTOR) {
            let to = address(p[0]["to"].as_str().unwrap())?;
            let listed = [
                &m.pt,
                &m.yt,
                &m.sy,
                &m.underlying_asset,
                &m.accounting_asset,
            ]
            .into_iter()
            .chain(&m.input_tokens)
            .find(|t| address(&t.address).ok() == Some(to))
            .map(|t| t.decimals);
            // A uint8 return is one ABI word, the same as a uint256.
            U256::from(
                self.decimals
                    .get(&to)
                    .copied()
                    .or(listed)
                    .expect("decimals of an unknown token"),
            )
            .abi_encode()
        } else {
            panic!("unexpected call {data}")
        };
        Ok(json!(format!("0x{}", hex::encode(response))).to_string())
    }
    fn get(&mut self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.store.get(key).cloned())
    }
    fn put(&mut self, key: &str, value: &[u8]) -> Result<()> {
        self.store.insert(key.into(), value.into());
        Ok(())
    }
    fn put_new(&mut self, key: &str, value: &[u8]) -> Result<()> {
        if key.starts_with("outbox-claims/") {
            match self.claim_fault.take() {
                Some("lost") => {
                    self.store.insert(key.into(), value.into());
                    return Err("lost reply".into());
                }
                Some(_) => return Err("store unavailable".into()),
                None => {}
            }
        }
        if self.store.contains_key(key) {
            return Err("already exists".into());
        }
        self.put(key, value)
    }
    fn delete_if(&mut self, key: &str, value: &[u8]) -> Result<()> {
        if self.store.get(key).map(Vec::as_slice) != Some(value) {
            return Err("lock differs".into());
        }
        self.store.remove(key);
        Ok(())
    }
    fn stage(&mut self, tx: &EvmTransaction) -> Result<StagedTransaction> {
        if self.dedupe {
            let pending = self.staged.iter().enumerate().find(|(i, old)| {
                let id = format!("{}-tx", i + 1);
                let chain = if old.chain == "ethereum" { ETH } else { ARB };
                !self.mined.contains(&id)
                    && self
                        .files
                        .contains_key(&outbox_path(chain, "pending", &id, "intent.json"))
                    && (&old.to, &old.value_wei, &old.data_hex)
                        == (&tx.to, &tx.value_wei, &tx.data_hex)
            });
            if let Some((i, _)) = pending {
                return Ok(StagedTransaction {
                    outbox_id: format!("{}-tx", i + 1),
                    plan_md: "existing entry".into(),
                    approval: None,
                });
            }
        }
        self.staged.push(tx.clone());
        if self.lose_stage {
            return Err("lost reply".into());
        }
        let id = self
            .stage_id
            .take()
            .unwrap_or_else(|| format!("{}-tx", self.staged.len()));
        let chain = if tx.chain == "ethereum" { ETH } else { ARB };
        self.files.insert(
            outbox_path(chain, "pending", &id, "intent.json"),
            json!({
                "action_id": "pending-action", "status": "pending", "created_ms": self.now,
                "to": tx.to, "value_wei": tx.value_wei, "data_hex": tx.data_hex,
            })
            .to_string()
            .into_bytes(),
        );
        Ok(StagedTransaction {
            outbox_id: id,
            plan_md: "owner review".into(),
            approval: None,
        })
    }
    fn confirm(&mut self, _wallet: &str, chain: Chain, id: &str) -> Result<StagedTransaction> {
        self.confirms.push(chain);
        if std::mem::take(&mut self.fail_confirm) {
            return Err("Bloom has not confirmed the transaction".into());
        }
        if !self.approved {
            return Ok(StagedTransaction {
                outbox_id: id.into(),
                plan_md: "owner approval pending".into(),
                approval: Some(petal::sdk::OutboxApproval {
                    action_id: self.approval_action.clone(),
                    expires_ms: self.now + 60_000,
                }),
            });
        }
        self.mined.insert(id.into());
        // The mined transaction moves the router's allowance like the token.
        let n: usize = id.trim_end_matches("-tx").parse().unwrap_or(0);
        if let Some(tx) = n.checked_sub(1).and_then(|i| self.staged.get(i)) {
            let data = hex::decode(&tx.data_hex[2..]).unwrap();
            if let Ok(call) = approveCall::abi_decode(&data) {
                self.allowance = call.amount;
            } else {
                self.allowance = self.allowance.saturating_sub(self.spend);
            }
        }
        Ok(StagedTransaction {
            outbox_id: id.into(),
            plan_md: "reviewed".into(),
            approval: None,
        })
    }
    fn inspect(&mut self, _wallet: &str, chain: Chain, id: &str) -> Result<OutboxInspection> {
        self.inspects.push(chain);
        if let Some(receipt) = self.receipts.get(id) {
            return Ok(OutboxInspection {
                outbox_id: id.into(),
                state: receipt["outcome"].as_str().unwrap().into(),
                tx_hash: Some("0xabc".into()),
                receipt_json: Some(receipt.to_string()),
            });
        }
        let mined = self.mined.contains(id);
        if let (false, Some((state, tx_hash))) = (mined, self.outbox.get(id)) {
            return Ok(OutboxInspection {
                outbox_id: id.into(),
                state: state.clone(),
                tx_hash: tx_hash.clone(),
                receipt_json: None,
            });
        }
        Ok(OutboxInspection {
            outbox_id: id.into(),
            state: if mined { "success" } else { "staged" }.into(),
            tx_hash: mined.then(|| "0xabc".into()),
            receipt_json: mined.then(|| json!({"outcome":"success","tx_hash":"0xabc"}).to_string()),
        })
    }
}

#[test]
fn input_rejects_unknown_fields_and_noncanonical_amounts() {
    for value in ["0", "01", "-1", "1.0", "1e18", &"9".repeat(79)] {
        let mut r = request();
        r.amount = value.into();
        assert!(DraftInput::parse(&serde_json::to_vec(&r).unwrap()).is_err());
    }
    let mut r = serde_json::to_value(request()).unwrap();
    r["receiver"] = json!(OWNER);
    assert!(DraftInput::parse(&serde_json::to_vec(&r).unwrap()).is_err());
    assert!(DraftInput::parse(&body()).is_ok());
}
#[test]
fn bounds_and_ids_are_strict() {
    assert_eq!(Chain::parse("arbitrum"), Ok(ARB));
    assert_eq!(Chain::parse("ethereum"), Ok(ETH));
    for name in ["base", "Ethereum", "mainnet", "1", ""] {
        assert!(Chain::parse(name).is_err(), "{name}");
    }
    assert_eq!((ETH.id(), ARB.id()), (1, 42161));
    for id in ["", "..", "a/b", "a?x", "a.b"] {
        assert!(operation_id(id).is_err());
    }
    assert!(operation_id("draft-1_ok").is_ok());
    let mut r = request();
    r.slippage_bps = 0;
    assert!(DraftInput::parse(&serde_json::to_vec(&r).unwrap()).is_err());
}
#[test]
fn real_unsigned_quote_validates() {
    assert!(validate_transaction(&tx(), OWNER, BUY, &request(), &market()).is_ok());
}
#[test]
fn real_ethereum_quote_validates() {
    let t = eth_tx();
    assert!(
        hex::decode(&t.data[2..])
            .unwrap()
            .starts_with(&swapExactTokenForPtCall::SELECTOR)
    );
    let minimum = validate_transaction(&t, OWNER, BUY, &eth_request(), &eth_market()).unwrap();
    assert!(minimum > U256::ZERO);
}
#[test]
fn reject_changed_receiver_market_input_and_floor() {
    let original = tx();
    let bytes = hex::decode(&original.data[2..]).unwrap();
    for field in ["receiver", "market", "amount", "minimum"] {
        let mut c = swapExactSyForPtCall::abi_decode(&bytes).unwrap();
        match field {
            "receiver" => c.receiver = Address::repeat_byte(7),
            "market" => c.market = Address::repeat_byte(8),
            "amount" => c.exactSyIn += U256::from(1),
            "minimum" => c.minPtOut = U256::ZERO,
            _ => unreachable!(),
        }
        let mut t = original.clone();
        t.data = format!("0x{}", hex::encode(c.abi_encode()));
        assert!(
            validate_transaction(&t, OWNER, BUY, &request(), &market()).is_err(),
            "{field}"
        );
    }
}
#[test]
fn reject_target_sender_native_value_and_trailing_bytes() {
    for field in ["to", "from", "value", "data"] {
        let mut t = tx();
        match field {
            "to" => t.to = format!("{:#x}", Address::repeat_byte(7)),
            "from" => t.from = format!("{:#x}", Address::repeat_byte(7)),
            "value" => t.value = "1".into(),
            "data" => t.data.push_str("00"),
            _ => unreachable!(),
        }
        assert!(validate_transaction(&t, OWNER, BUY, &request(), &market()).is_err());
    }
}
#[test]
fn reject_embedded_limit_order_and_wrong_operation() {
    let mut t = tx();
    let mut c = swapExactSyForPtCall::abi_decode(&hex::decode(&t.data[2..]).unwrap()).unwrap();
    c.limit.optData = vec![1].into();
    t.data = format!("0x{}", hex::encode(c.abi_encode()));
    assert!(validate_transaction(&t, OWNER, BUY, &request(), &market()).is_err());
    assert!(validate_transaction(&tx(), OWNER, Operation::SellPt, &request(), &market()).is_err());
}
#[test]
fn draft_is_idempotent_and_never_stages() {
    let mut h = Fake::default();
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    let count = h.http_calls;
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    assert_eq!(h.http_calls, count);
    assert!(h.staged.is_empty());
    let d = workflow::load(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!(d.steps.len(), 2);
    assert_eq!((d.chain, d.schema.as_str()), (ARB, workflow::SCHEMA));
    let mut changed = request();
    changed.amount = "2".into();
    assert!(
        workflow::create(
            &mut h,
            ARB,
            &account(),
            BUY,
            "one",
            &serde_json::to_vec(&changed).unwrap()
        )
        .is_err()
    );
}
#[test]
fn allowance_reset_is_explicit_and_bounded() {
    let mut h = Fake {
        allowance: U256::from(1),
        ..Fake::default()
    };
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    let d = workflow::load(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!(d.steps.len(), 3);
    for (step, expected) in d.steps[..2]
        .iter()
        .zip([U256::ZERO, amount(&request().amount).unwrap()])
    {
        let a =
            approveCall::abi_decode(&hex::decode(&step.transaction.data[2..]).unwrap()).unwrap();
        assert_eq!(a.amount, expected);
        assert_eq!(a.spender, address(ROUTER).unwrap());
    }
}
#[test]
fn sufficient_allowance_never_adds_approval() {
    let mut h = Fake {
        allowance: U256::MAX,
        ..Fake::default()
    };
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    assert_eq!(
        workflow::load(&mut h, ARB, &account(), BUY, "one")
            .unwrap()
            .steps
            .len(),
        1
    );
}
#[test]
fn lost_stage_reply_never_creates_a_second_outbox() {
    let mut h = Fake::default();
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    h.lose_stage = true;
    assert!(workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").is_err());
    assert!(workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").is_err());
    assert_eq!(h.staged.len(), 1);
    assert_eq!(
        workflow::load(&mut h, ARB, &account(), BUY, "one")
            .unwrap()
            .state,
        "staging_unknown"
    );
}
#[test]
fn approval_retries_reuse_the_same_outbox_id() {
    let mut h = Fake::default();
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap();
    workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap();
    assert_eq!(
        workflow::load(&mut h, ARB, &account(), BUY, "one")
            .unwrap()
            .state,
        "approval_required"
    );
    assert_eq!(h.staged.len(), 1);
    h.approved = true;
    workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap();
    assert_eq!(h.staged.len(), 1);
}
#[test]
fn expired_quote_and_wrong_confirmation_do_not_stage() {
    let mut h = Fake::default();
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    assert!(workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"yes").is_err());
    h.now += 300_001;
    assert!(workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").is_err());
    assert!(h.staged.is_empty());
}
#[test]
fn an_approval_requested_before_expiry_can_finish_after_it() {
    let mut h = Fake::default();
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    // Stage the token approval and request the owner's approval.
    workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap();
    workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap();
    // The owner finishes the ceremony after the quote has expired.
    h.now += 300_001;
    h.approved = true;
    workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap();
    assert_eq!(h.mined, BTreeSet::from(["1-tx".into()]));
    // The swap is not staged from the expired quote.
    assert_eq!(
        workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap_err(),
        "quote expired; reconcile prior steps and create a new reviewed intent"
    );
    assert_eq!(h.staged.len(), 1);
}
#[test]
fn an_expired_quote_requests_no_new_approval() {
    let mut h = Fake::default();
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    // Staged, but the owner's approval has not been requested yet.
    workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap();
    h.now += 300_001;
    assert!(workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").is_err());
    assert!(h.confirms.is_empty());
}
#[test]
fn forged_account_cannot_load_another_accounts_record() {
    let mut h = Fake::default();
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    assert!(
        workflow::load(
            &mut h,
            ARB,
            &petal::testing::wallet_account("alice", 1),
            BUY,
            "one"
        )
        .is_err()
    );
    assert!(
        workflow::load(
            &mut h,
            ARB,
            &petal::testing::wallet_account("bob", 0),
            BUY,
            "one"
        )
        .is_err()
    );
}
#[test]
fn status_reads_do_not_mutate_state_or_send_transactions() {
    let mut h = Fake::default();
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    let original = h.store.clone();
    workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!(h.store, original);
    assert!(h.staged.is_empty());
}
#[test]
fn concurrent_or_crashed_writer_lock_refuses_execution() {
    let mut h = Fake::default();
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    h.store.insert(
        workflow::lock_key(ARB, BUY, "one").unwrap(),
        b"crashed writer".to_vec(),
    );
    assert!(workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").is_err());
    assert!(h.staged.is_empty());
}
#[test]
fn factory_and_maturity_checks_run_before_quotes() {
    for (factory, expired) in [(false, false), (true, true)] {
        let mut h = Fake {
            factory,
            expired,
            ..Fake::default()
        };
        assert!(workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).is_err());
        assert_eq!(h.http_calls, 1);
    }
}
#[test]
fn refuse_changed_quote_outputs_and_aggregators() {
    for field in ["token", "aggregator", "impact"] {
        let mut h = Fake::default();
        match field {
            "token" => h.quote["routes"][0]["outputs"][0]["token"] = json!(OWNER),
            "aggregator" => h.quote["routes"][0]["data"]["aggregatorType"] = json!("KYBERSWAP"),
            "impact" => h.quote["routes"][0]["data"]["priceImpact"] = json!(0.5),
            _ => unreachable!(),
        }
        assert!(workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).is_err());
        assert!(h.store.is_empty());
    }
}
#[test]
fn quotes_allowing_more_slippage_than_asked_are_refused() {
    // The Ethereum fixture was quoted at 10 bps: expected 1123968, and the
    // encoded minimum 1122844 is floor(1123968 × 9990 / 10000).
    let with_minimum = |minimum: U256| {
        let mut c = swapExactTokenForPtCall::abi_decode(&hex::decode(&eth_tx().data[2..]).unwrap())
            .unwrap();
        c.minPtOut = minimum;
        let mut q = eth_quote();
        q["routes"][0]["tx"]["data"] = json!(format!("0x{}", hex::encode(c.abi_encode())));
        q
    };
    let floor = U256::from(1_122_844);
    for (minimum, slippage_bps, accepted) in [
        (floor, 10, true),
        (floor - U256::from(1), 10, false),
        (U256::from(1), 10, false),
        (floor, 9, false),
    ] {
        let mut h = Fake {
            quote: with_minimum(minimum),
            ..Fake::ethereum()
        };
        let mut r = eth_request();
        r.slippage_bps = slippage_bps;
        let result = workflow::create(
            &mut h,
            ETH,
            &account(),
            BUY,
            "one",
            &serde_json::to_vec(&r).unwrap(),
        );
        if accepted {
            result.unwrap();
        } else {
            assert_eq!(
                result.unwrap_err(),
                "quote minimum allows more slippage than slippage_bps",
                "{minimum} at {slippage_bps} bps"
            );
            assert!(h.store.is_empty());
        }
    }
}
#[test]
fn native_token_drafts_are_refused() {
    // Listings show ETH as the zero address. Drafts refuse it before any read.
    let mut h = Fake::ethereum();
    let mut r = eth_request();
    r.token = format!("{:#x}", Address::ZERO);
    let draft = |h: &mut Fake, r: &DraftInput| {
        workflow::create(
            h,
            ETH,
            &account(),
            BUY,
            "one",
            &serde_json::to_vec(r).unwrap(),
        )
    };
    assert_eq!(
        draft(&mut h, &r).unwrap_err(),
        "native-token and zero addresses are not supported"
    );
    assert_eq!(h.http_calls, 0);
    // The 0xEeee… placeholder for ETH is not one of the SY's tokens.
    r.token = "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".into();
    assert_eq!(
        draft(&mut h, &r).unwrap_err(),
        "asset is not natively supported by this market's SY; aggregator routes are disabled"
    );
    assert_eq!(h.http_calls, 1, "no quote is requested");
    assert!(h.store.is_empty());
}
#[test]
fn sdk42_account_context_has_no_default() {
    struct Route;
    impl petal::RouteIdentity for Route {
        const PATH: &'static str = "arbitrum/buy-pt/[wallet]/[index]/[id]/draft.json";
        const CANONICAL_PATH: &'static str = Self::PATH;
        const PARAMS: &'static [(&'static str, usize)] = &[("wallet", 2), ("index", 3), ("id", 4)];
    }
    let missing =
        petal::testing::RouteCtx::new("arbitrum/buy-pt/alice/1/one/draft.json").bind::<Route>();
    assert!(petal::wallet_account(&missing).is_err());
    let selected = petal::testing::RouteCtx::new("arbitrum/buy-pt/alice/1/one/draft.json")
        .account("alice", 1)
        .bind::<Route>();
    assert_eq!(petal::wallet_account(&selected).unwrap().number(), 1);
}

#[test]
fn pinned_factories_are_pendles_deployments() {
    // `marketFactory` and `marketFactoryV3` to `V6`, and `router`, from
    // pendle-core-v2-public `deployments/1-core.json` and `42161-core.json`,
    // read on 2026-10-30.
    assert_eq!(
        factories(ETH),
        [
            "0x27b1dacd74688af24a64bd3c9c1b143118740784",
            "0x1a6fcc85557bc4fb7b534ed835a03ef056552d52",
            "0x3d75bd20c983edb5fd218a1b7e0024f1056c7a2f",
            "0x6fcf753f2c67b83f7b09746bbc4fa0047b35d050",
            "0x6d247b1c044fa1e22e6b04fa9f71baf99eb29a9f",
        ]
    );
    assert_eq!(
        factories(ARB),
        [
            "0xf5a7de2d276dbda3eef1b62a9e718eff4d29ddc8",
            "0x2fcb47b58350cd377f94d3821e7373df60bd9ced",
            "0xd9f5e9589016da862d2abce980a5a5b99a94f3e8",
            "0xd29e76c6f15ada0150d10a1d3f45accd2098283b",
            "0x49f2f7002669e0e4425fa0203975625ab4af3143",
        ]
    );
    assert_eq!(ROUTER, "0x888888888889758f76e7103c6cbf23abbf58f946");
    // Visit all factories, rather than stopping after the first successful
    // fake response.
    for (chain, m) in [(ARB, market()), (ETH, eth_market())] {
        let mut h = Fake {
            factory: false,
            market: m.clone(),
            ..Fake::default()
        };
        let error =
            verify_market(&mut h, chain, &m, BUY, address(&m.sy.address).unwrap()).unwrap_err();
        assert_eq!(
            error,
            "market is not authenticated by a supported Pendle factory"
        );
        let pinned: Vec<Address> = factories(chain)
            .iter()
            .map(|f| address(f).unwrap())
            .collect();
        assert_eq!(h.factory_calls, pinned, "{chain:?}");
        assert!(h.reads.iter().all(|(read, _)| *read == chain));
    }
}

#[test]
fn ethereum_draft_uses_chain_one_end_to_end() {
    let mut h = Fake::ethereum();
    workflow::create(&mut h, ETH, &account(), BUY, "one", &eth_body()).unwrap();
    let urls: Vec<_> = h.requests.iter().map(|r| r.url.as_str()).collect();
    assert_eq!(
        urls,
        [
            format!("{API}/v1/1/markets/0x13285bcbc27f92b47b4edb99d744c07b48c977c0"),
            format!("{API}/v3/sdk/1/convert"),
        ]
    );
    let quote_body: Value = serde_json::from_slice(&h.requests[1].body).unwrap();
    assert_eq!(quote_body["useLimitOrder"], json!(false));
    assert_eq!(quote_body["enableAggregator"], json!(false));

    let d = workflow::load(&mut h, ETH, &account(), BUY, "one").unwrap();
    assert_eq!(d.chain, ETH);
    let kinds: Vec<_> = d.steps.iter().map(|s| s.kind.as_str()).collect();
    assert_eq!(kinds, ["approve_input", "pendle_execution"]);
    // The approval goes to reUSD, the token the Router will pull.
    assert_eq!(d.steps[0].transaction.to, eth_request().token);
    let plan = workflow::plan(&d);
    assert!(plan.contains("Chain: ethereum (1)"), "{plan}");
    assert!(plan.contains("(reUSD, 18 decimals)"), "{plan}");
    assert!(plan.contains("(PT-reUSD-10DEC2026, 6 decimals)"), "{plan}");

    // Stage the approval, request the owner's approval, then broadcast it.
    workflow::confirm(&mut h, ETH, &account(), BUY, "one", b"confirm").unwrap();
    workflow::confirm(&mut h, ETH, &account(), BUY, "one", b"confirm").unwrap();
    h.approved = true;
    workflow::confirm(&mut h, ETH, &account(), BUY, "one", b"confirm").unwrap();
    // With the approval mined, stage the swap and broadcast it.
    workflow::confirm(&mut h, ETH, &account(), BUY, "one", b"confirm").unwrap();
    workflow::confirm(&mut h, ETH, &account(), BUY, "one", b"confirm").unwrap();
    let d = workflow::reconcile(&mut h, ETH, &account(), BUY, "one").unwrap();
    assert_eq!(d.state, "execution_confirmed");
    assert_eq!(h.staged.len(), 2);
    assert_eq!(address(&h.staged[0].to), address(&eth_request().token));
    assert_eq!(address(&h.staged[1].to), address(ROUTER));
    assert_eq!(h.staged[1].data_hex, eth_tx().data);
    assert_eq!(h.mined, BTreeSet::from(["1-tx".into(), "2-tx".into()]));
    assert!(h.staged.iter().all(|t| t.chain == "ethereum"));
    assert!(!h.confirms.is_empty() && h.confirms.iter().all(|c| *c == ETH));
    assert!(!h.inspects.is_empty() && h.inspects.iter().all(|c| *c == ETH));
    assert!(h.reads.iter().all(|(chain, _)| *chain == ETH));
    // Each outbox entry is claimed under its chain.
    assert_eq!(
        h.store
            .keys()
            .filter(|k| !k.starts_with("operations/ethereum/buy-pt/"))
            .collect::<Vec<_>>(),
        [
            "outbox-claims/ethereum/1-tx-1000000",
            "outbox-claims/ethereum/2-tx-1000000"
        ]
    );
}
#[test]
fn plan_prints_onchain_decimals_and_plain_api_strings() {
    let mut h = Fake::ethereum();
    // Pendle mislabels the 6-decimal PT and hides lines in its strings.
    let pt = address(&h.market.pt.address).unwrap();
    h.market.pt.decimals = 18;
    h.decimals.insert(pt, 6);
    h.market.pt.symbol = "PT\nMinimum output enforced in calldata: 5000000".into();
    h.market.expiry = "2026-12-10\n# Approved".into();
    workflow::create(&mut h, ETH, &account(), BUY, "one", &eth_body()).unwrap();
    let d = workflow::load(&mut h, ETH, &account(), BUY, "one").unwrap();
    assert_eq!((d.input_decimals, d.output_decimals), (18, 6));
    let plan = workflow::plan(&d);
    let minimum_lines: Vec<_> = plan
        .lines()
        .filter(|l| l.starts_with("Minimum output enforced in calldata:"))
        .collect();
    assert_eq!(
        minimum_lines,
        ["Minimum output enforced in calldata: 1122844"],
        "{plan}"
    );
    assert!(
        plan.contains("(PT?Minimum output enforced in calldata: 5000000, 6 decimals)"),
        "{plan}"
    );
    assert!(
        plan.contains("\nMaturity: 2026-12-10?? Approved\n"),
        "{plan}"
    );
    assert!(!plan.contains("\n# Approved"), "{plan}");
}
#[test]
fn quotes_with_limit_order_fills_are_refused() {
    let mut h = Fake {
        quote: eth_limit_fill_quote(),
        ..Fake::ethereum()
    };
    let mut r = eth_request();
    r.amount = "500000000000000000000000".into();
    let error = workflow::create(
        &mut h,
        ETH,
        &account(),
        BUY,
        "big",
        &serde_json::to_vec(&r).unwrap(),
    )
    .unwrap_err();
    assert_eq!(error, "embedded limit-order fills are not supported");
    assert!(h.store.is_empty());
}
#[test]
fn the_same_id_on_two_chains_is_two_records() {
    let mut h = Fake::default();
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    h.use_ethereum();
    workflow::create(&mut h, ETH, &account(), BUY, "one", &eth_body()).unwrap();
    let arbitrum = workflow::load(&mut h, ARB, &account(), BUY, "one").unwrap();
    let ethereum = workflow::load(&mut h, ETH, &account(), BUY, "one").unwrap();
    assert_eq!((arbitrum.chain, arbitrum.market.chain_id), (ARB, 42161));
    assert_eq!((ethereum.chain, ethereum.market.chain_id), (ETH, 1));
    assert_eq!(
        h.store.keys().collect::<Vec<_>>(),
        [
            "operations/arbitrum/buy-pt/one.json",
            "operations/ethereum/buy-pt/one.json"
        ]
    );
}
#[test]
fn a_draft_is_invisible_from_another_chain() {
    let mut h = Fake::default();
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    assert_eq!(
        workflow::load(&mut h, ETH, &account(), BUY, "one").unwrap_err(),
        "operation not found"
    );
    assert!(workflow::confirm(&mut h, ETH, &account(), BUY, "one", b"confirm").is_err());
    assert!(h.staged.is_empty());
}
#[test]
fn tampered_records_are_refused() {
    let mut h = Fake::default();
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    let arb_key = "operations/arbitrum/buy-pt/one.json";
    let eth_key = "operations/ethereum/buy-pt/one.json";
    let original: Value = serde_json::from_slice(&h.store[arb_key]).unwrap();
    let cases: [(&str, &str, Value); 3] = [
        // A record claiming another chain under this chain's key.
        (arb_key, "chain", json!("ethereum")),
        // An Arbitrum market moved under the Ethereum key.
        (eth_key, "chain", json!("ethereum")),
        // A record whose schema is not v2. Real v1 records used keys without
        // the chain, which this build never reads.
        (arb_key, "schema", json!("bloom.pendle.operation.v1")),
    ];
    for (key, field, value) in cases {
        let mut h = Fake::default();
        let mut record = original.clone();
        record[field] = value;
        h.store
            .insert(key.into(), serde_json::to_vec(&record).unwrap());
        let chain = if key == eth_key { ETH } else { ARB };
        assert_eq!(
            workflow::load(&mut h, chain, &account(), BUY, "one").unwrap_err(),
            "persisted operation does not belong to the selected chain",
            "{key} {field}"
        );
        assert!(workflow::confirm(&mut h, chain, &account(), BUY, "one", b"confirm").is_err());
        assert!(h.staged.is_empty());
    }
}
#[test]
fn verify_market_refuses_a_market_from_another_chain() {
    let mut h = Fake::default();
    let error = verify_market(
        &mut h,
        ETH,
        &market(),
        BUY,
        address(&market().sy.address).unwrap(),
    )
    .unwrap_err();
    assert_eq!(error, "market belongs to chain 42161, not ethereum");
    assert!(h.reads.is_empty());
}
#[test]
fn arbitrum_market_data_from_the_ethereum_url_is_refused() {
    let mut h = Fake::ethereum();
    h.market = market();
    let mut r = eth_request();
    r.market = market().address;
    r.token = market().sy.address;
    let error = workflow::create(
        &mut h,
        ETH,
        &account(),
        BUY,
        "one",
        &serde_json::to_vec(&r).unwrap(),
    )
    .unwrap_err();
    assert_eq!(error, "Pendle market identity mismatch");
    assert_eq!(h.http_calls, 1);
    assert!(h.store.is_empty());
}
#[test]
fn an_rpc_on_the_wrong_chain_is_refused() {
    let mut h = Fake {
        chain_id: Some(42161),
        ..Fake::ethereum()
    };
    let error = workflow::create(&mut h, ETH, &account(), BUY, "one", &eth_body()).unwrap_err();
    assert_eq!(error, "Bloom's ethereum RPC does not report chain id 1");
    assert_eq!(h.http_calls, 1, "no quote is requested");
    assert!(h.factory_calls.is_empty());

    // A chain repointed after the draft was made is caught at confirm.
    let mut h = Fake::ethereum();
    workflow::create(&mut h, ETH, &account(), BUY, "one", &eth_body()).unwrap();
    h.chain_id = Some(42161);
    assert!(workflow::confirm(&mut h, ETH, &account(), BUY, "one", b"confirm").is_err());
    assert!(h.staged.is_empty());
}

fn listed(n: u8, chain_id: u64) -> Value {
    let token = |b: u8| format!("{chain_id}-{:#x}", Address::repeat_byte(b));
    json!({
        "chainId": chain_id,
        "address": format!("{:#x}", Address::repeat_byte(n)),
        "name": format!("market-{n}"),
        "expiry": "2027-01-01T00:00:00.000Z",
        "pt": token(n + 100), "yt": token(n + 101), "sy": token(n + 102),
        "underlyingAsset": token(n + 103), "accountingAsset": token(n + 104),
        "inputTokens": [token(n + 103)], "outputTokens": [token(n + 103)],
        "details": {"liquidity": 1000.5, "impliedApy": 0.1, "totalTvl": 9.0},
        "icon": "https://example.invalid/icon.svg"
    })
}
fn page(total: usize, results: Vec<Value>) -> Value {
    json!({"total": total, "limit": api::MARKET_PAGE, "skip": 0, "results": results})
}
fn with_pages(pages: impl IntoIterator<Item = Value>) -> Fake {
    Fake {
        pages: pages.into_iter().collect(),
        ..Fake::default()
    }
}
#[test]
fn market_list_pages_merge_dedupe_and_stop_at_total() {
    let mut h = with_pages([
        page(3, vec![listed(1, 1), listed(2, 1)]),
        page(3, vec![listed(2, 1), listed(3, 1)]),
    ]);
    let list = api::active_markets(&mut h, ETH).unwrap();
    assert_eq!((list.chain, list.chain_id), (ETH, 1));
    assert_eq!((list.total, list.count, list.truncated), (3, 3, false));
    let urls: Vec<_> = h.requests.iter().map(|r| r.url.clone()).collect();
    assert_eq!(
        urls,
        [
            format!("{API}/v2/markets/all?chainId=1&isActive=true&skip=0&limit=40"),
            format!("{API}/v2/markets/all?chainId=1&isActive=true&skip=2&limit=40"),
        ]
    );
    let first = &list.results[0];
    assert_eq!(first.address, format!("{:#x}", Address::repeat_byte(1)));
    assert_eq!(first.pt, format!("{:#x}", Address::repeat_byte(101)));
    assert_eq!(
        first.input_tokens,
        [format!("{:#x}", Address::repeat_byte(104))]
    );
    assert_eq!(first.liquidity_usd, Some(1000.5));
}
#[test]
fn market_list_reports_truncation() {
    let mut h = with_pages([page(5, vec![listed(1, 42161)]), page(5, vec![])]);
    let list = api::active_markets(&mut h, ARB).unwrap();
    assert_eq!((list.total, list.count, list.truncated), (5, 1, true));

    let mut h = with_pages((1..=20).map(|n| page(1000, vec![listed(n, 1)])));
    let list = api::active_markets(&mut h, ETH).unwrap();
    assert_eq!(h.http_calls, api::MAX_MARKET_PAGES);
    assert_eq!((list.count, list.truncated), (api::MAX_MARKET_PAGES, true));

    // Pages that repeat earlier rows add nothing, so they cannot complete the
    // list, and paging stops at the first one.
    for total in [3, 100] {
        let repeated = || page(total, vec![listed(1, 1), listed(2, 1)]);
        let mut h = with_pages([repeated(), repeated(), repeated()]);
        let list = api::active_markets(&mut h, ETH).unwrap();
        assert_eq!((list.count, list.truncated), (2, true), "{total}");
        assert_eq!(h.http_calls, 2, "{total}");
    }
}
#[test]
fn market_list_keeps_native_token_entries() {
    // wstETH's SY accepts ETH, which Pendle lists as the zero address.
    let mut m = listed(1, 1);
    m["inputTokens"] = json!([
        format!("1-{:#x}", Address::ZERO),
        "1-0x7f39c581f595b53c5cb19bd0b3f8da6c935e2ca0"
    ]);
    let mut h = with_pages([page(1, vec![m])]);
    let list = api::active_markets(&mut h, ETH).unwrap();
    assert_eq!(
        list.results[0].input_tokens,
        [
            format!("{:#x}", Address::ZERO),
            "0x7f39c581f595b53c5cb19bd0b3f8da6c935e2ca0".to_owned()
        ]
    );
}
#[test]
fn market_list_refuses_other_chains() {
    for bad in [listed(1, 42161), {
        let mut m = listed(1, 1);
        m["pt"] = json!(format!("42161-{:#x}", Address::repeat_byte(9)));
        m
    }] {
        let mut h = with_pages([page(1, vec![bad])]);
        assert!(api::active_markets(&mut h, ETH).is_err());
    }
}
#[test]
fn portfolio_keeps_only_the_selected_chain() {
    let mut h = Fake {
        portfolio: json!({"positions": [
            {"chainId": 1, "openPositions": [{"marketId": "1-0xabc"}]},
            {"chainId": 42161, "openPositions": []},
            {"chainId": 143, "openPositions": []}
        ]}),
        ..Fake::default()
    };
    let owner = "0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf";
    let value = api::portfolio(&mut h, ETH, owner).unwrap();
    assert_eq!(
        value["positions"],
        json!([{"chainId": 1, "openPositions": [{"marketId": "1-0xabc"}]}])
    );
    assert_eq!(
        h.requests[0].url,
        format!("{API}/v1/dashboard/positions/database/{OWNER}")
    );
}

#[test]
fn rest_reads_request_json_without_a_protocol_envelope() {
    let mut host = Fake {
        http_response: Some(HttpResponse {
            status: 200,
            headers: vec![],
            body: br#"{"total":0,"limit":50,"skip":0,"results":[]}"#.to_vec(),
        }),
        ..Fake::default()
    };
    let url = format!("{API}/v2/markets/all?chainId=42161&isActive=true&skip=0&limit=50");
    let result = api::http_json(&mut host, "GET", url.clone(), None).unwrap();
    assert_eq!(result["total"], 0);
    assert_eq!(result["results"], json!([]));
    let request = &host.requests[0];
    assert_eq!(request.method, "GET");
    assert_eq!(request.url, url);
    assert!(request.body.is_empty());
    assert!(
        request
            .headers
            .iter()
            .any(|(k, v)| k == "Accept" && v == "application/json")
    );
    assert!(host.staged.is_empty());
}

#[test]
fn rest_reads_reject_non_json_without_echoing_upstream_body() {
    let mut host = Fake {
        http_response: Some(HttpResponse {
            status: 200,
            headers: vec![],
            body: b"data: private upstream detail".to_vec(),
        }),
        ..Fake::default()
    };
    assert_eq!(
        api::http_json(&mut host, "GET", format!("{API}/v2/markets/all"), None).unwrap_err(),
        "invalid Pendle JSON response"
    );
}

#[test]
fn rest_reads_preserve_http_failure_and_bound_response_size() {
    let mut host = Fake {
        http_response: Some(HttpResponse {
            status: 429,
            headers: vec![],
            body: b"private upstream detail".to_vec(),
        }),
        ..Fake::default()
    };
    let error =
        api::http_json(&mut host, "GET", format!("{API}/v2/markets/all"), None).unwrap_err();
    assert!(error.contains("HTTP 429"));
    assert!(!error.contains("private upstream detail"));
    host.http_response = Some(HttpResponse {
        status: 200,
        headers: vec![],
        body: vec![b' '; 512 * 1024 + 1],
    });
    assert_eq!(
        api::http_json(&mut host, "GET", format!("{API}/v2/markets/all"), None).unwrap_err(),
        "Pendle response is too large"
    );
}
fn sell_quote() -> Value {
    serde_json::from_str(include_str!("fixtures/quote-sell.json")).unwrap()
}
fn redeem_market() -> Market {
    serde_json::from_str(include_str!("fixtures/market-ethereum-srusde.json")).unwrap()
}
fn redeem_quote() -> Value {
    serde_json::from_str(include_str!("fixtures/quote-ethereum-srusde-redeem.json")).unwrap()
}
/// Stage the first step of a buy and have Bloom ask for its approval.
fn staged_with_approval(h: &mut Fake) {
    workflow::create(h, ARB, &account(), BUY, "one", &body()).unwrap();
    workflow::confirm(h, ARB, &account(), BUY, "one", b"confirm").unwrap();
    workflow::confirm(h, ARB, &account(), BUY, "one", b"confirm").unwrap();
}
const NEVER_SENT: &str = "Bloom refused, cancelled or expired an outbox step that can no longer land; reconcile prior steps and create a new reviewed intent";
const MAYBE_SENT: &str = "Bloom no longer sends an outbox step but may have broadcast it; check its hash on chain before creating a new intent";
const EXPIRED: &str = "quote expired; reconcile prior steps and create a new reviewed intent";
/// The expiry refusal for an operation whose staged step was never sent.
const EXPIRED_STAGED: &str = "quote expired; reconcile prior steps and create a new reviewed intent; first discard its unsent outbox entry by writing `cancel` to /wallets/alice/0/chains/arbitrum/outbox/pending/1-tx/confirm";
const TAMPERED: &str = "operation steps do not match the reviewed intent";

#[test]
fn entries_bloom_never_sent_end_the_operation() {
    // Bloom 0.4.1 leaves `pending` in the intent of an entry it expired or
    // refused under policy, and writes `cancelled` for one cancelled before
    // sending; later Bloom records why each left the queue.
    for state in ["pending", "failed", "cancelled"] {
        let mut h = Fake::default();
        staged_with_approval(&mut h);
        h.fail_entry(ARB, "1-tx", state);
        let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
        assert_eq!(
            (d.state.as_str(), d.steps[0].state.as_str()),
            ("failed", "failed")
        );
        // A failed step no longer shows the approval it had asked for.
        assert_eq!(d.steps[0].approval, None);
        let confirms = h.confirms.len();
        assert_eq!(
            workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap_err(),
            NEVER_SENT
        );
        // Nothing is confirmed again and nothing new is staged.
        assert_eq!((h.confirms.len(), h.staged.len()), (confirms, 1));
    }
    // With a transaction hash it may have been broadcast: keep waiting.
    let mut h = Fake::default();
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap();
    h.send_entry(ARB, "1-tx", "failed", "0xdef");
    let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!(d.steps[0].state, "submitted");
    assert_ne!(d.state, "failed");
}
#[test]
fn entries_bloom_may_have_broadcast_are_never_abandoned() {
    // A file in `failed/`, then the step's hash and its cancel's hash.
    let cases: [(&str, Option<&str>, Option<&str>); 5] = [
        // A lost submit reply leaves the hash only in Bloom's marker.
        ("broadcast_attempted.json", Some("0xfeed"), None),
        ("replacement_broadcast_attempted.json", None, None),
        ("cancel_broadcast_attempted.json", None, Some("0xfeed")),
        ("cancel_tx_hash", None, Some("0xfeed")),
        // Bloom reports it cancelled, but it is not where discarded entries go.
        ("", None, None),
    ];
    for status in ["pending", "cancelled"] {
        for (file, hash, cancel) in cases {
            let case = format!("{status} {file}");
            let mut h = Fake::default();
            staged_with_approval(&mut h);
            h.fail_entry(ARB, "1-tx", status);
            match file {
                "" => {
                    h.files
                        .remove(&outbox_path(ARB, "failed", "1-tx", "intent.json"));
                    h.outbox.insert("1-tx".into(), ("cancelled".into(), None));
                }
                // Bloom writes the cancel's hash as bare text.
                "cancel_tx_hash" => {
                    h.files
                        .insert(outbox_path(ARB, "failed", "1-tx", file), b"0xfeed".to_vec());
                }
                _ => {
                    h.files.insert(
                        outbox_path(ARB, "failed", "1-tx", file),
                        json!({"tx_hash": "0xfeed"}).to_string().into_bytes(),
                    );
                }
            }
            let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
            assert_eq!(
                (d.state.as_str(), d.steps[0].state.as_str()),
                ("broadcast_unknown", "broadcast_unknown"),
                "{case}"
            );
            assert_eq!(d.steps[0].tx_hash.as_deref(), hash, "{case}");
            assert_eq!(d.steps[0].cancel_tx_hash.as_deref(), cancel, "{case}");
            assert_eq!(d.steps[0].approval, None, "{case}");
            let confirms = h.confirms.len();
            assert_eq!(
                workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap_err(),
                MAYBE_SENT,
                "{case}"
            );
            assert_eq!((h.confirms.len(), h.staged.len()), (confirms, 1), "{case}");
        }
    }
}
#[test]
fn a_cancelled_send_may_still_have_mined() {
    // Bloom 0.4.1 moves a sent entry it cancels to `failed/`, keeps its
    // status and hash, and never looks for a receipt for either again.
    let mut h = Fake::default();
    staged_with_approval(&mut h);
    let created = h.created(ARB, "1-tx");
    h.send_entry(ARB, "1-tx", "sent", "0xorig");
    let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!(d.steps[0].state, "submitted");
    h.files
        .remove(&outbox_path(ARB, "sent", "1-tx", "intent.json"));
    for (file, hash) in [
        ("intent.json", "0xorig"),
        ("broadcast_attempted.json", "0xorig"),
        ("cancel_broadcast_attempted.json", "0xcancel"),
    ] {
        h.files.insert(
            outbox_path(ARB, "failed", "1-tx", file),
            json!({"action_id": "pending-action", "status": "sent", "tx_hash": hash, "created_ms": created})
                .to_string()
                .into_bytes(),
        );
    }
    h.files.insert(
        outbox_path(ARB, "failed", "1-tx", "cancel_tx_hash"),
        b"0xcancel".to_vec(),
    );
    let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!(
        (d.state.as_str(), d.steps[0].state.as_str()),
        ("broadcast_unknown", "broadcast_unknown")
    );
    let shown = serde_json::to_value(&d).unwrap();
    assert_eq!(shown["steps"][0]["tx_hash"], "0xorig");
    assert_eq!(shown["steps"][0]["cancel_tx_hash"], "0xcancel");
    assert!(shown["steps"][1].get("cancel_tx_hash").is_none());
    let confirms = h.confirms.len();
    assert_eq!(
        workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap_err(),
        MAYBE_SENT
    );
    assert_eq!((h.confirms.len(), h.staged.len()), (confirms, 1));
}
#[test]
fn an_unreadable_outbox_entry_is_an_error_not_a_guess() {
    for file in ["intent.json", "broadcast_attempted.json", "cancel_tx_hash"] {
        let mut h = Fake::default();
        staged_with_approval(&mut h);
        h.fail_entry(ARB, "1-tx", "pending");
        h.unreadable = Some(outbox_path(ARB, "failed", "1-tx", file));
        assert_eq!(
            workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap_err(),
            "outbox entry read failed",
            "{file}"
        );
        let confirms = h.confirms.len();
        assert!(workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").is_err());
        assert_eq!((h.confirms.len(), h.staged.len()), (confirms, 1), "{file}");
    }
}
#[test]
fn a_mined_cancel_receipt_fails_the_step() {
    // Bloom after 0.4.1 records `cancelled` once the same-nonce cancel mines.
    let mut h = Fake::default();
    staged_with_approval(&mut h);
    h.receipts.insert(
        "1-tx".into(),
        json!({"outcome": "cancelled", "tx_hash": "0xabc"}),
    );
    let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!(
        (d.state.as_str(), d.steps[0].state.as_str()),
        ("failed", "failed")
    );
}
#[test]
fn status_names_the_pending_owner_approval() {
    let mut h = Fake::default();
    staged_with_approval(&mut h);
    let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!(
        d.steps[0].approval,
        Some(workflow::Approval {
            action_id: "pending-action".into(),
            expires_ms: h.now + 60_000,
            kind: "transaction".into(),
        })
    );
    let shown = serde_json::to_value(&d).unwrap();
    assert_eq!(shown["steps"][0]["approval"]["kind"], "transaction");
    assert!(shown["steps"][1].get("approval").is_none());
    // Once Bloom broadcasts the step, the approval is gone.
    h.approved = true;
    workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap();
    let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!(d.steps[0].approval, None);
    // Any other action id is the wallet-policy approval for this package.
    let mut h = Fake {
        approval_action: "policy-update".into(),
        ..Fake::default()
    };
    staged_with_approval(&mut h);
    let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!(d.steps[0].approval.as_ref().unwrap().kind, "wallet_policy");
    // Without the entry's intent, the approval cannot be told apart, and the
    // Petal names no discard for an entry it cannot read.
    let mut h = Fake::default();
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap();
    h.files
        .remove(&outbox_path(ARB, "pending", "1-tx", "intent.json"));
    workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap();
    let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!(d.steps[0].approval.as_ref().unwrap().kind, "unknown");
    h.now += 300_001;
    assert_eq!(
        workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap_err(),
        EXPIRED
    );
}
#[test]
fn only_a_requested_transaction_approval_outlives_the_quote() {
    // A host error is not an approval request.
    let mut h = Fake::default();
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap();
    h.fail_confirm = true;
    assert!(workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").is_err());
    let d = workflow::load(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!(
        (d.state.as_str(), d.steps[0].approval.is_none()),
        ("staged", true)
    );
    h.now += 300_001;
    let confirms = h.confirms.len();
    assert_eq!(
        workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap_err(),
        EXPIRED_STAGED
    );
    assert_eq!(h.confirms.len(), confirms);

    // The wallet-policy approval is not the transaction's.
    let mut h = Fake {
        approval_action: "policy-update".into(),
        ..Fake::default()
    };
    staged_with_approval(&mut h);
    h.now += 300_001;
    let confirms = h.confirms.len();
    assert_eq!(
        workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap_err(),
        EXPIRED_STAGED
    );
    assert_eq!(h.confirms.len(), confirms);

    // An approval recorded without a kind counts as neither.
    let mut h = Fake::default();
    staged_with_approval(&mut h);
    let key = "operations/arbitrum/buy-pt/one.json";
    let mut record: Value = serde_json::from_slice(&h.store[key]).unwrap();
    record["steps"][0]["approval"]
        .as_object_mut()
        .unwrap()
        .remove("kind");
    h.store
        .insert(key.into(), serde_json::to_vec(&record).unwrap());
    h.now += 300_001;
    assert_eq!(
        workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap_err(),
        EXPIRED_STAGED
    );

    // An error after the transaction approval was requested keeps it, so a
    // ceremony the owner finishes after expiry still broadcasts.
    let mut h = Fake::default();
    staged_with_approval(&mut h);
    h.fail_confirm = true;
    assert!(workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").is_err());
    assert_eq!(
        workflow::load(&mut h, ARB, &account(), BUY, "one")
            .unwrap()
            .state,
        "approval_required"
    );
    h.now += 300_001;
    h.approved = true;
    workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap();
    assert_eq!(h.mined, BTreeSet::from(["1-tx".into()]));
}
#[test]
fn tampered_operation_steps_are_refused() {
    let mut h = Fake {
        allowance: U256::from(1),
        ..Fake::default()
    };
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    let key = "operations/arbitrum/buy-pt/one.json";
    let original: Value = serde_json::from_slice(&h.store[key]).unwrap();
    let steps = original["steps"].as_array().unwrap().clone();
    let (reset, approve, execute) = (&steps[0], &steps[1], &steps[2]);
    let unlimited = format!(
        "0x{}",
        hex::encode(
            approveCall {
                spender: address(ROUTER).unwrap(),
                amount: U256::MAX
            }
            .abi_encode()
        )
    );
    let mut unlimited_step = approve.clone();
    unlimited_step["transaction"]["data"] = json!(unlimited);
    let mut other_token = approve.clone();
    other_token["transaction"]["to"] = json!(market().pt.address);
    let mut nonzero_reset = reset.clone();
    nonzero_reset["transaction"]["data"] = approve["transaction"]["data"].clone();
    let mut renamed = reset.clone();
    renamed["kind"] = json!("transfer");
    let mut renamed_execution = execute.clone();
    renamed_execution["kind"] = json!("approve_input");
    let cases = [
        ("unlimited approval", vec![reset, &unlimited_step, execute]),
        ("another token", vec![reset, &other_token, execute]),
        ("nonzero reset", vec![&nonzero_reset, approve, execute]),
        ("unknown kind", vec![&renamed, approve, execute]),
        (
            "renamed execution",
            vec![reset, approve, &renamed_execution],
        ),
        ("reordered", vec![approve, reset, execute]),
        ("duplicated approval", vec![approve, approve, execute]),
        ("extra approval", vec![reset, approve, approve, execute]),
    ];
    for (case, steps) in cases {
        let mut record = original.clone();
        record["steps"] = json!(steps);
        h.store
            .insert(key.into(), serde_json::to_vec(&record).unwrap());
        assert_eq!(
            workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").err(),
            Some(TAMPERED.to_string()),
            "{case}"
        );
        assert!(h.staged.is_empty(), "{case}");
    }
}
#[test]
fn approval_steps_rebuild_for_every_operation() {
    let exact = U256::from(10u64.pow(18));
    let m = market();
    let r = redeem_market();
    // A buy that resets a leftover allowance, a sell and a redemption.
    let cases = [
        (
            ARB,
            BUY,
            m.clone(),
            quote(),
            U256::from(1),
            m.sy.address.clone(),
            m.sy.address.clone(),
        ),
        (
            ARB,
            Operation::SellPt,
            m.clone(),
            sell_quote(),
            U256::ZERO,
            m.sy.address.clone(),
            m.pt.address.clone(),
        ),
        (
            ETH,
            Operation::RedeemPt,
            r.clone(),
            redeem_quote(),
            U256::ZERO,
            r.underlying_asset.address.clone(),
            r.pt.address.clone(),
        ),
    ];
    for (chain, op, market, quote, allowance, token, spent) in cases {
        let mut h = Fake {
            expired: op == Operation::RedeemPt,
            market: market.clone(),
            quote,
            allowance,
            approved: true,
            ..Fake::default()
        };
        let request = DraftInput {
            market: market.address.clone(),
            token,
            amount: exact.to_string(),
            min_output: "1".into(),
            slippage_bps: 10,
            max_price_impact_bps: 100,
        };
        let body = serde_json::to_vec(&request).unwrap();
        workflow::create(&mut h, chain, &account(), op, "one", &body).unwrap();
        let steps = workflow::load(&mut h, chain, &account(), op, "one")
            .unwrap()
            .steps
            .len();
        // Each step stages, then broadcasts; approvals first, as built.
        for _ in 0..steps * 2 {
            workflow::confirm(&mut h, chain, &account(), op, "one", b"confirm").unwrap();
        }
        assert_eq!(
            workflow::reconcile(&mut h, chain, &account(), op, "one")
                .unwrap()
                .state,
            "execution_confirmed",
            "{op:?}"
        );
        // The allowance is read on the token spent, from owner to router.
        let spent_by = (
            address(&spent).unwrap(),
            address(OWNER).unwrap(),
            address(ROUTER).unwrap(),
        );
        assert!(
            h.allowance_calls.len() > 1 && h.allowance_calls.iter().all(|call| *call == spent_by),
            "{op:?}"
        );
        let amounts: Vec<U256> = h.staged[..steps - 1]
            .iter()
            .map(|tx| {
                assert_eq!(address(&tx.to).unwrap(), address(&spent).unwrap(), "{op:?}");
                let a = approveCall::abi_decode(&hex::decode(&tx.data_hex[2..]).unwrap()).unwrap();
                assert_eq!(a.spender, address(ROUTER).unwrap());
                a.amount
            })
            .collect();
        let expected = if allowance.is_zero() {
            vec![exact]
        } else {
            vec![U256::ZERO, exact]
        };
        assert_eq!(amounts, expected, "{op:?}");
        assert_eq!(
            address(&h.staged[steps - 1].to).unwrap(),
            address(ROUTER).unwrap()
        );
    }
}
/// The refusal for a step Bloom matched to `holder`'s pending entry `id`.
fn shared(id: &str, holder: &str) -> String {
    format!(
        "Bloom returned pending outbox entry {id} of operation {holder}; finish that operation, or discard the entry by writing `cancel` to /wallets/alice/0/chains/arbitrum/outbox/pending/{id}/confirm, then confirm this one again"
    )
}
#[test]
fn two_operations_never_share_an_outbox_entry() {
    // Two drafts of the same intent get the same calldata, and Bloom hands a
    // second stage of it the first one's pending entry.
    let mut h = Fake {
        dedupe: true,
        ..Fake::default()
    };
    for id in ["one", "two"] {
        workflow::create(&mut h, ARB, &account(), BUY, id, &body()).unwrap();
    }
    let confirm =
        |h: &mut Fake, id: &str| workflow::confirm(h, ARB, &account(), BUY, id, b"confirm");
    let status = |h: &mut Fake, id: &str| workflow::reconcile(h, ARB, &account(), BUY, id).unwrap();
    confirm(&mut h, "one").unwrap();
    assert_eq!(
        confirm(&mut h, "two").unwrap_err(),
        shared("1-tx", "buy-pt/one/0")
    );
    let two = status(&mut h, "two");
    assert_eq!(
        (
            two.state.as_str(),
            two.steps[0].state.as_str(),
            &two.steps[0].outbox_id
        ),
        ("prepared", "prepared", &None)
    );
    // Once "one" has finished, "two" stages and sends its own entries.
    h.approved = true;
    for _ in 0..3 {
        confirm(&mut h, "one").unwrap();
    }
    for _ in 0..4 {
        confirm(&mut h, "two").unwrap();
    }
    let (one, two) = (status(&mut h, "one"), status(&mut h, "two"));
    assert_eq!(
        (one.state.as_str(), two.state.as_str()),
        ("execution_confirmed", "execution_confirmed")
    );
    let ids = |d: &workflow::Draft| {
        d.steps
            .iter()
            .map(|s| s.outbox_id.clone().unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        (ids(&one), ids(&two)),
        (
            vec!["1-tx".into(), "2-tx".into()],
            vec!["3-tx".into(), "4-tx".into()]
        )
    );
    assert_eq!((h.mined.len(), h.allowance), (4, U256::ZERO));
}
#[test]
fn a_swap_whose_allowance_was_spent_is_refused() {
    // Interleaved, two exact approvals of the same amount leave the router
    // one amount, and the first swap spends it.
    let mut h = Fake {
        dedupe: true,
        approved: true,
        ..Fake::default()
    };
    for id in ["one", "two"] {
        workflow::create(&mut h, ARB, &account(), BUY, id, &body()).unwrap();
    }
    let confirm =
        |h: &mut Fake, id: &str| workflow::confirm(h, ARB, &account(), BUY, id, b"confirm");
    confirm(&mut h, "one").unwrap(); // stage one's approval
    confirm(&mut h, "one").unwrap(); // send it
    confirm(&mut h, "two").unwrap(); // stage two's approval
    confirm(&mut h, "two").unwrap(); // send it: the allowance is still 1e18
    confirm(&mut h, "one").unwrap(); // stage one's swap
    confirm(&mut h, "one").unwrap(); // send it: the allowance is now 0
    let staged = h.staged.len();
    assert_eq!(
        confirm(&mut h, "two").unwrap_err(),
        "the router may spend 0 of the input token, less than this operation's 1000000000000000000: another operation spent or replaced its approval; create a new reviewed intent"
    );
    // Nothing that would revert is staged or sent.
    assert_eq!((h.staged.len(), h.mined.len()), (staged, 3));
    // A staged swap is checked again before Bloom sends it.
    let mut h = Fake::default();
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    h.approved = true;
    confirm(&mut h, "one").unwrap();
    confirm(&mut h, "one").unwrap();
    confirm(&mut h, "one").unwrap(); // the swap is staged
    h.allowance = U256::ZERO;
    let confirms = h.confirms.len();
    assert!(
        confirm(&mut h, "one")
            .unwrap_err()
            .starts_with("the router may spend 0")
    );
    assert_eq!(h.confirms.len(), confirms);
}
#[test]
fn the_operation_state_follows_its_steps() {
    let state = |h: &mut Fake| {
        workflow::reconcile(h, ARB, &account(), BUY, "one")
            .unwrap()
            .state
    };
    let mut h = Fake::default();
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    assert_eq!(state(&mut h), "prepared");
    workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap();
    assert_eq!(state(&mut h), "staged");
    workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap();
    assert_eq!(state(&mut h), "approval_required");
    h.approved = true;
    workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap();
    // The approval mined and nothing is in flight: the swap is next.
    assert_eq!(state(&mut h), "prepared");
    h.now += 300_001;
    assert_eq!(state(&mut h), "expired");
    assert_eq!(
        workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap_err(),
        EXPIRED
    );

    // A staged step with no transaction approval stops with the quote; one
    // whose transaction approval Bloom requested does not.
    let mut h = Fake::default();
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap();
    h.now += 300_001;
    assert_eq!(state(&mut h), "expired");
    for (action, expired) in [
        ("pending-action", "approval_required"),
        ("policy-update", "expired"),
    ] {
        let mut h = Fake {
            approval_action: action.into(),
            ..Fake::default()
        };
        staged_with_approval(&mut h);
        h.now += 300_001;
        assert_eq!(state(&mut h), expired, "{action}");
    }
}
#[test]
fn step_states_win_over_expiry() {
    // Bloom sweeps an unsent entry an hour after staging, long after the
    // five-minute quote, so each of these is seen after expiry.
    let mut h = Fake::default();
    staged_with_approval(&mut h);
    h.now += 3_600_001;
    h.fail_entry(ARB, "1-tx", "pending");
    assert_eq!(
        workflow::reconcile(&mut h, ARB, &account(), BUY, "one")
            .unwrap()
            .state,
        "failed"
    );
    h.files.insert(
        outbox_path(ARB, "failed", "1-tx", "broadcast_attempted.json"),
        json!({"tx_hash": "0xfeed"}).to_string().into_bytes(),
    );
    assert_eq!(
        workflow::reconcile(&mut h, ARB, &account(), BUY, "one")
            .unwrap()
            .state,
        "broadcast_unknown"
    );
    let mut h = Fake::default();
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    h.lose_stage = true;
    assert!(workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").is_err());
    h.now += 300_001;
    assert_eq!(
        workflow::reconcile(&mut h, ARB, &account(), BUY, "one")
            .unwrap()
            .state,
        "staging_unknown"
    );
    let mut h = Fake::default();
    staged_with_approval(&mut h);
    h.send_entry(ARB, "1-tx", "sent", "0xorig");
    h.now += 300_001;
    assert_eq!(
        workflow::reconcile(&mut h, ARB, &account(), BUY, "one")
            .unwrap()
            .state,
        "submitted"
    );
    let confirms = h.confirms.len();
    assert_eq!(
        workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap_err(),
        "transaction submitted; poll status before advancing"
    );
    assert_eq!(h.confirms.len(), confirms);
}
#[test]
fn a_reverted_receipt_ends_the_operation() {
    for late in [0, 300_001] {
        let mut h = Fake::default();
        staged_with_approval(&mut h);
        h.receipts.insert(
            "1-tx".into(),
            json!({"outcome": "reverted", "tx_hash": "0xabc"}),
        );
        h.now += late;
        let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
        assert_eq!(
            (d.state.as_str(), d.steps[0].state.as_str()),
            ("reverted", "reverted")
        );
        let confirms = h.confirms.len();
        assert_eq!(
            workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap_err(),
            "operation requires reconciliation or manual recovery; it cannot be resubmitted"
        );
        assert_eq!((h.confirms.len(), h.staged.len()), (confirms, 1));
    }
}
#[test]
fn odd_failed_entry_files_never_read_as_never_sent() {
    for file in [
        "replacement_broadcast_attempted.json",
        "cancel_broadcast_attempted.json",
        "signing_unknown.json",
    ] {
        let mut h = Fake::default();
        staged_with_approval(&mut h);
        h.fail_entry(ARB, "1-tx", "pending");
        h.unreadable = Some(outbox_path(ARB, "failed", "1-tx", file));
        assert_eq!(
            workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap_err(),
            "outbox entry read failed",
            "{file}"
        );
    }
    // A torn marker and an intent the Petal cannot parse may hide a hash.
    for (file, bytes) in [
        ("broadcast_attempted.json", &b"{"[..]),
        ("intent.json", &b"{"[..]),
    ] {
        let mut h = Fake::default();
        staged_with_approval(&mut h);
        h.fail_entry(ARB, "1-tx", "pending");
        h.files
            .insert(outbox_path(ARB, "failed", "1-tx", file), bytes.to_vec());
        assert_eq!(
            workflow::reconcile(&mut h, ARB, &account(), BUY, "one")
                .unwrap()
                .state,
            "broadcast_unknown",
            "{file}"
        );
    }
}
#[test]
fn bloom_after_0_4_1_signing_unknown_may_have_been_sent() {
    // Bloom marks an entry an older Bloom released although it may have been
    // signed, and reports it `signing_unknown`.
    for (state, marked) in [
        ("signing_unknown", false),
        ("pending", true),
        ("signing_unknown", true),
    ] {
        let mut h = Fake::default();
        staged_with_approval(&mut h);
        h.fail_entry(ARB, "1-tx", state);
        if marked {
            h.files.insert(
                outbox_path(ARB, "failed", "1-tx", "signing_unknown.json"),
                json!({"tx_id": "1-tx"}).to_string().into_bytes(),
            );
        }
        let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
        assert_eq!(d.state, "broadcast_unknown", "{state} {marked}");
        assert_eq!(
            workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap_err(),
            MAYBE_SENT
        );
    }
}
#[test]
fn bloom_after_0_4_1_cancelling_waits_for_a_receipt() {
    // A sent entry's cancel is followed by Bloom until one of them mines.
    let mut h = Fake::default();
    staged_with_approval(&mut h);
    h.send_entry(ARB, "1-tx", "cancelling", "0xorig");
    h.now += 300_001;
    let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!(
        (d.state.as_str(), d.steps[0].state.as_str()),
        ("submitted", "submitted")
    );
    let confirms = h.confirms.len();
    assert!(workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").is_err());
    assert_eq!(h.confirms.len(), confirms);
    // A pending one, possibly signed but with no hash, is never settled.
    let mut h = Fake::default();
    staged_with_approval(&mut h);
    h.outbox.insert("1-tx".into(), ("cancelling".into(), None));
    h.files.insert(
        outbox_path(ARB, "pending", "1-tx", "cancel_tx_hash"),
        b"0xcancel".to_vec(),
    );
    let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!(d.state, "broadcast_unknown");
    assert_eq!(d.steps[0].cancel_tx_hash.as_deref(), Some("0xcancel"));
    // Bloom may record that cancel only in its private signing state.
    h.files
        .remove(&outbox_path(ARB, "pending", "1-tx", "cancel_tx_hash"));
    let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!(d.state, "broadcast_unknown");
}
#[test]
fn a_send_whose_reply_was_lost_stays_confirmable() {
    // Bloom writes the broadcast marker, then the submit reply is lost. The
    // entry stays pending, and Bloom settles the attempt on the next confirm.
    // 0.4.1 keeps the hash only in the marker; later Bloom reports it.
    for (reported, expired) in [(false, false), (false, true), (true, true)] {
        let mut h = Fake {
            approval_action: "policy-update".into(),
            ..Fake::default()
        };
        staged_with_approval(&mut h);
        if reported {
            h.outbox
                .insert("1-tx".into(), ("pending".into(), Some("0xswap".into())));
        } else {
            h.files.insert(
                outbox_path(ARB, "pending", "1-tx", "broadcast_attempted.json"),
                json!({"tx_hash": "0xswap"}).to_string().into_bytes(),
            );
        }
        if expired {
            h.now += 300_001;
        }
        let case = format!("reported {reported}, expired {expired}");
        let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
        assert_eq!(d.steps[0].state, "staged", "{case}");
        assert_eq!(d.steps[0].tx_hash.as_deref(), Some("0xswap"), "{case}");
        assert_ne!(d.state, "expired", "{case}");
        // Confirm asks Bloom again, without checking the allowance it spent.
        h.approved = true;
        h.allowance = U256::ZERO;
        let confirms = h.confirms.len();
        workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap();
        assert_eq!(h.confirms.len(), confirms + 1, "{case}");
    }
    // A swap that may have mined spent its own allowance: Bloom still
    // settles it on the next confirm.
    let mut h = Fake {
        allowance: U256::MAX,
        ..Fake::default()
    };
    staged_with_approval(&mut h);
    assert_eq!(h.staged.len(), 1);
    h.files.insert(
        outbox_path(ARB, "pending", "1-tx", "broadcast_attempted.json"),
        json!({"tx_hash": "0xswap"}).to_string().into_bytes(),
    );
    h.allowance = U256::ZERO;
    h.approved = true;
    workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap();
    assert_eq!(h.mined, BTreeSet::from(["1-tx".into()]));
    // A marker with no readable hash may still have gone out.
    let mut h = Fake::default();
    staged_with_approval(&mut h);
    h.files.insert(
        outbox_path(ARB, "pending", "1-tx", "broadcast_attempted.json"),
        b"{".to_vec(),
    );
    assert_eq!(
        workflow::reconcile(&mut h, ARB, &account(), BUY, "one")
            .unwrap()
            .state,
        "broadcast_unknown"
    );
}
#[test]
fn an_expired_operation_names_how_to_release_its_entry() {
    let mut h = Fake {
        dedupe: true,
        ..Fake::default()
    };
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap();
    h.now += 300_001;
    assert_eq!(
        workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap_err(),
        EXPIRED_STAGED
    );
    // The new intent finds the unsent entry until it is discarded.
    workflow::create(&mut h, ARB, &account(), BUY, "two", &body()).unwrap();
    assert_eq!(
        workflow::confirm(&mut h, ARB, &account(), BUY, "two", b"confirm").unwrap_err(),
        shared("1-tx", "buy-pt/one/0")
    );
    // Writing `cancel` makes Bloom discard it unsent.
    h.fail_entry(ARB, "1-tx", "cancelled");
    assert_eq!(
        workflow::reconcile(&mut h, ARB, &account(), BUY, "one")
            .unwrap()
            .state,
        "failed"
    );
    workflow::confirm(&mut h, ARB, &account(), BUY, "two", b"confirm").unwrap();
    let two = workflow::reconcile(&mut h, ARB, &account(), BUY, "two").unwrap();
    assert_eq!(two.steps[0].outbox_id.as_deref(), Some("2-tx"));
}
#[test]
fn claims_survive_lost_replies_and_refuse_store_errors() {
    // A claim written but reported lost is still this operation's.
    let mut h = Fake {
        claim_fault: Some("lost"),
        ..Fake::default()
    };
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap();
    let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!(
        (d.state.as_str(), d.steps[0].outbox_id.as_deref()),
        ("staged", Some("1-tx"))
    );
    assert_eq!(d.steps[0].outbox_created_ms, Some(1_000_000));
    // An unclaimed entry is never recorded: the step needs recovery.
    let mut h = Fake {
        claim_fault: Some("fail"),
        ..Fake::default()
    };
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    assert_eq!(
        workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap_err(),
        "store unavailable"
    );
    let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!(
        (d.state.as_str(), d.steps[0].outbox_id.as_deref()),
        ("staging_unknown", Some("1-tx"))
    );
    // The next confirm claims it without staging again.
    workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap();
    let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!((d.state.as_str(), h.staged.len()), ("staged", 1));
    // An outbox id that is not Bloom's form goes into no key.
    let mut h = Fake {
        stage_id: Some("../1-tx".into()),
        ..Fake::default()
    };
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    assert_eq!(
        workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap_err(),
        "Bloom returned an unexpected outbox id"
    );
    assert!(h.store.keys().all(|k| !k.starts_with("outbox-claims/")));
}
#[test]
fn a_reused_outbox_id_is_a_different_entry() {
    // Outbox ids repeat after Bloom restarts. An older entry's claim does not
    // block a new one, and its `failed/` record does not fail a live step.
    let mut h = Fake::default();
    h.store.insert(
        "outbox-claims/arbitrum/1-tx-999".into(),
        b"buy-pt/old/0".to_vec(),
    );
    staged_with_approval(&mut h);
    let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!(d.state, "approval_required");
    h.files.insert(
        outbox_path(ARB, "failed", "1-tx", "intent.json"),
        json!({"action_id": "old-action", "status": "pending", "created_ms": 999})
            .to_string()
            .into_bytes(),
    );
    let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!(d.state, "approval_required");
}
#[test]
fn a_refused_staged_swap_names_how_to_release_its_entry() {
    // The swap is staged while the allowance covers it, then another
    // operation spends the allowance.
    let mut h = Fake {
        allowance: U256::MAX,
        ..Fake::default()
    };
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap();
    h.allowance = U256::ZERO;
    assert_eq!(
        workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap_err(),
        "the router may spend 0 of the input token, less than this operation's 1000000000000000000: another operation spent or replaced its approval; create a new reviewed intent; first discard its unsent outbox entry by writing `cancel` to /wallets/alice/0/chains/arbitrum/outbox/pending/1-tx/confirm"
    );
}
#[test]
fn a_shared_entry_that_may_have_been_sent_is_never_offered_for_discard() {
    // "one" sent its approval, but the reply was lost; "two" approves the
    // same token and amount, and Bloom hands it one's pending entry.
    let mut h = Fake {
        dedupe: true,
        ..Fake::default()
    };
    for id in ["one", "two"] {
        workflow::create(&mut h, ARB, &account(), BUY, id, &body()).unwrap();
    }
    workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap();
    h.files.insert(
        outbox_path(ARB, "pending", "1-tx", "broadcast_attempted.json"),
        json!({"tx_hash": "0xfeed"}).to_string().into_bytes(),
    );
    assert_eq!(
        workflow::confirm(&mut h, ARB, &account(), BUY, "two", b"confirm").unwrap_err(),
        "Bloom returned pending outbox entry 1-tx of operation buy-pt/one/0; it may have been sent or signed, so write `confirm` to /petals/pendle/arbitrum/buy-pt/alice/0/one/confirm.json to let Bloom settle it, then confirm this one again"
    );
    // A replacement Bloom keeps pending is no longer Bloom's to settle.
    h.files.insert(
        outbox_path(ARB, "pending", "1-tx", "replacement_tx_hash"),
        b"0xrepl".to_vec(),
    );
    assert_eq!(
        workflow::confirm(&mut h, ARB, &account(), BUY, "two", b"confirm").unwrap_err(),
        "Bloom returned pending outbox entry 1-tx of operation buy-pt/one/0; it may have been sent and Bloom no longer settles it: check its hashes on chain, as Bloom hands the entry out until it expires an hour after staging, then confirm this one again"
    );
}
#[test]
fn a_send_bloom_cannot_settle_or_that_was_replaced_may_have_gone_out() {
    // Bloom could not settle a lost reply, or sent a same-nonce replacement
    // or cancel, and keeps the entry pending.
    for (file, bytes) in [
        (
            "reconcile_ambiguous.json",
            json!({"reason": "nonce advanced"}).to_string(),
        ),
        ("replacement_tx_hash", "0xrepl".to_owned()),
        (
            "replacement_broadcast_attempted.json",
            json!({"tx_hash": "0xrepl"}).to_string(),
        ),
        ("cancel_tx_hash", "0xcancel".to_owned()),
    ] {
        let mut h = Fake::default();
        staged_with_approval(&mut h);
        h.files.insert(
            outbox_path(ARB, "pending", "1-tx", "broadcast_attempted.json"),
            json!({"tx_hash": "0xswap"}).to_string().into_bytes(),
        );
        h.files.insert(
            outbox_path(ARB, "pending", "1-tx", file),
            bytes.into_bytes(),
        );
        h.now += 300_001;
        let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
        assert_eq!(
            (d.state.as_str(), d.steps[0].state.as_str()),
            ("broadcast_unknown", "broadcast_unknown"),
            "{file}"
        );
        assert_eq!(d.steps[0].tx_hash.as_deref(), Some("0xswap"), "{file}");
        assert_eq!(d.steps[0].approval, None, "{file}");
        if file.starts_with("replacement") {
            assert_eq!(d.steps[0].replacement_tx_hash.as_deref(), Some("0xrepl"));
        }
        let confirms = h.confirms.len();
        assert_eq!(
            workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap_err(),
            MAYBE_SENT,
            "{file}"
        );
        assert_eq!(h.confirms.len(), confirms, "{file}");
    }
    // A replacement's hash shows for a discarded entry too.
    let mut h = Fake::default();
    staged_with_approval(&mut h);
    h.fail_entry(ARB, "1-tx", "pending");
    h.files.insert(
        outbox_path(ARB, "failed", "1-tx", "replacement_tx_hash"),
        b"0xrepl".to_vec(),
    );
    let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!(d.state, "broadcast_unknown");
    assert_eq!(d.steps[0].replacement_tx_hash.as_deref(), Some("0xrepl"));
}
#[test]
fn a_lost_reply_after_an_approval_asks_for_no_ceremony() {
    let mut h = Fake::default();
    staged_with_approval(&mut h);
    h.files.insert(
        outbox_path(ARB, "pending", "1-tx", "broadcast_attempted.json"),
        json!({"tx_hash": "0xswap"}).to_string().into_bytes(),
    );
    let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!((d.state.as_str(), &d.steps[0].approval), ("staged", &None));
    // The market may have matured since: confirm still lets Bloom settle it.
    h.expired = true;
    h.approved = true;
    workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap();
    assert_eq!(h.mined, BTreeSet::from(["1-tx".into()]));
}
#[test]
fn a_step_under_a_reused_id_reads_its_own_entry() {
    // This step's entry expired into failed/, while an older entry under the
    // same id sits in sent/ with a receipt; Bloom reports the older one.
    let mut h = Fake::default();
    staged_with_approval(&mut h);
    h.fail_entry(ARB, "1-tx", "pending");
    h.files.insert(
        outbox_path(ARB, "sent", "1-tx", "intent.json"),
        json!({"status": "sent", "tx_hash": "0xold", "created_ms": 999})
            .to_string()
            .into_bytes(),
    );
    h.receipts.insert(
        "1-tx".into(),
        json!({"outcome": "success", "tx_hash": "0xold"}),
    );
    let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!((d.state.as_str(), &d.steps[0].tx_hash), ("failed", &None));
    // A finished step whose id a newer pending entry reuses keeps its own
    // receipt.
    let mut h = Fake::default();
    staged_with_approval(&mut h);
    h.send_entry(ARB, "1-tx", "sent", "0xmine");
    h.files.insert(
        outbox_path(ARB, "sent", "1-tx", "receipt.json"),
        json!({"outcome": "success", "tx_hash": "0xmine"})
            .to_string()
            .into_bytes(),
    );
    h.files.insert(
        outbox_path(ARB, "pending", "1-tx", "intent.json"),
        json!({"status": "pending", "created_ms": 5})
            .to_string()
            .into_bytes(),
    );
    let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!(
        (d.steps[0].state.as_str(), d.steps[0].tx_hash.as_deref()),
        ("confirmed", Some("0xmine"))
    );
    // Neither found: Bloom reports another entry, and this one is unknown.
    let mut h = Fake::default();
    staged_with_approval(&mut h);
    h.files.insert(
        outbox_path(ARB, "pending", "1-tx", "intent.json"),
        json!({"status": "pending", "created_ms": 5})
            .to_string()
            .into_bytes(),
    );
    let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!(d.state, "broadcast_unknown");
}
#[test]
fn a_resumed_claim_never_adopts_another_transaction() {
    // The claim failed; meanwhile the id names an entry with other calldata.
    let mut h = Fake {
        claim_fault: Some("fail"),
        ..Fake::default()
    };
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    assert!(workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").is_err());
    h.files.insert(
        outbox_path(ARB, "pending", "1-tx", "intent.json"),
        json!({"status": "pending", "created_ms": 5, "to": ROUTER, "value_wei": "0", "data_hex": "0xdead"})
            .to_string()
            .into_bytes(),
    );
    assert_eq!(
        workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap_err(),
        "Bloom's outbox entry 1-tx holds another transaction; inspect the account outbox before recovery"
    );
    assert!(h.store.keys().all(|k| !k.starts_with("outbox-claims/")));
}
#[test]
fn a_hash_on_a_step_with_no_entry_means_nothing() {
    let mut h = Fake::default();
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    let key = "operations/arbitrum/buy-pt/one.json";
    let mut record: Value = serde_json::from_slice(&h.store[key]).unwrap();
    record["steps"][0]["tx_hash"] = json!("0xforged");
    h.store
        .insert(key.into(), serde_json::to_vec(&record).unwrap());
    h.now += 300_001;
    let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!((d.state.as_str(), &d.steps[0].tx_hash), ("expired", &None));
    assert_eq!(
        workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap_err(),
        EXPIRED
    );
    assert!(h.staged.is_empty());
}
#[test]
fn an_unfinished_claim_resumes_on_the_next_confirm() {
    // Bloom stages the entry, then its intent cannot be read for the claim.
    let mut h = Fake {
        unreadable: Some(outbox_path(ARB, "pending", "1-tx", "intent.json")),
        ..Fake::default()
    };
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    assert_eq!(
        workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap_err(),
        "outbox entry read failed"
    );
    h.unreadable = None;
    let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!(
        (d.state.as_str(), d.steps[0].outbox_id.as_deref()),
        ("staging_unknown", Some("1-tx"))
    );
    // Meanwhile Bloom sent it: the claim reads its intent from sent/.
    h.send_entry(ARB, "1-tx", "sent", "0xone");
    workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap();
    let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!((d.state.as_str(), h.staged.len()), ("submitted", 1));
    assert!(h.store.contains_key("outbox-claims/arbitrum/1-tx-1000000"));
    // A lost stage reply leaves no id, and is never staged again.
    let mut h = Fake {
        lose_stage: true,
        ..Fake::default()
    };
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    assert!(workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").is_err());
    assert_eq!(
        workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap_err(),
        "staging outcome unknown; inspect the account outbox before recovery"
    );
    assert_eq!(h.staged.len(), 1);
}
#[test]
fn an_approval_on_a_step_with_no_entry_means_nothing() {
    let mut h = Fake::default();
    workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
    let key = "operations/arbitrum/buy-pt/one.json";
    let mut record: Value = serde_json::from_slice(&h.store[key]).unwrap();
    record["steps"][0]["approval"] =
        json!({"action_id": "pending-action", "expires_ms": 9_999_999u64, "kind": "transaction"});
    h.store
        .insert(key.into(), serde_json::to_vec(&record).unwrap());
    h.now += 300_001;
    let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!((d.state.as_str(), &d.steps[0].approval), ("expired", &None));
    assert_eq!(
        workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap_err(),
        EXPIRED
    );
    assert!(h.staged.is_empty());
}
#[test]
fn a_pending_cancel_marker_may_have_gone_out() {
    let mut h = Fake::default();
    staged_with_approval(&mut h);
    h.files.insert(
        outbox_path(ARB, "pending", "1-tx", "broadcast_attempted.json"),
        json!({"tx_hash": "0xswap"}).to_string().into_bytes(),
    );
    h.files.insert(
        outbox_path(ARB, "pending", "1-tx", "cancel_broadcast_attempted.json"),
        json!({"tx_hash": "0xcancel"}).to_string().into_bytes(),
    );
    let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!(d.state, "broadcast_unknown");
    assert_eq!(d.steps[0].cancel_tx_hash.as_deref(), Some("0xcancel"));
}
#[test]
fn a_pending_entry_wins_over_an_older_sent_entry_under_its_id() {
    // Bloom reads pending/ before sent/, so this step's pending entry is
    // the one Bloom reports.
    let mut h = Fake::default();
    staged_with_approval(&mut h);
    h.files.insert(
        outbox_path(ARB, "sent", "1-tx", "intent.json"),
        json!({"status": "sent", "tx_hash": "0xold", "created_ms": 999})
            .to_string()
            .into_bytes(),
    );
    let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!(d.state, "approval_required");
}
#[test]
fn a_fee_bumped_send_may_mine_under_its_replacement() {
    // Bloom 0.4.1 leaves a replaced sent entry in sent/ and follows only
    // the original hash; later Bloom records the receipt of whichever mines.
    let mut h = Fake::default();
    staged_with_approval(&mut h);
    h.send_entry(ARB, "1-tx", "sent", "0xone");
    h.files.insert(
        outbox_path(ARB, "sent", "1-tx", "replacement_tx_hash"),
        b"0xrepl".to_vec(),
    );
    let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!(d.state, "broadcast_unknown");
    assert_eq!(
        (
            d.steps[0].tx_hash.as_deref(),
            d.steps[0].replacement_tx_hash.as_deref()
        ),
        (Some("0xone"), Some("0xrepl"))
    );
    h.receipts.insert(
        "1-tx".into(),
        json!({"outcome": "success", "tx_hash": "0xrepl"}),
    );
    let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!(d.steps[0].state, "confirmed");
}
#[test]
fn a_wallet_policy_approval_for_a_lost_reply_stays_visible() {
    let mut h = Fake {
        approval_action: "policy-update".into(),
        ..Fake::default()
    };
    staged_with_approval(&mut h);
    h.files.insert(
        outbox_path(ARB, "pending", "1-tx", "broadcast_attempted.json"),
        json!({"tx_hash": "0xswap"}).to_string().into_bytes(),
    );
    let d = workflow::reconcile(&mut h, ARB, &account(), BUY, "one").unwrap();
    assert_eq!(d.state, "approval_required");
    assert_eq!(d.steps[0].approval.as_ref().unwrap().kind, "wallet_policy");
}
#[test]
fn a_discard_is_offered_only_while_the_approval_awaits_its_ceremony() {
    // The owner granted the transaction approval, then confirm failed: Bloom
    // may have started signing, which only Bloom can see.
    let refused_after = |challenge: Option<&str>| {
        let mut h = Fake {
            allowance: U256::MAX,
            ..Fake::default()
        };
        workflow::create(&mut h, ARB, &account(), BUY, "one", &body()).unwrap();
        workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap();
        workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap();
        if let Some(state) = challenge {
            h.files.insert(
                outbox_path(ARB, "pending", "1-tx", "approval_challenge.json"),
                json!({"state": state}).to_string().into_bytes(),
            );
        }
        h.allowance = U256::ZERO;
        workflow::confirm(&mut h, ARB, &account(), BUY, "one", b"confirm").unwrap_err()
    };
    let hint = "; first discard its unsent outbox entry by writing `cancel` to /wallets/alice/0/chains/arbitrum/outbox/pending/1-tx/confirm";
    assert!(refused_after(Some("awaiting_ceremony")).ends_with(hint));
    for challenge in [None, Some("active"), Some("exhausted"), Some("unknown")] {
        let refusal = refused_after(challenge);
        assert!(
            refusal.ends_with("create a new reviewed intent"),
            "{challenge:?}: {refusal}"
        );
    }
}
