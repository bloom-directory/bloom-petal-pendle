petal::route_file!(
    spec: petal::write_spec().caps(&["bloom:store","bloom:chain","bloom:vfs.read","bloom:tx.outbox"]),
    read: |_ctx: &petal::Ctx| petal::read_json_value(&serde_json::json!({"write":"confirm","effect":"advance one existing outbox step; owner approval remains required","retry":"inspect status and owner outbox; never change the intent under an existing id"})),
    write: |ctx: &petal::Ctx,body: &[u8]| { let account=match pendle_route::selected(ctx) { Ok(a)=>a,Err(e)=>return e };
    let id=match petal::param(ctx,"id") { Ok(v)=>v,Err(e)=>return e };
    let op=pendle_route::Operation::SellPt;
        pendle_route::written(pendle_route::workflow::confirm(&mut pendle_route::host::BloomHost,&account,op,id,body))
    }
);
