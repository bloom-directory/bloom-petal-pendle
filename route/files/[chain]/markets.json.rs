petal::route_file!(spec: petal::http_read_spec(30_000), read: |ctx: &petal::Ctx| {
    let chain = match petal::param(ctx,"chain") { Ok(v)=>v,Err(e)=>return e };
    if let Err(e)=pendle_route::chain(chain) { return petal::route_invalid(e); }
    pendle_route::response(pendle_route::api::query(&mut pendle_route::host::BloomHost,"get_markets",serde_json::json!({"filter":[{"field":"chainId","op":"=","value":42161}],"include":["name","expiry","pt","yt","sy","details_impliedApy","details_liquidity"],"limit":50})))
});
