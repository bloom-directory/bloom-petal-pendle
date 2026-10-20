petal::route_file!(spec: petal::store_read_spec().caps(&["bloom:store","bloom:vfs.read"]), read: |ctx: &petal::Ctx| { let account=match pendle_route::selected(ctx) { Ok(a)=>a,Err(e)=>return e };
    let id=match petal::param(ctx,"id") { Ok(v)=>v,Err(e)=>return e };
    let op=pendle_route::Operation::SellPt;
    match pendle_route::workflow::load(&mut pendle_route::host::BloomHost,&account,op,id) {
        Ok(draft)=>petal::DispatchResponse::Read(pendle_route::workflow::plan(&draft).into_bytes()),
        Err(e)=>petal::error(-4,e)
    }
});
