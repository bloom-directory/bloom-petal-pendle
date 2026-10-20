petal::route_file!(spec: petal::wallet_http_read_spec(10_000), read: |ctx: &petal::Ctx| {
    let account = match pendle_route::selected(ctx) { Ok(a)=>a,Err(e)=>return e };
    let mut host=pendle_route::host::BloomHost;
    let owner=match pendle_route::host::account_address(&mut host,&account) { Ok(v)=>v,Err(e)=>return petal::error(-4,e) };
    pendle_route::response(pendle_route::api::query(&mut host,"get_portfolio",serde_json::json!({"user":owner})))
});
