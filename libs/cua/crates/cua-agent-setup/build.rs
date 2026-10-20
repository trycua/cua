//! Files this crate embeds from elsewhere in the repository (skills).
//!
//! In the repository they are read from their source of truth; the crate
//! published to crates.io carries copies under `vendor/` (copied by
//! libs/cua/scripts/publish-crates.sh). Either way they land in OUT_DIR and
//! the code embeds them from there.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

/// (repository path relative to this crate, name in OUT_DIR and vendor/):
/// a file or a directory.
const FILES: &[(&str, &str)] = &[("../../skills", "skills")];

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
    for (repo_path, name) in FILES {
        let repo = manifest_dir.join(repo_path);
        let source = if repo.exists() {
            repo
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
