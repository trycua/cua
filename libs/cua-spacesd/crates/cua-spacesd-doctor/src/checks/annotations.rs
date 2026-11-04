// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `annotations`: the labels the image is published with (recorded in its
//! manifest) agree with the live guest. The registry side (the pushed
//! index's annotations against this manifest) is compared by the host-side
//! `cua doctor` and the image-doctor workflow; the guest cannot see the
//! registry.

use cua_spacesd_client::diagnose::Check;

use crate::{Ctx, Recorder};

/// Runtimes a variant may run under.
pub fn runtimes_for(variant: &str) -> &'static [&'static str] {
    match variant {
        "rootfs" => &["container", "gvisor"],
        "containerdisk" => &["qemu", "kubevirt"],
        "lume" => &["lume"],
        _ => &[],
    }
}

pub async fn run(ctx: &Ctx, rec: &mut Recorder<'_>) {
    if !rec.wants_group("annotations") {
        return;
    }
    if let Some(want) = ctx.options.expect_runtime.clone() {
        rec.push(
            Check::new(
                "annotations.runtime",
                super::verdict(want == ctx.runtime),
                format!("guest detects runtime {:?}, started under {want:?}", ctx.runtime),
            )
            .fix("GetCapabilities runtime detection is wrong for this runtime (budgets and parity use it)"),
            &["core"],
        )
        .await;
    }
    if !ctx.manifest.present() {
        return;
    }
    let manifest = &ctx.manifest.manifest;
    let claims: &[&str] = &["manifest:annotations"];
    let get = |key: &str| manifest.annotations.get(key).cloned().unwrap_or_default();

    let os = get("ai.cua.image.os");
    rec.push(
        Check::new(
            "annotations.os",
            super::verdict(os == ctx.os() && manifest.os == ctx.os()),
            format!(
                "ai.cua.image.os={os:?}, manifest os {:?}, guest {}",
                manifest.os,
                ctx.os()
            ),
        ),
        claims,
    )
    .await;

    let spacesd = get("ai.cua.spacesd");
    let legacy = get("ai.cua.env-driver");
    rec.push(
        Check::new(
            "annotations.spacesd",
            super::verdict(spacesd == "true" && legacy == "true" && manifest.spacesd.present),
            format!("ai.cua.spacesd={spacesd:?}, ai.cua.env-driver={legacy:?}; cua-spacesd is running"),
        )
        .fix("the image labels say it has no spacesd but one answers; labels must come from the built image"),
        claims,
    )
    .await;

    let variant = get("ai.cua.image.variant");
    let allowed = runtimes_for(&manifest.variant);
    rec.push(
        Check::new(
            "annotations.variant",
            super::verdict(variant == manifest.variant && allowed.contains(&ctx.runtime.as_str())),
            format!(
                "variant {} (label {variant:?}) runs under {} (allowed: {})",
                manifest.variant,
                ctx.runtime,
                allowed.join(", ")
            ),
        )
        .fix("a rootfs image runs under docker/gVisor, a containerDisk under QEMU/KubeVirt"),
        claims,
    )
    .await;
}

#[cfg(test)]
mod tests {
    #[test]
    fn variants_map_to_runtimes() {
        assert!(super::runtimes_for("rootfs").contains(&"gvisor"));
        assert!(super::runtimes_for("containerdisk").contains(&"kubevirt"));
        assert!(super::runtimes_for("nope").is_empty());
    }
}
