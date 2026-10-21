petal::route_file!(spec: petal::http_read_spec(30_000), read: |ctx: &petal::Ctx| {
    let chain = match petal::param(ctx,"chain") { Ok(v)=>v,Err(e)=>return e };
    if let Err(e)=pendle_route::chain(chain) { return petal::route_invalid(e); }
    pendle_route::response(pendle_route::api::http_json(
        &mut pendle_route::host::BloomHost,
        "GET",
        format!("{}/v2/markets/all?chainId=42161&isActive=true&skip=0&limit=50", pendle_route::API),
        None,
    ))
});
