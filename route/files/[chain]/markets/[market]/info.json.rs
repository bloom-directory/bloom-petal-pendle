petal::route_file!(spec: petal::http_read_spec(30_000), read: |ctx: &petal::Ctx| {
    let chain = match pendle_route::route_chain(ctx) { Ok(c)=>c,Err(e)=>return e };
    let market = match petal::param(ctx,"market") { Ok(v)=>v,Err(e)=>return e };
    let mut host=pendle_route::host::BloomHost;
    // The market endpoint answers an uncurated market with an undocumented
    // 404, so the curated list is asked first and its refusal names the reason.
    pendle_route::response(
        pendle_route::api::curated(&mut host,chain,market)
            .and_then(|()| pendle_route::api::market(&mut host,chain,market)),
    )
});
