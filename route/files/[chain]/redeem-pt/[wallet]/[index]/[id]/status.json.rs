petal::route_file!(spec: petal::store_read_spec().caps(&["bloom:store","bloom:vfs.read","bloom:tx.outbox"]), read: |ctx: &petal::Ctx| { let account=match pendle_route::selected(ctx) { Ok(a)=>a,Err(e)=>return e };
    let id=match petal::param(ctx,"id") { Ok(v)=>v,Err(e)=>return e };
    let op=pendle_route::Operation::RedeemPt;
    pendle_route::response(pendle_route::workflow::reconcile(&mut pendle_route::host::BloomHost,&account,op,id))
});
