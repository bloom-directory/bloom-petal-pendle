use alloy::{
    primitives::{Address, U256},
    sol_types::{SolCall, SolValue},
};
use pendle_route::{api::Market, evm::*, host::Host, workflow, *};
use petal::sdk::{EvmTransaction, HttpRequest, HttpResponse, OutboxInspection, StagedTransaction};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const OWNER: &str = "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf";
fn market() -> Market {
    serde_json::from_str(include_str!("fixtures/market.json")).unwrap()
}
fn quote() -> Value {
    serde_json::from_str(include_str!("fixtures/quote.json")).unwrap()
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
fn body() -> Vec<u8> {
    serde_json::to_vec(&request()).unwrap()
}
fn account() -> petal::WalletAccount {
    petal::testing::wallet_account("alice", 0)
}
fn tx() -> Transaction {
    serde_json::from_value(quote()["routes"][0]["tx"].clone()).unwrap()
}

struct Fake {
    store: BTreeMap<String, Vec<u8>>,
    now: u64,
    quote: Value,
    expired: bool,
    factory: bool,
    allowance: U256,
    http_calls: usize,
    http_response: Option<HttpResponse>,
    requests: Vec<HttpRequest>,
    staged: Vec<EvmTransaction>,
    lose_stage: bool,
    approved: bool,
    confirmed: bool,
}
impl Default for Fake {
    fn default() -> Self {
        Self {
            store: BTreeMap::new(),
            now: 1_000_000,
            quote: quote(),
            expired: false,
            factory: true,
            allowance: U256::ZERO,
            http_calls: 0,
            http_response: None,
            requests: vec![],
            staged: vec![],
            lose_stage: false,
            approved: false,
            confirmed: false,
        }
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
        let payload = if req.method == "GET" {
            serde_json::to_value(market()).unwrap()
        } else {
            self.quote.clone()
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
    fn chain(&mut self, method: &str, params: &str) -> Result<String> {
        assert_eq!(method, "eth_call");
        let p: Value = serde_json::from_str(params).unwrap();
        let data = p[0]["data"].as_str().unwrap();
        let bytes = hex::decode(&data[2..]).unwrap();
        let m = market();
        let response = if bytes.starts_with(&isValidMarketCall::SELECTOR) {
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
        } else if bytes.starts_with(&getTokensInCall::SELECTOR)
            || bytes.starts_with(&getTokensOutCall::SELECTOR)
        {
            vec![address(&m.underlying_asset.address)?].abi_encode()
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
    fn confirm(&mut self, _wallet: &str, id: &str) -> Result<StagedTransaction> {
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
        self.confirmed = true;
        Ok(StagedTransaction {
            outbox_id: id.into(),
            plan_md: "reviewed".into(),
            approval: None,
        })
    }
    fn inspect(&mut self, _wallet: &str, id: &str) -> Result<OutboxInspection> {
        Ok(OutboxInspection {
            outbox_id: id.into(),
            state: if self.confirmed { "success" } else { "staged" }.into(),
            tx_hash: self.confirmed.then(|| "0xabc".into()),
            receipt_json: self
                .confirmed
                .then(|| json!({"outcome":"success","tx_hash":"0xabc"}).to_string()),
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
    assert!(chain("arbitrum").is_ok());
    assert!(chain("ethereum").is_err());
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
    assert!(validate_transaction(&tx(), OWNER, Operation::BuyPt, &request(), &market()).is_ok());
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
            validate_transaction(&t, OWNER, Operation::BuyPt, &request(), &market()).is_err(),
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
        assert!(validate_transaction(&t, OWNER, Operation::BuyPt, &request(), &market()).is_err());
    }
}
#[test]
fn reject_embedded_limit_order_and_wrong_operation() {
    let mut t = tx();
    let mut c = swapExactSyForPtCall::abi_decode(&hex::decode(&t.data[2..]).unwrap()).unwrap();
    c.limit.optData = vec![1].into();
    t.data = format!("0x{}", hex::encode(c.abi_encode()));
    assert!(validate_transaction(&t, OWNER, Operation::BuyPt, &request(), &market()).is_err());
    assert!(validate_transaction(&tx(), OWNER, Operation::SellPt, &request(), &market()).is_err());
}
#[test]
fn draft_is_idempotent_and_never_stages() {
    let mut h = Fake::default();
    workflow::create(&mut h, &account(), Operation::BuyPt, "one", &body()).unwrap();
    let count = h.http_calls;
    workflow::create(&mut h, &account(), Operation::BuyPt, "one", &body()).unwrap();
    assert_eq!(h.http_calls, count);
    assert!(h.staged.is_empty());
    let d = workflow::load(&mut h, &account(), Operation::BuyPt, "one").unwrap();
    assert_eq!(d.steps.len(), 2);
    let mut changed = request();
    changed.amount = "2".into();
    assert!(
        workflow::create(
            &mut h,
            &account(),
            Operation::BuyPt,
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
    workflow::create(&mut h, &account(), Operation::BuyPt, "one", &body()).unwrap();
    let d = workflow::load(&mut h, &account(), Operation::BuyPt, "one").unwrap();
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
    workflow::create(&mut h, &account(), Operation::BuyPt, "one", &body()).unwrap();
    assert_eq!(
        workflow::load(&mut h, &account(), Operation::BuyPt, "one")
            .unwrap()
            .steps
            .len(),
        1
    );
}
#[test]
fn lost_stage_reply_never_creates_a_second_outbox() {
    let mut h = Fake::default();
    workflow::create(&mut h, &account(), Operation::BuyPt, "one", &body()).unwrap();
    h.lose_stage = true;
    assert!(workflow::confirm(&mut h, &account(), Operation::BuyPt, "one", b"confirm").is_err());
    assert!(workflow::confirm(&mut h, &account(), Operation::BuyPt, "one", b"confirm").is_err());
    assert_eq!(h.staged.len(), 1);
    assert_eq!(
        workflow::load(&mut h, &account(), Operation::BuyPt, "one")
            .unwrap()
            .state,
        "staging_unknown"
    );
}
#[test]
fn approval_retries_reuse_the_same_outbox_id() {
    let mut h = Fake::default();
    workflow::create(&mut h, &account(), Operation::BuyPt, "one", &body()).unwrap();
    workflow::confirm(&mut h, &account(), Operation::BuyPt, "one", b"confirm").unwrap();
    workflow::confirm(&mut h, &account(), Operation::BuyPt, "one", b"confirm").unwrap();
    assert_eq!(
        workflow::load(&mut h, &account(), Operation::BuyPt, "one")
            .unwrap()
            .state,
        "approval_required"
    );
    assert_eq!(h.staged.len(), 1);
    h.approved = true;
    workflow::confirm(&mut h, &account(), Operation::BuyPt, "one", b"confirm").unwrap();
    assert_eq!(h.staged.len(), 1);
}
#[test]
fn expired_quote_and_wrong_confirmation_do_not_stage() {
    let mut h = Fake::default();
    workflow::create(&mut h, &account(), Operation::BuyPt, "one", &body()).unwrap();
    assert!(workflow::confirm(&mut h, &account(), Operation::BuyPt, "one", b"yes").is_err());
    h.now += 300_001;
    assert!(workflow::confirm(&mut h, &account(), Operation::BuyPt, "one", b"confirm").is_err());
    assert!(h.staged.is_empty());
}
#[test]
fn forged_account_cannot_load_another_accounts_record() {
    let mut h = Fake::default();
    workflow::create(&mut h, &account(), Operation::BuyPt, "one", &body()).unwrap();
    assert!(
        workflow::load(
            &mut h,
            &petal::testing::wallet_account("alice", 1),
            Operation::BuyPt,
            "one"
        )
        .is_err()
    );
    assert!(
        workflow::load(
            &mut h,
            &petal::testing::wallet_account("bob", 0),
            Operation::BuyPt,
            "one"
        )
        .is_err()
    );
}
#[test]
fn status_reads_do_not_mutate_state_or_send_transactions() {
    let mut h = Fake::default();
    workflow::create(&mut h, &account(), Operation::BuyPt, "one", &body()).unwrap();
    let original = h.store.clone();
    workflow::reconcile(&mut h, &account(), Operation::BuyPt, "one").unwrap();
    assert_eq!(h.store, original);
    assert!(h.staged.is_empty());
}
#[test]
fn concurrent_or_crashed_writer_lock_refuses_execution() {
    let mut h = Fake::default();
    workflow::create(&mut h, &account(), Operation::BuyPt, "one", &body()).unwrap();
    h.store
        .insert("locks/buy-pt/one".into(), b"crashed writer".to_vec());
    assert!(workflow::confirm(&mut h, &account(), Operation::BuyPt, "one", b"confirm").is_err());
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
        assert!(workflow::create(&mut h, &account(), Operation::BuyPt, "one", &body()).is_err());
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
        assert!(workflow::create(&mut h, &account(), Operation::BuyPt, "one", &body()).is_err());
        assert!(h.store.is_empty());
    }
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
fn every_pinned_deployment_is_a_canonical_address() {
    // Visit all factories, rather than stopping after the first successful
    // fake response. This catches transcription mistakes in later entries.
    let mut h = Fake {
        factory: false,
        ..Fake::default()
    };
    let error = verify_market(
        &mut h,
        &market(),
        Operation::BuyPt,
        address(&market().sy.address).unwrap(),
    )
    .unwrap_err();
    assert_eq!(
        error,
        "market is not authenticated by a supported Pendle factory"
    );
    assert_eq!(address(ROUTER).unwrap().to_string().len(), 42);
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
