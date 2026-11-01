//! The generated files match the provider files: a stale one fails here, in
//! `cargo nextest`, and in `cua-connections-gen --check`.

#[test]
fn generated_files_are_current() {
    let root = cua_connections::generate::repo_root();
    let stale = cua_connections::generate::check(&root).expect("render");
    assert!(
        stale.is_empty(),
        "stale generated files: {stale:?}\nrun: cargo run -p cua-connections --bin cua-connections-gen"
    );
}

#[test]
fn the_tables_hold_every_provider_file() {
    let root = cua_connections::generate::repo_root();
    let defs =
        cua_connections::schema::load_dir(&root.join(cua_connections::generate::PROVIDERS_DIR))
            .expect("provider files");
    assert_eq!(defs.len(), cua_connections::PROVIDERS.len());
    for (d, p) in defs.iter().zip(cua_connections::PROVIDERS) {
        assert_eq!(d.id, p.id);
        assert_eq!(d.method.len(), p.methods.len(), "{}", d.id);
    }
}
