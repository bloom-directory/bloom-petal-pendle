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
        self.staged.push(tx.clone());
        if self.lose_stage {
            return Err("lost reply".into());
        }
        Ok(StagedTransaction {
            outbox_id: format!("{}-tx", self.staged.len()),
            plan_md: "owner review".into(),
            approval: None,
        })
    }
    fn confirm(&mut self, _wallet: &str, chain: Chain, id: &str) -> Result<StagedTransaction> {
        self.confirms.push(chain);
        if !self.approved {
            return Ok(StagedTransaction {
                outbox_id: id.into(),
                plan_md: "owner approval pending".into(),
                approval: Some(petal::sdk::OutboxApproval {
                    action_id: "pending-action".into(),
                    expires_ms: self.now + 60_000,
                }),
            });
        }
        self.mined.insert(id.into());
        Ok(StagedTransaction {
            outbox_id: id.into(),
            plan_md: "reviewed".into(),
            approval: None,
        })
    }
    fn inspect(&mut self, _wallet: &str, chain: Chain, id: &str) -> Result<OutboxInspection> {
        self.inspects.push(chain);
        let mined = self.mined.contains(id);
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
    assert!(
        h.store
            .keys()
            .all(|k| k.starts_with("operations/ethereum/buy-pt/"))
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
