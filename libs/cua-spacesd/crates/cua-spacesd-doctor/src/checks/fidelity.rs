// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The report's `fidelity` block: facts that change how a task behaves,
//! with every key always present so two variants diff key by key. Some are
//! filled by the groups that measure them (tz, clock skew, encoder, audio
//! backend, CPUs, memory, display); the rest are gathered here, read-only.
//! Commands that describe what tasks see (locale, app versions) run through
//! `ProcessService`, in the same environment tasks get.

use std::path::Path;
use std::time::Duration;

use cua_spacesd_client::diagnose::Fidelity;
use cua_spacesd_client::pb;
use cua_spacesd_client::Command;
use sha2::{Digest, Sha256};

use crate::sys;
use crate::Ctx;

/// SHA-256 of sorted, newline-joined lines (empty input hashes to "").
pub fn sorted_hash(text: &str) -> String {
    let mut lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    lines.sort_unstable();
    lines.dedup();
    hex::encode(Sha256::digest(lines.join("\n").as_bytes()))
}

/// `name=version` lines minus the packages named in `excluded` (one name
/// per line: what an image variant's own layer added, e.g. the VM kernel).
pub fn without_packages(listing: &str, excluded: &str) -> String {
    let skip: std::collections::BTreeSet<&str> = excluded
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    listing
        .lines()
        .filter(|l| !skip.contains(l.split('=').next().unwrap_or_default()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// CPU model from `/proc/cpuinfo` text.
pub fn cpu_model(cpuinfo: &str) -> String {
    for key in ["model name", "Model", "Hardware", "cpu model"] {
        if let Some(v) = cpuinfo.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            (k.trim() == key).then(|| v.trim().to_owned())
        }) {
            if !v.is_empty() {
                return v;
            }
        }
    }
    // arm64 without a model string: implementer/part.
    let field = |key: &str| {
        cpuinfo.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            (k.trim() == key).then(|| v.trim().to_owned())
        })
    };
    match (field("CPU implementer"), field("CPU part")) {
        (Some(i), Some(p)) => format!("arm implementer {i} part {p}"),
        _ => String::new(),
    }
}

async fn run_text(ctx: &Ctx, program: &str, args: &[&str]) -> Option<String> {
    let out = ctx
        .client
        .run(
            Command::new(program)
                .args(args.iter().copied())
                .timeout(Duration::from_secs(15)),
        )
        .await
        .ok()?;
    out.success().then(|| out.stdout_str())
}

fn gpu() -> String {
    if Path::new("/dev/nvidia0").exists() {
        return "nvidia".into();
    }
    let Ok(entries) = std::fs::read_dir("/dev/dri") else {
        return "none".into();
    };
    let render: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
        .filter(|n| n.starts_with("renderD"))
        .take(8)
        .collect();
    match render.first() {
        None => "none".into(),
        Some(node) => {
            let vendor =
                sys::read_capped(Path::new(&format!("/sys/class/drm/{node}/device/vendor")))
                    .map(|v| v.trim().to_owned())
                    .unwrap_or_default();
            format!(
                "drm:{node}{}",
                if vendor.is_empty() {
                    String::new()
                } else {
                    format!(" vendor {vendor}")
                }
            )
        }
    }
}

/// Finishes the fidelity block.
pub async fn collect(ctx: &Ctx) -> Fidelity {
    let mut f = ctx.fidelity.lock().await.clone();
    f.runtime = ctx.runtime.clone();
    f.init = ctx.init.clone();
    f.kernel = ctx
        .caps
        .os
        .as_ref()
        .map(|o| o.kernel.clone())
        .unwrap_or_default();
    if f.display.is_empty() {
        if let Ok(displays) = ctx.client.displays().await {
            if let Some(d) = displays.iter().find(|d| d.primary).or(displays.first()) {
                let b = d.bounds.unwrap_or_default();
                f.display = format!(
                    "{}x{}@{}",
                    b.width,
                    b.height,
                    if d.scale_factor > 0.0 {
                        d.scale_factor
                    } else {
                        1.0
                    }
                );
            }
        }
    }
    if f.cpu_count == 0 || f.memory_mib == 0 {
        if let Ok(m) = ctx
            .client
            .system()
            .metrics(pb::MetricsRequest::default())
            .await
        {
            let m = m.into_inner();
            f.cpu_count = m.cpu_count;
            f.memory_mib = m.memory_total_bytes / (1024 * 1024);
        }
    }
    f.a11y_backend = match (ctx.os(), ctx.supports("a11y")) {
        (_, false) => "none".into(),
        ("macos", true) => "ax".into(),
        ("windows", true) => "uia".into(),
        _ => "atspi".into(),
    };
    if f.audio_backend.is_empty() {
        f.audio_backend = if ctx.supports("audio.desktop") {
            match ctx.os() {
                "macos" => "screencapturekit".into(),
                "windows" => "wasapi".into(),
                _ => "pulse".into(),
            }
        } else {
            "none".into()
        };
    }
    if f.encoder.is_empty() {
        f.encoder = "none".into();
    }
    if f.tz.is_empty() {
        f.tz = super::time::guest_tz();
    }
    if ctx.os() == "linux" {
        f.cpu_model = cpu_model(&sys::read_capped(Path::new("/proc/cpuinfo")).unwrap_or_default());
        f.gpu = gpu();
        if let Ok(out) = sys::run_local(
            "fc-list",
            &["--format", "%{file}\n"],
            &[],
            Duration::from_secs(20),
        )
        .await
        {
            f.fonts_sha256 = sorted_hash(&out.stdout);
        }
        if let Ok(out) = sys::run_local(
            "dpkg-query",
            &["-W", "-f", "${Package}=${Version}\n"],
            &[],
            Duration::from_secs(20),
        )
        .await
        {
            let variant =
                sys::read_capped(Path::new("/etc/cua-image/variant-packages")).unwrap_or_default();
            f.packages_sha256 = sorted_hash(&without_packages(&out.stdout, &variant));
        }
        f.locale = run_text(ctx, "/bin/sh", &["-c", "echo \"${LC_ALL:-${LANG:-}}\""])
            .await
            .map(|s| s.trim().to_owned())
            .unwrap_or_default();
    } else {
        f.gpu = if f.gpu.is_empty() {
            "unknown".into()
        } else {
            f.gpu
        };
    }
    // `software` recorded these when it ran; probe here when it was not
    // selected, so the block is complete either way.
    let manifest = &ctx.manifest.manifest;
    if f.app_versions.is_empty() && !manifest.apps.is_empty() {
        f.app_versions = super::software::probe(ctx, &manifest.apps).await;
    }
    if f.tool_versions.is_empty() && !manifest.tools.is_empty() {
        f.tool_versions = super::software::probe(ctx, &manifest.tools).await;
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_order_independent() {
        assert_eq!(sorted_hash("b\na\n"), sorted_hash("a\nb"));
        assert_eq!(sorted_hash(""), "");
        assert_ne!(sorted_hash("a"), sorted_hash("b"));
    }

    #[test]
    fn variant_packages_are_left_out() {
        let listing = "bash=5.2\nlinux-image-virtual=6.8\nsystemd=255\nfirefox=130";
        let kept = without_packages(listing, "linux-image-virtual\nsystemd\n");
        assert_eq!(kept, "bash=5.2\nfirefox=130");
        assert_eq!(sorted_hash(&kept), sorted_hash("firefox=130\nbash=5.2"));
    }

    #[test]
    fn cpu_models() {
        assert_eq!(cpu_model("model name\t: Intel Xeon\n"), "Intel Xeon");
        assert_eq!(
            cpu_model("CPU implementer\t: 0x61\nCPU part\t: 0x000\n"),
            "arm implementer 0x61 part 0x000"
        );
        assert_eq!(cpu_model(""), "");
    }
}
