petal::route_file!(spec: petal::static_dir_spec(), ctx_list: |ctx: &petal::Ctx| {
    pendle_route::route_chain(ctx)?;
    Ok(vec![petal::file("markets.json"), petal::dir("markets"), petal::dir("accounts"), petal::dir("buy-pt"), petal::dir("sell-pt"), petal::dir("redeem-pt")])
});
