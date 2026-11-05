petal::route_file!(spec: petal::static_dir_spec(), list: std::iter::once(petal::file("status.json")).chain(pendle_route::Chain::ALL.iter().map(|chain| petal::dir(chain.name()))).collect());
