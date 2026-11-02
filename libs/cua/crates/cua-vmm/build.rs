//! Files this crate embeds from elsewhere in the repository (the macOS image
//! recipe that builds the developer tools image on the user's own Mac).
//!
//! The crate reads committed copies under `recipe/`, so everything it compiles
//! lives inside libs/cua (the Rust build cache key hashes that tree). The
//! source of truth stays in libs/images/macos; a test in scripts/images/tests
//! fails when a copy falls behind (refresh with
//! `scripts/images/sync-cua-vmm-recipe.sh`). The crate published to crates.io
//! also carries copies under `vendor/` (copied by
//! libs/cua/scripts/publish-crates.sh). Either way they land in OUT_DIR and
//! the code embeds them from there.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

/// (committed copy relative to this crate, name in OUT_DIR and vendor/):
/// a file or a directory.
const FILES: &[(&str, &str)] = &[
    ("recipe/macos-derive-local.sh", "macos-derive-local.sh"),
    ("recipe/macos-versions.env", "macos-versions.env"),
    ("recipe/macos-tier-xcode.sh", "macos-tier-xcode.sh"),
    ("recipe/macos-fetch.sh", "macos-fetch.sh"),
    ("recipe/macos-xcode-source.sh", "macos-xcode-source.sh"),
];

fn copy(from: &Path, to: &Path) -> io::Result<()> {
    if from.is_dir() {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(from, to).map(|_| ())
    }
}

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    println!("cargo:rerun-if-changed=build.rs");
    for (repo_path, name) in FILES {
        let committed = manifest_dir.join(repo_path);
        let source = if committed.exists() {
            committed
        } else {
            manifest_dir.join("vendor").join(name)
        };
        println!("cargo:rerun-if-changed={}", source.display());
        let target = out_dir.join(name);
        let _ = fs::remove_dir_all(&target);
        copy(&source, &target).unwrap_or_else(|e| {
            panic!(
                "{}: {e} (libs/cua/scripts/publish-crates.sh vendors it)",
                source.display()
            )
        });
    }
}
