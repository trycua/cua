// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `--help` must never print a secret taken from the environment, and every
//! secret-looking flag in the Cua CLIs must say so (`hide_env_values`).

use std::path::{Path, PathBuf};
use std::process::Command;

const SECRET_ENVS: &[&str] = &[
    "CUA_RELAY_TOKENS",
    "CUA_RELAY_ADMIN_TOKEN",
    "CUA_RELAY_WEB_CLIENT_SECRET",
    "CUA_RELAY_POSTHOG_KEY",
    "CUA_RELAY_SPACES_API_CLIENT_SECRET",
    "CUA_RELAY_INTERNAL_TOKEN",
];

#[test]
fn help_never_prints_secrets_from_the_environment() {
    for flag in ["--help", "-h"] {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cua-relay"));
        cmd.arg(flag).env_clear();
        for (i, name) in SECRET_ENVS.iter().enumerate() {
            cmd.env(name, format!("DUMMY-SECRET-{i}-{name}"));
        }
        let out = cmd.output().expect("run cua-relay");
        assert!(out.status.success());
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("--listen"), "help printed: {text}");
        assert!(
            !text.contains("DUMMY-SECRET"),
            "{flag} printed a secret:\n{text}"
        );
    }
}

/// Source-level guard across every Cua CLI: an `env = "...SECRET|TOKEN|KEY|
/// PASSWORD..."` arg that is not a file/URL/path must hide its value.
#[test]
fn secret_env_args_hide_their_values() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut files = Vec::new();
    for dir in ["cua-spacesd", "cua/crates"] {
        walk(&root.join(dir), &mut files);
    }
    assert!(!files.is_empty());
    let mut bad = Vec::new();
    for f in files {
        let src = std::fs::read_to_string(&f).unwrap();
        let mut rest = src.as_str();
        while let Some(at) = rest.find("env = \"") {
            let after = &rest[at + 7..];
            let Some(end) = after.find('"') else { break };
            let name = &after[..end];
            // The attribute ends at the closing `)]`.
            let attr_end = after.find(")]").unwrap_or(after.len());
            let attr = &after[..attr_end];
            let secretish = ["SECRET", "TOKEN", "KEY", "PASSWORD"]
                .iter()
                .any(|w| name.contains(w));
            // Names that only mention a secret: paths, URLs, rates, and
            // on/off switches about one (e.g. CUA_RELAY_STRICT_INTERNAL_TOKEN).
            let not_secret = ["_FILE", "_URL", "_PATH", "_POLL_MS", "_PER_TOKEN"]
                .iter()
                .any(|w| name.ends_with(w))
                || name.contains("_STRICT_");
            if secretish && !not_secret && !attr.contains("hide_env_values") {
                bad.push(format!("{}: {name}", f.display()));
            }
            rest = &after[end..];
        }
    }
    assert!(
        bad.is_empty(),
        "secret env args that print their value in --help: {bad:#?}"
    );
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name();
        if p.is_dir() {
            if name != "target" && name != "tests" && name != ".git" {
                walk(&p, out);
            }
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}
