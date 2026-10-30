petal::route_file!(spec: petal::http_read_spec(30_000), read: |ctx: &petal::Ctx| {
    let chain = match pendle_route::route_chain(ctx) { Ok(c)=>c,Err(e)=>return e };
    let market = match petal::param(ctx,"market") { Ok(v)=>v,Err(e)=>return e };
    pendle_route::response(pendle_route::api::market(&mut pendle_route::host::BloomHost,chain,market))
});
