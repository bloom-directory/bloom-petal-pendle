petal::route_file!(spec: petal::http_read_spec(30_000), read: |ctx: &petal::Ctx| {
    let chain = match petal::param(ctx,"chain") { Ok(v)=>v,Err(e)=>return e };
    if let Err(e)=pendle_route::chain(chain) { return petal::route_invalid(e); }
    let market = match petal::param(ctx,"market") { Ok(v)=>v,Err(e)=>return e };
    pendle_route::response(pendle_route::api::market(&mut pendle_route::host::BloomHost,market))
});
