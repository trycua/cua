// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The New Space wizard's plan, and the one SDK call it maps to.
//!
//! The wizard (ui/src/spaceWizard.ts) picks an image from the shared list
//! (`libs/images/sandbox-images.json`), where it runs (Cua Cloud or This
//! machine), resources and a name. [`plan_call`] turns that into exactly
//! one `cua-spaces` call:
//!
//! | Where | Call |
//! |---|---|
//! | This machine | `Spaces::create(SpaceCreate { on: local, image, kind, runtime, name, cpus, memory_mb, spacesd })` |
//! | Cua Cloud | `Spaces::create(SpaceCreate { on: cloud, image, kind, runtime, name, spacesd })` |
//!
//! `kind` comes from the entry's `variant` (container or vm) and `runtime`
//! from its `local` (container: auto, gVisor when available; qemu; lume) or
//! `cloud` (gvisor, kubevirt) engine.
//!
//! The image entry is looked up in the shared list, so the UI cannot ask for
//! a runtime the image does not support.

use crate::{Error, Result};
use cua_sandbox_core::placement::{Kind, On, Runtime};
use cua_spaces::SpaceCreate;
use serde::{Deserialize, Serialize};

/// The shared image list, compiled in (one source for the docs and every
/// app picker).
pub const SANDBOX_IMAGES_JSON: &str =
    include_str!("../../../../../libs/images/sandbox-images.json");

/// One entry of `sandbox-images.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageEntry {
    pub r#ref: String,
    pub group: String,
    pub os: String,
    pub name: String,
    pub variant: String,
    pub summary: String,
    pub spacesd: bool,
    /// Local runtime: `container`, `qemu` or `lume`; `None`: cloud only.
    pub local: Option<String>,
    /// Cloud runtime: `gvisor` or `kubevirt`; `None`: local only.
    pub cloud: Option<String>,
    pub published: bool,
}

#[derive(Deserialize)]
struct ImageList {
    groups: Vec<ImageGroup>,
    images: Vec<ImageEntry>,
}

#[derive(Deserialize)]
struct ImageGroup {
    id: String,
    /// `false` keeps the group out of pickers (benchmark images).
    #[serde(default = "offered")]
    picker: bool,
}

fn offered() -> bool {
    true
}

/// The `published: true` entries of groups offered in pickers, in file
/// order (what the wizard offers).
pub fn published_images() -> Vec<ImageEntry> {
    let list: ImageList =
        serde_json::from_str(SANDBOX_IMAGES_JSON).expect("sandbox-images.json parses");
    let offered: Vec<String> = list
        .groups
        .into_iter()
        .filter(|g| g.picker)
        .map(|g| g.id)
        .collect();
    list.images
        .into_iter()
        .filter(|i| i.published && offered.contains(&i.group))
        .collect()
}

/// Where the Space runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    /// Cua Cloud (metered).
    Cloud,
    /// This machine (containers, QEMU or Lume, orchestrated by the SDK).
    Local,
}

/// What the wizard's Create button sends.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpacePlan {
    /// An image `ref` from the shared list.
    pub image: String,
    pub target: Target,
    /// A DNS label (lowercase letters, digits, `-`, at most 63).
    pub name: String,
    /// Local only.
    #[serde(default)]
    pub cpus: Option<u32>,
    /// Local only (MiB).
    #[serde(default)]
    pub memory_mb: Option<u64>,
}

/// The SDK call a plan maps to: one [`SpaceCreate`].
pub type PlanCall = SpaceCreate;

/// Whether `name` is a DNS label (RFC 1123): 1 to 63 of `a-z0-9-`, not
/// starting or ending with `-`.
pub fn is_dns_label(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The kind of an entry (`variant`: container or vm).
pub fn entry_kind(entry: &ImageEntry) -> Kind {
    Kind::parse(&entry.variant).unwrap_or(Kind::Auto)
}

/// The local engine for an entry (`None`: it does not run locally). A
/// container is `auto` (gVisor when the engine has it).
pub fn local_runtime(entry: &ImageEntry) -> Option<Runtime> {
    match entry.local.as_deref()? {
        "container" => Some(Runtime::Auto),
        "qemu" => Some(Runtime::Qemu),
        "lume" => Some(Runtime::Lume),
        _ => None,
    }
}

/// The cloud engine for an entry (`None`: it does not run in the cloud).
pub fn cloud_runtime(entry: &ImageEntry) -> Option<Runtime> {
    match entry.cloud.as_deref()? {
        "gvisor" => Some(Runtime::Gvisor),
        "kubevirt" => Some(Runtime::Kubevirt),
        _ => None,
    }
}

/// Maps a plan onto one SDK call, against the published images.
pub fn plan_call(plan: &SpacePlan) -> Result<PlanCall> {
    plan_call_in(plan, &published_images())
}

/// [`plan_call`] against a given image list (tests).
pub fn plan_call_in(plan: &SpacePlan, images: &[ImageEntry]) -> Result<PlanCall> {
    let entry = images
        .iter()
        .find(|i| i.r#ref == plan.image && i.published)
        .ok_or_else(|| Error::Invalid(format!("{} is not a published image", plan.image)))?;
    if !is_dns_label(&plan.name) {
        return Err(Error::Invalid(format!(
            "`{}` is not a DNS label (a-z, 0-9 and -, at most 63)",
            plan.name
        )));
    }
    let (on, runtime, cpus, memory_mb) = match plan.target {
        Target::Local => (
            On::Local,
            local_runtime(entry).ok_or_else(|| {
                Error::Invalid(format!("{} does not run on this machine", entry.name))
            })?,
            Some(plan.cpus.unwrap_or(2).clamp(1, 64)),
            Some(plan.memory_mb.unwrap_or(4096).clamp(1024, 262_144)),
        ),
        Target::Cloud => (
            On::Cloud,
            cloud_runtime(entry).ok_or_else(|| {
                Error::Invalid(format!("{} does not run in the cloud", entry.name))
            })?,
            None,
            None,
        ),
    };
    Ok(SpaceCreate {
        image: Some(entry.r#ref.clone()),
        on: Some(on),
        kind: entry_kind(entry),
        runtime,
        name: Some(plan.name.clone()),
        cpus,
        memory_mb,
        wait: Some(true),
        spacesd: Some(entry.spacesd),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(image: &str, target: Target) -> SpacePlan {
        SpacePlan {
            image: image.into(),
            target,
            name: "koala-desk".into(),
            cpus: Some(4),
            memory_mb: Some(8192),
        }
    }

    #[test]
    fn published_images_are_the_published_entries_in_order() {
        let all: serde_json::Value = serde_json::from_str(SANDBOX_IMAGES_JSON).unwrap();
        let want: Vec<String> = all["images"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|i| i["published"] == true)
            .map(|i| i["ref"].as_str().unwrap().to_string())
            .collect();
        let got: Vec<String> = published_images().into_iter().map(|i| i.r#ref).collect();
        assert_eq!(got, want);
        assert!(!got.is_empty());
    }

    #[test]
    fn a_local_container_plan_is_one_create_on_this_machine() {
        let p = plan_call(&plan("ghcr.io/trycua/linux:24.04", Target::Local)).unwrap();
        assert_eq!(p.on, Some(On::Local));
        assert_eq!(p.image.as_deref(), Some("ghcr.io/trycua/linux:24.04"));
        assert_eq!(
            (p.kind, p.runtime.clone()),
            (Kind::Container, Runtime::Auto)
        );
        assert_eq!(p.name.as_deref(), Some("koala-desk"));
        assert_eq!(p.cpus, Some(4));
        assert_eq!(p.memory_mb, Some(8192));
        assert_eq!(p.spacesd, Some(true));
    }

    #[test]
    fn local_vm_and_lume_images_name_their_engine() {
        let vm = plan_call(&plan("ghcr.io/trycua/linux:24.04-disk", Target::Local)).unwrap();
        assert_eq!((vm.kind, vm.runtime.clone()), (Kind::Vm, Runtime::Qemu));
        assert_eq!(vm.image.as_deref(), Some("ghcr.io/trycua/linux:24.04-disk"));
        let mac = plan_call(&plan("ghcr.io/trycua/macos:26", Target::Local)).unwrap();
        assert_eq!((mac.kind, mac.runtime.clone()), (Kind::Vm, Runtime::Lume));
    }

    #[test]
    fn spacesd_comes_from_the_catalog_entry() {
        for entry in published_images() {
            let target = if entry.local.is_some() {
                Target::Local
            } else {
                Target::Cloud
            };
            let call = plan_call(&plan(&entry.r#ref, target)).unwrap();
            assert_eq!(call.spacesd, Some(entry.spacesd), "{}", entry.r#ref);
        }
        // macos:26 ships cua-spacesd (catalog `spacesd: true`).
        let mac = plan_call(&plan("ghcr.io/trycua/macos:26", Target::Local)).unwrap();
        assert_eq!(mac.spacesd, Some(true));
        // The entry decides, not the OS: a list that says otherwise wins.
        let images: Vec<ImageEntry> = published_images()
            .into_iter()
            .map(|mut i| {
                if i.r#ref == "ghcr.io/trycua/macos:26" {
                    i.spacesd = false;
                }
                i
            })
            .collect();
        let mac = plan_call_in(&plan("ghcr.io/trycua/macos:26", Target::Local), &images).unwrap();
        assert_eq!(mac.spacesd, Some(false));
    }

    #[test]
    fn cloud_plans_create_in_the_cloud_with_the_entry_engine() {
        let c = plan_call(&plan("ghcr.io/trycua/linux:24.04", Target::Cloud)).unwrap();
        assert_eq!(c.on, Some(On::Cloud));
        assert_eq!(c.image.as_deref(), Some("ghcr.io/trycua/linux:24.04"));
        assert_eq!(c.runtime, Runtime::Gvisor);
        assert_eq!(c.name.as_deref(), Some("koala-desk"));
        assert_eq!(c.wait, Some(true));
        assert_eq!((c.cpus, c.memory_mb), (None, None));
        let w = plan_call(&plan("ghcr.io/trycua/windows:2022", Target::Cloud)).unwrap();
        assert_eq!((w.kind, w.runtime.clone()), (Kind::Vm, Runtime::Kubevirt));
        // Whether it runs cua-spacesd is the catalog's to say (it changes as
        // images ship it).
        let entry = published_images()
            .into_iter()
            .find(|i| i.r#ref == "ghcr.io/trycua/windows:2022")
            .unwrap();
        assert_eq!(w.spacesd, Some(entry.spacesd));
    }

    #[test]
    fn unsupported_targets_unpublished_images_and_bad_names_are_refused() {
        let e = plan_call(&plan("ghcr.io/trycua/macos:26", Target::Cloud)).unwrap_err();
        assert!(e.to_string().contains("does not run in the cloud"), "{e}");
        let e = plan_call(&plan("ghcr.io/trycua/bench-web:1.0", Target::Local)).unwrap_err();
        assert!(e.to_string().contains("not a published image"), "{e}");
        let mut p = plan("ghcr.io/trycua/linux:24.04", Target::Local);
        p.name = "Koala Desk".into();
        assert!(plan_call(&p).is_err());
    }

    #[test]
    fn dns_labels() {
        assert!(is_dns_label("koala-1"));
        assert!(!is_dns_label(""));
        assert!(!is_dns_label("-a"));
        assert!(!is_dns_label("a-"));
        assert!(!is_dns_label("A"));
        assert!(!is_dns_label(&"a".repeat(64)));
    }

    #[test]
    fn resources_default_and_clamp() {
        let mut p = plan("ghcr.io/trycua/linux:24.04", Target::Local);
        p.cpus = None;
        p.memory_mb = Some(1);
        let l = plan_call(&p).unwrap();
        assert_eq!(l.cpus, Some(2));
        assert_eq!(l.memory_mb, Some(1024));
    }
}
