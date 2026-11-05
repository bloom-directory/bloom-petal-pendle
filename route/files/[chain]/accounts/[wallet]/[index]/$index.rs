petal::route_file!(spec: petal::static_dir_spec(), ctx_list: |ctx: &petal::Ctx| {
    pendle_route::route_chain(ctx)?;
    Ok(vec![petal::file("portfolio.json")])
});
