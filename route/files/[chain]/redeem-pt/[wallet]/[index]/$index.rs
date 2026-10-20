petal::route_file!(spec: petal::store_dir_spec().caps(&["bloom:store"]), ctx_list: |ctx: &petal::Ctx| {
    let _account=pendle_route::selected(ctx)?;
    let keys=petal::sdk::store_list("operations/redeem-pt/",32*1024).map_err(|_|petal::error(-4,"operation listing failed"))?;
    Ok(keys.into_iter().filter_map(|key|key.strip_prefix("operations/redeem-pt/").and_then(|s|s.strip_suffix(".json")).map(str::to_owned)).map(petal::dir).collect())
});
