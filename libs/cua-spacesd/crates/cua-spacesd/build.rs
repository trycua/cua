// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Embeds the source revision as `CUA_SPACESD_GIT_SHA`: the build
//! environment's `CUA_SPACESD_GIT_SHA` wins (image builds in containers
//! without `.git`), else `git rev-parse HEAD`, else empty. `main` hands it to
//! `cua_driver_core::build_info` so `health_report` (served in-process) and
//! `build-info` report the running build. Reruns when HEAD or its ref moves.

use std::path::PathBuf;
use std::process::Command;

fn rerun_if_exists(path: &std::path::Path) {
    if path.exists() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

/// Watches the branch ref `reference` (for example `refs/heads/main`). While
/// it is a loose file, watch that file. When it is packed, git writes a new
/// loose file at the next commit without touching HEAD, the reflog (it may be
/// disabled) or packed-refs, so watch the nearest existing directory above it
/// instead: creating the file changes that directory. Paths that do not exist
/// are never watched (Cargo would treat them as always changed).
fn rerun_if_ref_changes(common: &std::path::Path, reference: &str) {
    let refs = common.join("refs");
    let mut path = common.join(reference);
    loop {
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
            return;
        }
        match path.parent() {
            Some(parent) if parent.starts_with(&refs) => path = parent.to_path_buf(),
            _ => return,
        }
    }
}

fn git(args: &[&str]) -> Option<String> {
    let dir = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (!text.is_empty()).then_some(text)
}

fn main() {
    // A musl Linux build is the static headless guest daemon that Cua injects
    // into any OCI image: exec, files, processes, ports, no desktop (no
    // cua-driver platform, capture, presence or doctor).
    println!("cargo:rustc-check-cfg=cfg(cua_headless)");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("musl") {
        println!("cargo:rustc-cfg=cua_headless");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // The linked Windows driver expects physical pixels. Declare the
        // process context before any async, native, or overlay thread starts.
        // The DPI manifest (cua-spacesd.rc) and the version resource, in one
        // resource script written to OUT_DIR (the manifest beside it).
        println!("cargo:rerun-if-changed=cua-spacesd.rc");
        println!("cargo:rerun-if-changed=cua-spacesd.manifest");
        let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
        let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
        let manifest = std::fs::read_to_string("cua-spacesd.rc").expect("cua-spacesd.rc");
        std::fs::copy("cua-spacesd.manifest", out.join("cua-spacesd.manifest"))
            .expect("copy cua-spacesd.manifest");
        let rc = out.join("cua-spacesd-resources.rc");
        std::fs::write(
            &rc,
            format!(
                "{manifest}\n{}",
                version_resource(
                    "Cua Spaces background service",
                    "cua-spacesd",
                    "Cua Spaces",
                    version.trim()
                )
            ),
        )
        .expect("write the resource script");
        embed_resource::compile_for(&rc, ["cua-spacesd"], embed_resource::NONE);
    }

    println!("cargo:rerun-if-env-changed=CUA_SPACESD_GIT_SHA");
    let from_env = std::env::var("CUA_SPACESD_GIT_SHA").unwrap_or_default();
    let sha = if !from_env.trim().is_empty() {
        from_env.trim().to_owned()
    } else {
        git(&["rev-parse", "HEAD"]).unwrap_or_default()
    };
    // Only paths that exist are watched: Cargo treats a missing
    // `rerun-if-changed` path as always changed, which rebuilt this crate on
    // every cargo invocation of a CI checkout (it has no packed-refs file).
    // The HEAD reflog is appended by every commit, checkout and reset, so it
    // follows the checkout even when the branch ref is packed.
    if let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        let git_dir = PathBuf::from(git_dir);
        rerun_if_exists(&git_dir.join("HEAD"));
        rerun_if_exists(&git_dir.join("logs").join("HEAD"));
        if let Some(common) = git(&["rev-parse", "--git-common-dir"]) {
            // git prints a relative path relative to the directory it ran in.
            let common = PathBuf::from(common);
            let common = if common.is_absolute() {
                common
            } else {
                PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default()).join(common)
            };
            rerun_if_exists(&common.join("packed-refs"));
            if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"]) {
                rerun_if_ref_changes(&common, &reference);
            }
        }
    }
    println!("cargo:rustc-env=CUA_SPACESD_GIT_SHA={sha}");
}

/// The Windows version resource (`VS_VERSION_INFO`): what Explorer's
/// Details tab and the Windows Firewall prompt show. The prompt reads the
/// name from `FileDescription` and the publisher from `CompanyName`, not
/// from the Authenticode signature, so without it a signed binary showed
/// Publisher "Unknown" (CUA-3515). Numeric constants only, so no Windows
/// SDK header is needed: 1 = VS_VERSION_INFO, 0x40004 = VOS_NT_WINDOWS32,
/// 1 = VFT_APP.
fn version_resource(description: &str, internal: &str, product: &str, version: &str) -> String {
    let mut parts = version
        .split(['.', '-', '+'])
        .map(|p| p.parse::<u16>().unwrap_or(0))
        .chain(std::iter::repeat(0));
    let numeric: Vec<String> = (0..4)
        .map(|_| parts.next().unwrap_or(0).to_string())
        .collect();
    let numeric = numeric.join(",");
    format!(
        r#"1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEFLAGSMASK 0x3F
FILEFLAGS 0x0
FILEOS 0x40004
FILETYPE 0x1
FILESUBTYPE 0x0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Cua AI, Inc."
      VALUE "FileDescription", "{description}"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "{internal}"
      VALUE "LegalCopyright", "Copyright (c) 2026 Cua AI, Inc."
      VALUE "OriginalFilename", "{internal}.exe"
      VALUE "ProductName", "{product}"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    )
}
