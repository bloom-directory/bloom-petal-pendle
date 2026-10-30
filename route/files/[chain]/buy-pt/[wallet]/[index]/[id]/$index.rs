petal::route_file!(spec: petal::static_dir_spec(), ctx_list: |ctx: &petal::Ctx| {
    pendle_route::route_chain(ctx)?;
    Ok(vec![petal::writable("draft.json"), petal::file("plan.md"), petal::writable("confirm.json"), petal::file("status.json")])
});
