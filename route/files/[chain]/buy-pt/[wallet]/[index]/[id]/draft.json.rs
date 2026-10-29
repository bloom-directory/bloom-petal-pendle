petal::route_file!(
    spec: petal::write_spec().caps(&["bloom:http","bloom:store","bloom:chain","bloom:vfs.read"]),
    read: |_ctx: &petal::Ctx| petal::read_json_value(&serde_json::json!({"write_body":{"market":"0x...","token":"0x...","amount":"1000000","min_output":"1","slippage_bps":10,"max_price_impact_bps":100},"units":"positive decimal base units","receiver":"selected account only","effect":"prepare immutable quote; does not stage or sign"})),
    write: |ctx: &petal::Ctx,body: &[u8]| { let (chain,account)=match pendle_route::selected(ctx) { Ok(a)=>a,Err(e)=>return e };
    let id=match petal::param(ctx,"id") { Ok(v)=>v,Err(e)=>return e };
    let op=pendle_route::Operation::BuyPt;
        pendle_route::written(pendle_route::workflow::create(&mut pendle_route::host::BloomHost,chain,&account,op,id,body))
    }
);
