//! `cua-connections-gen [--check]`: regenerates (or verifies) everything
//! derived from `providers/*.toml`.

use std::process::ExitCode;

fn main() -> ExitCode {
    let check = std::env::args().any(|a| a == "--check");
    let root = cua_connections::generate::repo_root();
    if check {
        match cua_connections::generate::check(&root) {
            Ok(stale) if stale.is_empty() => {
                println!("connections: generated files are current");
                ExitCode::SUCCESS
            }
            Ok(stale) => {
                eprintln!("connections: stale generated files (run `cargo run -p cua-connections --bin cua-connections-gen`):");
                for s in stale {
                    eprintln!("  {s}");
                }
                ExitCode::FAILURE
            }
            Err(e) => {
                eprintln!("connections: {e}");
                ExitCode::FAILURE
            }
        }
    } else {
        match cua_connections::generate::write(&root) {
            Ok(changed) => {
                for c in &changed {
                    println!("wrote {c}");
                }
                println!("connections: {} file(s) updated", changed.len());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("connections: {e}");
                ExitCode::FAILURE
            }
        }
    }
}
