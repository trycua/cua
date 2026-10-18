// Bake Swift runtime rpaths into the cua-driver binary on macOS.
//
// The `screencapturekit` dep ships a small Swift-bridge shim that links
// against the Swift Concurrency runtime (`@rpath/libswift_Concurrency.dylib`
// and friends). Its own build.rs emits `cargo:rustc-link-arg=-Wl,-rpath,…`
// directives, but those only flow through to the binary linker when the
// emitting crate is the final binary crate — for transitive deps Cargo
// silently drops them. So we re-emit the same rpaths from here.
//
// On Windows, embed the Per-Monitor V2 DPI-awareness manifest so the
// process sees physical pixels (no DWM coordinate virtualization) at
// 125%/150%/200% scaling and clicks land where screenshots say they do.

fn main() {
    embed_git_sha();
    println!("cargo:rerun-if-env-changed=CUA_DRIVER_REVIEW_EXTENSION_PUBLIC_KEY_BASE64");
    validate_review_trust_root_build();

    #[cfg(target_os = "windows")]
    {
        embed_resource::compile("cua-driver.rc", embed_resource::NONE);
    }

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    emit_sdk_framework_search_path();
    emit_swift_runtime_link_args();
}

/// Embeds the source revision as `CUA_DRIVER_GIT_SHA` (read by `main` and
/// handed to `cua_driver_core::build_info::init`): the `CUA_DRIVER_GIT_SHA`
/// build environment wins (release builds in containers without `.git`),
/// else `git rev-parse HEAD`, else empty. Reruns when HEAD or its ref moves.
fn embed_git_sha() {
    println!("cargo:rerun-if-env-changed=CUA_DRIVER_GIT_SHA");
    let from_env = std::env::var("CUA_DRIVER_GIT_SHA").unwrap_or_default();
    let sha = if !from_env.trim().is_empty() {
        from_env.trim().to_owned()
    } else {
        git(&["rev-parse", "HEAD"]).unwrap_or_default()
    };
    // Rebuild when the checkout moves: HEAD itself (worktrees keep it in
    // their own git dir), the branch ref it names, and packed refs.
    if let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        let git_dir = std::path::PathBuf::from(git_dir);
        println!("cargo:rerun-if-changed={}", git_dir.join("HEAD").display());
        if let Some(common) = git(&["rev-parse", "--git-common-dir"]) {
            let common = std::path::PathBuf::from(common);
            let common = if common.is_absolute() {
                common
            } else {
                git_dir.join(common)
            };
            println!(
                "cargo:rerun-if-changed={}",
                common.join("packed-refs").display()
            );
            if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"]) {
                println!(
                    "cargo:rerun-if-changed={}",
                    common.join(reference).display()
                );
            }
        }
    }
    println!("cargo:rustc-env=CUA_DRIVER_GIT_SHA={sha}");
}

fn git(args: &[&str]) -> Option<String> {
    let dir = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let out = std::process::Command::new("git")
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

fn validate_review_trust_root_build() {
    if std::env::var_os("CARGO_FEATURE_REVIEW_TRUST_ROOT").is_none() {
        return;
    }
    if std::env::var("PROFILE").as_deref() == Ok("release") {
        panic!("review-trust-root is review-only and cannot be enabled in release artifacts");
    }
    let key = std::env::var("CUA_DRIVER_REVIEW_EXTENSION_PUBLIC_KEY_BASE64").expect(
        "review-trust-root requires CUA_DRIVER_REVIEW_EXTENSION_PUBLIC_KEY_BASE64 at build time",
    );
    let key = key.trim().as_bytes();
    if key.len() != 44
        || key[43] != b'='
        || !key[..43]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/'))
    {
        panic!("review-trust-root override must be standard base64 for exactly one 32-byte Ed25519 public key");
    }
}

fn emit_sdk_framework_search_path() {
    if let Some(sdk_root) = active_macos_sdk_root() {
        println!(
            "cargo:rustc-link-search=framework={}",
            sdk_root.join("System/Library/Frameworks").display()
        );
    }
}

fn emit_swift_runtime_link_args() {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");

    let mut dirs = BTreeSet::new();

    // `swiftc` links against the SDK's Swift TBDs on current Xcode releases.
    // The toolchain runtime directories below contain compatibility libraries,
    // but are not guaranteed to contain the full Swift runtime surface.
    if let Some(sdk_root) = active_macos_sdk_root() {
        let sdk_swift_dir = sdk_root.join("usr/lib/swift");
        if sdk_swift_dir.is_dir() {
            println!("cargo:rustc-link-search=native={}", sdk_swift_dir.display());
        }
    }

    if let Ok(out) = std::process::Command::new("xcode-select")
        .arg("-p")
        .output()
    {
        if out.status.success() {
            let xcode_path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let developer_dir = PathBuf::from(xcode_path);
            for sub in [
                "Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/macosx",
                "Toolchains/XcodeDefault.xctoolchain/usr/lib/swift-5.5/macosx",
                "Toolchains/XcodeDefault.xctoolchain/usr/lib/swift-6.2/macosx",
                "usr/lib/swift/macosx",
                "usr/lib/swift-5.5/macosx",
                "usr/lib/swift-6.2/macosx",
            ] {
                dirs.insert(developer_dir.join(sub));
            }
        }
    }

    if let Ok(out) = Command::new("xcrun").args(["--find", "swiftc"]).output() {
        if out.status.success() {
            let swiftc = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim().to_string());
            if let Some(usr_dir) = swiftc.parent().and_then(Path::parent) {
                for sub in [
                    "lib/swift/macosx",
                    "lib/swift-5.5/macosx",
                    "lib/swift-6.2/macosx",
                ] {
                    dirs.insert(usr_dir.join(sub));
                }
            }
        }
    }

    for dir in dirs {
        if dir.is_dir() {
            println!("cargo:rustc-link-search=native={}", dir.display());
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", dir.display());
        }
    }
}

fn active_macos_sdk_root() -> Option<std::path::PathBuf> {
    std::env::var_os("SDKROOT")
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_dir())
        .or_else(|| {
            let out = std::process::Command::new("xcrun")
                .args(["--sdk", "macosx", "--show-sdk-path"])
                .output()
                .ok()?;
            out.status
                .success()
                .then(|| std::path::PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
                .filter(|path| path.is_dir())
        })
}
