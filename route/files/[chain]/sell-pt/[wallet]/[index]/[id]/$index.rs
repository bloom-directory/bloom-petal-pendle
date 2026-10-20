petal::route_file!(spec: petal::static_dir_spec(), list: vec![petal::writable("draft.json"), petal::file("plan.md"), petal::writable("confirm.json"), petal::file("status.json")]);
