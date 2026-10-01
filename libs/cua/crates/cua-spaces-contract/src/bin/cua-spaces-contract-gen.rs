//! Writes `libs/cua/spaces-contract/manifest.json`, or with `--check` fails
//! when the checked-in file no longer matches the Rust it is generated from.
//!
//! The `--check` mode is the whole point: a contract that can rot silently is
//! documentation, not a contract.

use std::path::PathBuf;

fn main() {
    let check = std::env::args()
        .skip(1)
        .any(|argument| argument == "--check");
    let contract_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical libs/cua root")
        .join("spaces-contract");
    let output = contract_root.join("manifest.json");
    let generated = cua_spaces_contract::manifest_json();

    if check {
        let existing = std::fs::read_to_string(&output)
            .unwrap_or_else(|error| panic!("read {}: {error}", output.display()));
        if existing != generated {
            eprintln!(
                "{} is stale; run: cargo run -p cua-spaces-contract --bin cua-spaces-contract-gen",
                output.display()
            );
            std::process::exit(1);
        }
        println!("{} is up to date.", output.display());
        return;
    }

    std::fs::create_dir_all(&contract_root).expect("create contract directory");
    std::fs::write(&output, generated)
        .unwrap_or_else(|error| panic!("write {}: {error}", output.display()));
    println!("wrote {}", output.display());
}
