#![cfg(not(target_os = "linux"))]

fn assert_menu_definition(compat: bool) {
    let registry = platform_linux::tools::build_registry(compat);
    let definition = registry.get_def("invoke_menu").expect("menu stub exists");
    let contract = cua_driver_contract::tool_contract("invoke_menu").unwrap();
    assert_eq!(
        contract.schema_mode,
        cua_driver_contract::SchemaMode::PortableSubset
    );
    assert_eq!(definition.name, contract.name);
    assert_eq!(definition.description, contract.description);
    assert_eq!(definition.input_schema, contract.input_schema);
    assert_eq!(definition.read_only, contract.annotations.read_only);
    assert_eq!(definition.destructive, contract.annotations.destructive);
    assert_eq!(definition.idempotent, contract.annotations.idempotent);
    assert_eq!(definition.open_world, contract.annotations.open_world);
    assert!(definition.destructive && definition.open_world);
    let list = registry.tools_list();
    assert_eq!(
        list["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|tool| tool["name"] == "invoke_menu")
            .count(),
        1
    );
}

#[test]
fn standard_registry_constructs_portable_menu_stub() {
    assert_menu_definition(false);
}

#[test]
fn compat_registry_constructs_portable_menu_stub() {
    assert_menu_definition(true);
}
