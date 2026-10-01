// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! New Space: the image catalog, which placements, kinds and runtimes an
//! image offers, and the step-by-step wizard (System, Resources, Options,
//! Summary) with its "Connect by address" form.
//!
//! "Run on" is one menu of every machine that can host the Space
//! ([`placement_options`]): This Mac, your machines that provide Spaces
//! (relay and direct hosts, from the SDK's `Spaces.hosts()`; disabled with
//! why while offline or at a limit), and, with the "Your cloud" experiment,
//! your connected clouds ("AWS · us-west-2"). Choosing one sets the
//! create's `on`: `local`, `host:<machine>` or the cloud's word.
//!
//! The images come from ONE file, `libs/images/sandbox-images.json`, which
//! the docs generator also renders, so the apps and the docs cannot drift.
//! Only `published` entries are offered.

use crate::model::{Location, Runtime, SpaceKind, SpaceOs};
use crate::spaces::sidebar::Fact;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

const IMAGES_JSON: &str = include_str!("../../../../images/sandbox-images.json");

/// The image list's local engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LocalEngine {
    /// A container (gVisor when installed, else runc).
    Container,
    /// A QEMU VM.
    Qemu,
    /// A Lume VM.
    Lume,
}

impl LocalEngine {
    /// Tile description.
    pub fn label(self) -> &'static str {
        match self {
            LocalEngine::Container => "Container on this Mac (gVisor when installed)",
            LocalEngine::Qemu => "QEMU virtual machine on this Mac",
            LocalEngine::Lume => "Lume virtual machine (Apple silicon)",
        }
    }

    /// The `local_status().backends` that run it.
    pub fn backends(self) -> &'static [&'static str] {
        match self {
            LocalEngine::Container => &["container", "managed", "docker", "runsc"],
            LocalEngine::Qemu => &["qemu", "qemudocker"],
            LocalEngine::Lume => &["lume"],
        }
    }
}

/// The image list's cloud engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloudEngine {
    /// gVisor containers.
    Gvisor,
    /// KubeVirt VMs.
    Kubevirt,
}

impl CloudEngine {
    /// Tile description.
    pub fn label(self) -> &'static str {
        match self {
            CloudEngine::Gvisor => "gVisor container in Cua Cloud",
            CloudEngine::Kubevirt => "Virtual machine in Cua Cloud",
        }
    }

    fn runtime(self) -> Runtime {
        match self {
            CloudEngine::Gvisor => Runtime::Gvisor,
            CloudEngine::Kubevirt => Runtime::Kubevirt,
        }
    }
}

/// The catalog's image tier (canonical images only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageTier {
    /// The minimum that passes `cua-spacesd doctor --strict` (what CI runs).
    Slim,
    /// The default: slim plus dev tooling.
    Full,
    /// macOS: full plus one pinned Xcode.
    Xcode,
}

/// One image of the catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxImage {
    /// OCI ref.
    #[serde(rename = "ref")]
    pub image_ref: String,
    /// Group id.
    pub group: String,
    /// OS.
    pub os: SpaceOs,
    /// Display name.
    pub name: String,
    /// Container or VM.
    pub variant: SpaceKind,
    /// One line.
    pub summary: String,
    /// Runs cua-spacesd (streams).
    pub spacesd: bool,
    /// Local engine.
    pub local: Option<LocalEngine>,
    /// Cloud engine.
    pub cloud: Option<CloudEngine>,
    /// `slim`, `full` or `xcode` (canonical images only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<ImageTier>,
    /// Offered in pickers.
    pub published: bool,
    /// The guest's distribution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distro: Option<ImageDistro>,
    /// Platforms it is built for (`amd64`, `arm64`); empty when unknown.
    #[serde(default)]
    pub arch: Vec<String>,
    /// How big it is per platform, when measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sizes: Option<ImageSizes>,
}

/// The guest distribution a catalog image names: its id picks the OS icon
/// and its name is the System fact until the Space reports its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageDistro {
    /// `ubuntu`, `omarchy`, `macos`, `windows`.
    pub id: String,
    /// Pretty name ("Ubuntu 24.04", "Omarchy").
    pub name: String,
}

/// An image's sizes per platform, measured from its published manifests
/// at `digest` (`scripts/images/record-image-sizes.py`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageSizes {
    /// The digest they were measured at.
    pub digest: String,
    /// One row per platform.
    pub platforms: Vec<PlatformSize>,
}

/// One platform's sizes, in bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformSize {
    /// `amd64` or `arm64`.
    pub arch: String,
    /// The platform manifest's digest.
    pub manifest: String,
    /// What a pull downloads.
    pub download: u64,
    /// What the pulled image takes on the host.
    pub unpacked: u64,
    /// The disk the Space sees (a VM's virtual disk; a container's root
    /// filesystem).
    pub disk: u64,
}

/// A titled group of images.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageGroup {
    /// Group id.
    pub id: String,
    /// Title.
    pub label: String,
    /// Published images.
    pub images: Vec<SandboxImage>,
}

#[derive(Deserialize)]
struct ImageFile {
    groups: Vec<GroupEntry>,
    images: Vec<SandboxImage>,
}

#[derive(Deserialize)]
struct GroupEntry {
    id: String,
    label: String,
    /// `false` keeps the group out of the pickers (benchmark images).
    #[serde(default = "offered")]
    picker: bool,
}

fn offered() -> bool {
    true
}

struct Catalog {
    /// Every entry, published or not (to name unpublished refs).
    all: Vec<SandboxImage>,
    images: Vec<SandboxImage>,
    groups: Vec<ImageGroup>,
}

fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let file: ImageFile =
            serde_json::from_str(IMAGES_JSON).expect("libs/images/sandbox-images.json parses");
        let all = file.images;
        let offered: Vec<&str> = file
            .groups
            .iter()
            .filter(|g| g.picker)
            .map(|g| g.id.as_str())
            .collect();
        let images: Vec<SandboxImage> = all
            .iter()
            .filter(|i| i.published && offered.contains(&i.group.as_str()))
            .cloned()
            .collect();
        let groups = file
            .groups
            .into_iter()
            .filter(|g| g.picker)
            .map(|g| ImageGroup {
                images: images.iter().filter(|i| i.group == g.id).cloned().collect(),
                id: g.id,
                label: g.label,
            })
            .filter(|g| !g.images.is_empty())
            .collect();
        Catalog {
            all,
            images,
            groups,
        }
    })
}

/// Every image the pickers show, in file order.
pub fn picker_images() -> Vec<SandboxImage> {
    catalog().images.clone()
}

/// The picker images grouped as the file groups them.
pub fn picker_groups() -> Vec<ImageGroup> {
    catalog().groups.clone()
}

/// A published image by ref.
pub fn find_image(image_ref: &str) -> Option<SandboxImage> {
    catalog()
        .images
        .iter()
        .find(|i| i.image_ref == image_ref)
        .cloned()
}

/// `(repository, tag, digest)` of an image reference.
fn split_ref(reference: &str) -> (&str, Option<&str>, Option<&str>) {
    let (name, digest) = match reference.trim().split_once('@') {
        Some((n, d)) => (n, Some(d)),
        None => (reference.trim(), None),
    };
    match name.rsplit_once(':') {
        Some((repo, tag)) if !tag.contains('/') => (repo, Some(tag), digest),
        _ => (name, None, digest),
    }
}

/// What the catalog says about `image_ref` through `fact`: the entry with
/// that exact ref (published or not), else the entries of its repository
/// the ref pins (a digest: all of them; a dated pin such as
/// `24.04-slim-20260926-abc1234`: the tags it extends) when they agree.
/// `None` for images the catalog does not list, or another version of a
/// listed one.
fn catalog_fact<T: PartialEq>(image_ref: &str, fact: impl Fn(&SandboxImage) -> T) -> Option<T> {
    let all = &catalog().all;
    if let Some(i) = all.iter().find(|i| i.image_ref == image_ref.trim()) {
        return Some(fact(i));
    }
    let (repo, tag, _) = split_ref(image_ref);
    let mut candidates = all.iter().filter(|i| {
        let (r, t, _) = split_ref(&i.image_ref);
        r == repo
            && match (tag, t) {
                (None, _) => true,
                (Some(tag), Some(listed)) => tag.starts_with(&format!("{listed}-")),
                (Some(_), None) => false,
            }
    });
    let first = fact(candidates.next()?);
    candidates.all(|i| fact(i) == first).then_some(first)
}

/// The distribution the catalog names for `image_ref` (see
/// [`catalog_fact`]).
pub fn image_distro(image_ref: &str) -> Option<ImageDistro> {
    catalog_fact(image_ref, |i| i.distro.clone()).flatten()
}

/// Container or VM, as the catalog lists `image_ref`.
pub fn image_kind(image_ref: &str) -> Option<SpaceKind> {
    catalog_fact(image_ref, |i| i.variant)
}

/// The platforms the catalog lists for `image_ref` (empty when unknown).
pub fn image_arch(image_ref: &str) -> Vec<String> {
    catalog_fact(image_ref, |i| i.arch.clone()).unwrap_or_default()
}

/// The image field's placeholder: the shape of a reference.
pub const IMAGE_PLACEHOLDER: &str = "ghcr.io/org/app:tag";

fn path_component_ok(c: &str) -> bool {
    // `[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*`
    let b = c.as_bytes();
    let alnum = |x: u8| x.is_ascii_lowercase() || x.is_ascii_digit();
    if b.is_empty() || !alnum(b[0]) || !alnum(b[b.len() - 1]) {
        return false;
    }
    let mut i = 0;
    while i < b.len() {
        if alnum(b[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && !alnum(b[i]) {
            i += 1;
        }
        let sep = &c[start..i];
        let ok = sep == "." || sep == "_" || sep == "__" || sep.bytes().all(|x| x == b'-');
        if !ok {
            return false;
        }
    }
    true
}

fn domain_ok(d: &str) -> bool {
    let (host, port) = match d.rsplit_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (d, None),
    };
    !host.is_empty()
        && host.split('.').all(|l| {
            !l.is_empty()
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes().all(|x| x.is_ascii_alphanumeric() || x == b'-')
        })
        && port.is_none_or(|p| (1..=5).contains(&p.len()) && p.bytes().all(|x| x.is_ascii_digit()))
}

fn tag_ok(t: &str) -> bool {
    let b = t.as_bytes();
    (1..=128).contains(&b.len())
        && (b[0].is_ascii_alphanumeric() || b[0] == b'_')
        && b.iter()
            .all(|&x| x.is_ascii_alphanumeric() || x == b'_' || x == b'.' || x == b'-')
}

fn digest_ok(d: &str) -> bool {
    let Some((alg, hex)) = d.split_once(':') else {
        return false;
    };
    !alg.is_empty()
        && alg
            .bytes()
            .all(|x| x.is_ascii_lowercase() || x.is_ascii_digit() || b"+._-".contains(&x))
        && hex.len() >= 32
        && hex.bytes().all(|x| x.is_ascii_hexdigit())
}

/// Checks an image reference the person typed. `None` when it can be
/// created (a preset, or any well-formed `[registry/]name[:tag][@digest]`
/// the catalog does not list); otherwise one line saying what is wrong.
pub fn validate_image_ref(text: &str) -> Option<String> {
    let r = text.trim();
    let malformed = || Some(format!("Not an image reference, like {IMAGE_PLACEHOLDER}."));
    if r.is_empty() {
        return Some(format!("Enter an image, like {IMAGE_PLACEHOLDER}."));
    }
    if r.chars().any(char::is_whitespace) {
        return Some("An image reference has no spaces.".into());
    }
    if let Some(e) = catalog().all.iter().find(|i| i.image_ref == r) {
        return (!e.published).then(|| "This image is not published yet.".into());
    }
    let (name_tag, digest) = match r.split_once('@') {
        Some((n, d)) => (n, Some(d)),
        None => (r, None),
    };
    if digest.is_some_and(|d| !digest_ok(d)) {
        return malformed();
    }
    let slash = name_tag.rfind('/').map_or(0, |i| i + 1);
    let (name, tag) = match name_tag[slash..].rfind(':') {
        Some(i) => (&name_tag[..slash + i], Some(&name_tag[slash + i + 1..])),
        None => (name_tag, None),
    };
    if tag.is_some_and(|t| !tag_ok(t)) || name.is_empty() || name.len() > 255 {
        return malformed();
    }
    let mut parts: Vec<&str> = name.split('/').collect();
    if parts.len() > 1 {
        let first = parts[0];
        if first.contains('.') || first.contains(':') || first == "localhost" {
            if !domain_ok(first) {
                return malformed();
            }
            parts.remove(0);
        }
    }
    if parts
        .iter()
        .any(|c| c.bytes().any(|x| x.is_ascii_uppercase()))
    {
        return Some("Use lowercase letters in the image name.".into());
    }
    if !parts.iter().all(|c| path_component_ok(c)) {
        return malformed();
    }
    None
}

/// The presets matching what is typed, grouped as the file groups them.
/// Every whitespace-separated word must appear (any case) in the ref or
/// the name; an empty query matches everything.
pub fn image_suggestions(query: &str) -> Vec<ImageGroup> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    catalog()
        .groups
        .iter()
        .map(|g| ImageGroup {
            id: g.id.clone(),
            label: g.label.clone(),
            images: g
                .images
                .iter()
                .filter(|i| {
                    let hay = format!("{} {}", i.image_ref, i.name).to_lowercase();
                    words.iter().all(|w| hay.contains(w.as_str()))
                })
                .cloned()
                .collect(),
        })
        .filter(|g| !g.images.is_empty())
        .collect()
}

/// Whether `image` can run in `on`.
pub fn can_place(image: &SandboxImage, on: Location) -> bool {
    match on {
        Location::Cloud => crate::model::CLOUD_SPACES_OFFERED && image.cloud.is_some(),
        Location::Local => image.local.is_some(),
        // Whether the connected cloud runs it is its own answer
        // ([`cloud_offer`]); every image is a candidate. A machine of
        // yours runs it with its own runtimes ([`host_refusal`]).
        Location::Yours | Location::Host => true,
    }
}

/// What "Your cloud" says while no cloud is connected.
pub const YOUR_CLOUD_CONNECT_DETAIL: &str = "Connect AWS, Google Cloud or Modal.";

/// The link that opens the "Connect a cloud" sheet.
pub const CONNECT_CLOUD_LABEL: &str = "Connect a cloud\u{2026}";

/// What one connected cloud can run for an image family, and at what cost
/// (a `cloud_status` provider's `kinds` row).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudOffer {
    /// The image family: `linux`, `linux-slim`, `linux-vm`, `windows`,
    /// `omarchy`, `macos`.
    pub image: String,
    /// `container` or `vm`.
    #[serde(default)]
    pub kind: String,
    /// Whether it can run there.
    pub supported: bool,
    /// Why not (one line), or a note.
    #[serde(default)]
    pub reason: String,
    /// The machine type the cloud picks (`t4g.medium`).
    #[serde(default)]
    pub machine_type: String,
    /// Estimated US dollars per hour while it runs.
    #[serde(default)]
    pub usd_per_hour: f64,
}

/// A connected cloud (a `cloud_status` provider with `connected`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectedCloud {
    /// The SDK's `on` word: `aws`, `gcp`, `modal`.
    pub name: String,
    /// `AWS`, `Google Cloud`, `Modal`.
    pub title: String,
    /// "AWS \u{b7} us-west-2".
    pub label: String,
    /// It is the default location (`default.on`).
    #[serde(default)]
    pub is_default: bool,
    /// Hours after which a Space there deletes itself (0: never).
    #[serde(default)]
    pub ttl_hours: u32,
    /// What it runs.
    #[serde(default)]
    pub offers: Vec<CloudOffer>,
}

/// One limit of a host that provides Spaces and how much of it is used
/// (`HostSpacesCapacity`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostLimit {
    /// `spaces` (every Space it provides) or `macos_vms` (macOS VMs on that
    /// Mac).
    pub resource: String,
    /// In use now.
    pub used: u32,
    /// The limit (0: none).
    pub limit: u32,
    /// Why the limit exists, for people ("Apple's macOS license allows two
    /// macOS VMs per Mac").
    #[serde(default)]
    pub reason: String,
}

/// One of the user's machines that provides Spaces (the SDK's
/// `Spaces.hosts()`): a relay host, or one added by its Tailscale or LAN
/// address. New Space can create on it (`on="host:<id>"`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceHost {
    /// What `on="host:<id>"` takes: the relay machine id, or the name a
    /// direct host was added as.
    pub id: String,
    /// Its name ("Mac mini").
    pub name: String,
    /// `relay` or `direct` (its Tailscale or LAN address).
    pub via: String,
    /// It answered just now.
    pub online: bool,
    /// Its operating system (`macos`, `linux`, `windows`), when it answered.
    #[serde(default)]
    pub os: String,
    /// Its limits, when it answered.
    #[serde(default)]
    pub limits: Vec<HostLimit>,
}

/// Why `host` cannot create a Space of `image` now, as a short word for
/// the menu and one line: `(word, line)`. `None` when it can.
pub fn host_refusal(image: &SandboxImage, host: &SpaceHost) -> Option<(&'static str, String)> {
    if !host.online {
        return Some(("offline", format!("{} is offline.", host.name)));
    }
    if image.os == SpaceOs::Macos && !host.os.is_empty() && host.os != "macos" {
        return Some((
            "not a Mac",
            format!("{} cannot run macOS: it is not a Mac.", host.name),
        ));
    }
    let full = host.limits.iter().find(|l| {
        l.limit > 0
            && l.used >= l.limit
            && (l.resource == "spaces" || (l.resource == "macos_vms" && image.os == SpaceOs::Macos))
    })?;
    let why = full.reason.trim().trim_end_matches('.');
    Some((
        "at its limit",
        if why.is_empty() {
            format!(
                "{} is at its limit ({} of {}).",
                host.name, full.used, full.limit
            )
        } else {
            format!("{} is at its limit: {why}.", host.name)
        },
    ))
}

/// One entry of the "Run on" menu: a machine that can host the Space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlacementOption {
    /// What choosing it sets as the create's `on`: `local`, `cloud`,
    /// `host:<machine>`, or a connected cloud's word (`aws`).
    pub id: String,
    /// The menu's text: "This Mac", "Mac mini", "Mac mini (offline)",
    /// "AWS · us-west-2".
    pub label: String,
    /// Its group, in menu order (a separator between groups): `this-mac`,
    /// `hosts`, `clouds`.
    pub group: String,
    /// Chosen.
    pub selected: bool,
    /// It can be chosen now (offline, at its limit: not).
    pub enabled: bool,
    /// One line: why not, or what it runs (the tooltip).
    pub detail: String,
}

/// The image family a cloud prices (`linux`, `linux-slim`, `linux-vm`,
/// `windows`, `omarchy`, `macos`).
pub fn cloud_family(image: &SandboxImage) -> &'static str {
    let r = image.image_ref.to_ascii_lowercase();
    match image.os {
        SpaceOs::Macos => "macos",
        SpaceOs::Windows => "windows",
        SpaceOs::Linux if r.contains("omarchy") => "omarchy",
        SpaceOs::Linux if image.variant == SpaceKind::Vm => "linux-vm",
        SpaceOs::Linux if r.contains("-slim") => "linux-slim",
        SpaceOs::Linux => "linux",
    }
}

/// What `cloud` offers for `image`, when it prices its family.
pub fn cloud_offer<'a>(image: &SandboxImage, cloud: &'a ConnectedCloud) -> Option<&'a CloudOffer> {
    let family = cloud_family(image);
    cloud.offers.iter().find(|o| o.image == family)
}

/// Why `cloud` cannot run `image` (one line), or `None` when it can.
/// Ends with the way around it: an agent can deploy the image on that cloud
/// itself and add the running machine as a Space.
pub fn cloud_refusal(image: &SandboxImage, cloud: &ConnectedCloud) -> Option<String> {
    let why = match cloud_offer(image, cloud) {
        Some(o) if o.supported => return None,
        Some(o) if !o.reason.is_empty() => o.reason.trim_end_matches('.').to_string(),
        _ => format!("{} does not run this image", cloud.title),
    };
    Some(format!(
        "{why}. Ask your agent to deploy the {} image on {} and add it with cua spaces add.",
        image.name, cloud.title
    ))
}

/// `About $0.03/hour` for a cloud's hourly estimate (`None`: unknown).
pub fn cloud_price_text(usd_per_hour: f64) -> Option<String> {
    if !usd_per_hour.is_finite() || usd_per_hour <= 0.0 {
        return None;
    }
    Some(if usd_per_hour < 0.005 {
        "Under $0.01/hour".to_string()
    } else {
        format!("About ${usd_per_hour:.2}/hour")
    })
}

/// When a Space in a cloud deletes itself: `After 8 hours`, `Never`.
pub fn ttl_text(ttl_hours: u32) -> String {
    match ttl_hours {
        0 => "Never".into(),
        1 => "After 1 hour".into(),
        h => format!("After {h} hours"),
    }
}

/// The connected clouds the wizard offers: none while the "Your cloud"
/// experiment is off (they stay connected; the wizard does not show them).
pub fn offered_clouds(env: &WizardEnv) -> &[ConnectedCloud] {
    if env.experiments.your_cloud {
        &env.clouds
    } else {
        &[]
    }
}

/// The connected cloud the wizard creates in: the chosen one, else the
/// default, else the first.
pub fn selected_cloud<'a>(state: &WizardState, env: &'a WizardEnv) -> Option<&'a ConnectedCloud> {
    let clouds = offered_clouds(env);
    state
        .cloud
        .as_deref()
        .and_then(|n| clouds.iter().find(|c| c.name == n))
        .or_else(|| clouds.iter().find(|c| c.is_default))
        .or_else(|| clouds.first())
}

/// The machine the wizard creates on (`Location::Host`): the chosen one,
/// while it is still listed.
pub fn selected_host<'a>(state: &WizardState, env: &'a WizardEnv) -> Option<&'a SpaceHost> {
    let id = state.host.as_deref()?;
    env.hosts.iter().find(|h| h.id == id)
}

/// The engines `image` runs on in `on`, the default first.
pub fn runtime_options(image: &SandboxImage, on: Location) -> Vec<Runtime> {
    match on {
        Location::Cloud => image.cloud.map(|c| vec![c.runtime()]).unwrap_or_default(),
        Location::Local => match image.local {
            Some(LocalEngine::Container) => vec![Runtime::Auto, Runtime::Gvisor, Runtime::Runc],
            Some(LocalEngine::Qemu) => vec![Runtime::Auto, Runtime::Qemu],
            Some(LocalEngine::Lume) => vec![Runtime::Auto, Runtime::Lume],
            None => vec![],
        },
        // The cloud picks the machine and the engine; a machine of yours
        // picks its engine.
        Location::Yours | Location::Host => vec![Runtime::Auto],
    }
}

fn family_of(image: &SandboxImage) -> &str {
    image
        .image_ref
        .strip_suffix("-disk")
        .unwrap_or(&image.image_ref)
}

/// A kind the image's family offers in a location, as the image offering it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KindOption {
    /// Container or VM.
    pub kind: SpaceKind,
    /// The image with that variant.
    pub image: SandboxImage,
}

/// The kinds `image` comes in that can run in `on` (`linux:24.04` and its
/// `-disk` VM variant are one family).
pub fn kind_options(image: &SandboxImage, on: Location) -> Vec<KindOption> {
    let family = family_of(image);
    [SpaceKind::Container, SpaceKind::Vm]
        .into_iter()
        .filter_map(|kind| {
            catalog()
                .images
                .iter()
                .find(|i| family_of(i) == family && i.variant == kind && can_place(i, on))
                .map(|i| KindOption {
                    kind,
                    image: i.clone(),
                })
        })
        .collect()
}

/// Can this Mac run `image` locally, given the ready backends?
pub fn local_runtime_ready(image: &SandboxImage, backends: &[String]) -> bool {
    image
        .local
        .is_some_and(|e| e.backends().iter().any(|b| backends.iter().any(|x| x == b)))
}

/// Why the image's local engine is not ready here, from
/// [`WizardEnv::local_details`]: the first detail of its backends, without
/// the doctor's setup plan and trailing period.
fn local_engine_detail(image: &SandboxImage, env: &WizardEnv) -> Option<String> {
    let details = env.local_details.as_ref()?;
    let engine = image.local?;
    engine.backends().iter().find_map(|b| {
        let d = details.get(*b)?;
        let d = d.split("; setup would:").next().unwrap_or(d).trim();
        let d = d.trim_end_matches('.').trim();
        (!d.is_empty()).then(|| d.to_string())
    })
}

/// `host:port` or `http(s)://host:port`: a shape check only.
pub fn looks_like_address(value: &str) -> bool {
    let v = value.trim();
    if v.is_empty() || v.chars().any(char::is_whitespace) {
        return false;
    }
    let v = v
        .strip_prefix("http://")
        .or_else(|| v.strip_prefix("https://"))
        .unwrap_or(v);
    let v = v.strip_suffix('/').unwrap_or(v);
    let (host, port) = if let Some(rest) = v.strip_prefix('[') {
        let Some(end) = rest.find(']') else {
            return false;
        };
        let inner = &rest[..end];
        if inner.is_empty() || !inner.bytes().all(|b| b.is_ascii_hexdigit() || b == b':') {
            return false;
        }
        (None, &rest[end + 1..])
    } else {
        match v.find(':') {
            Some(i) => (Some(&v[..i]), &v[i..]),
            None => (Some(v), ""),
        }
    };
    if let Some(host) = host
        && (host.is_empty()
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-'))
    {
        return false;
    }
    if port.is_empty() {
        return true;
    }
    let Some(digits) = port.strip_prefix(':') else {
        return false;
    };
    (1..=5).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit())
}

/// The SDK's handshake errors in words a person can act on (the raw detail
/// stays in parentheses for support).
pub fn friendly_add_error(raw: &str) -> String {
    let l = raw.to_lowercase();
    if [
        "unauthenticated",
        "invalid bearer token",
        "permission denied",
    ]
    .iter()
    .any(|k| l.contains(k))
    {
        return format!(
            "The Space rejected the token. Check the spacesd token (CUA_ENV_TOKEN). ({raw})"
        );
    }
    if [
        "connection refused",
        "not available",
        "unreachable",
        "timed out",
        "dns",
    ]
    .iter()
    .any(|k| l.contains(k))
    {
        return format!("Could not reach cua-spacesd at that address. ({raw})");
    }
    raw.to_string()
}

/// DNS-label rule for Space names.
pub fn valid_space_name(name: &str) -> bool {
    let b = name.as_bytes();
    let ok = |c: u8| c.is_ascii_lowercase() || c.is_ascii_digit();
    !b.is_empty() && ok(b[0]) && ok(b[b.len() - 1]) && b.iter().all(|&c| ok(c) || c == b'-')
}

fn default_cpus(image: &SandboxImage) -> u32 {
    if image.os == SpaceOs::Linux { 2 } else { 4 }
}

fn default_memory_gb(image: &SandboxImage) -> u32 {
    if image.os == SpaceOs::Linux { 4 } else { 8 }
}

/// Step titles.
pub const STEPS: [&str; 4] = ["System", "Resources", "Options", "Summary"];
/// Memory slider bounds, GB (local).
pub const MEMORY_GB_RANGE: (u32, u32) = (2, 16);
/// vCPU slider bounds for Cua Cloud: the everyday range,
/// `cua_fleet::CLOUD_DEFAULT_RANGE_CPUS` (a cua-sdk test keeps them equal).
/// The SDK accepts more (up to 64); Fleet decides what an account may run.
pub const CLOUD_CPU_RANGE: (u32, u32) = (1, 8);
/// Memory slider bounds for Cua Cloud, GB: the everyday range,
/// `cua_fleet::CLOUD_DEFAULT_RANGE_MEMORY_MB` (1-32 GB); desktops start
/// at 2.
pub const CLOUD_MEMORY_GB_RANGE: (u32, u32) = (2, 32);

/// Cua Cloud rates for this account (the SDK's `Fleet.usage_pricing()`,
/// read from Fleet's `GET /api/config`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudPricing {
    /// USD per reserved vCPU per hour.
    pub vcpu_hour_usd: f64,
    /// USD per reserved GB (GiB) of memory per hour.
    pub memory_gib_hour_usd: f64,
}

/// `About $0.18/hour` for `cpus` vCPUs and `memory_gb` GB at `pricing`,
/// or `None` without usable rates (never a guess).
pub fn price_text(pricing: Option<&CloudPricing>, cpus: u32, memory_gb: u32) -> Option<String> {
    let p = pricing.filter(|p| {
        p.vcpu_hour_usd.is_finite()
            && p.memory_gib_hour_usd.is_finite()
            && p.vcpu_hour_usd > 0.0
            && p.memory_gib_hour_usd > 0.0
    })?;
    let usd = f64::from(cpus) * p.vcpu_hour_usd + f64::from(memory_gb) * p.memory_gib_hour_usd;
    Some(if usd < 0.005 {
        "Under $0.01/hour".to_string()
    } else {
        format!("About ${usd:.2}/hour")
    })
}

/// The sliders' bounds where the Space runs: `(cpus, memory GB)`.
fn size_ranges(placement: Location, env: &WizardEnv) -> ((u32, u32), (u32, u32)) {
    match placement {
        // A machine of yours: the everyday range (its own size is not known
        // here; it refuses what it cannot run).
        Location::Cloud | Location::Yours | Location::Host => {
            (CLOUD_CPU_RANGE, CLOUD_MEMORY_GB_RANGE)
        }
        Location::Local => ((1, env.max_cpus.max(1)), MEMORY_GB_RANGE),
    }
}

/// The size the Space gets: the chosen one, within where it runs.
fn effective_size(state: &WizardState, env: &WizardEnv) -> (u32, u32) {
    let ((c0, c1), (m0, m1)) = size_ranges(state.placement, env);
    (state.cpus.clamp(c0, c1), state.memory_gb.clamp(m0, m1))
}

/// What the shell knows about this machine and account.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WizardEnv {
    /// Where "Run on" starts (Settings, `cua config set default.on`).
    pub default_location: Location,
    /// Signed in (or client credentials).
    pub cloud_available: bool,
    /// A local container backend or Lume exists.
    pub local_available: bool,
    /// Why local is unavailable.
    #[serde(default)]
    pub local_reason: Option<String>,
    /// Ready local backends, when known.
    #[serde(default)]
    pub local_backends: Option<Vec<String>>,
    /// Why each local backend that is not ready is not (the runtime
    /// doctor's detail, by backend name), when known: the wizard names the
    /// reason instead of only "This Mac cannot run container Spaces."
    #[serde(default)]
    pub local_details: Option<std::collections::HashMap<String, String>>,
    /// Upper bound for the CPU slider.
    pub max_cpus: u32,
    /// This machine's architecture (`arm64`, `amd64`), when known.
    #[serde(default)]
    pub host_arch: Option<String>,
    /// Free space and pulled images here (the SDK's `Local.storage()`),
    /// when known.
    #[serde(default)]
    pub storage: Option<LocalStorage>,
    /// This account's Cua Cloud rates, when known (no estimate without).
    #[serde(default)]
    pub cloud_pricing: Option<CloudPricing>,
    /// The user's connected clouds (`cloud_status`), for "Your cloud"
    /// (offered only while its experiment is on: [`offered_clouds`]).
    #[serde(default)]
    pub clouds: Vec<ConnectedCloud>,
    /// The user's machines that provide Spaces (`Spaces.hosts()`).
    #[serde(default)]
    pub hosts: Vec<SpaceHost>,
    /// Settings, Experiments ("Your cloud" decides whether clouds show).
    #[serde(default)]
    pub experiments: crate::experiments::Experiments,
    /// The GPU option of each runtime that has one (the SDK's
    /// `gpu_support`), when known. A runtime not listed has none: the
    /// Resources step shows no GPU row for it.
    #[serde(default)]
    pub gpus: Option<Vec<GpuChoice>>,
}

/// A runtime's GPU option, as the wizard offers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuChoice {
    /// `lume`, `qemu`, `container`, `gvisor`, `fleet`.
    pub runtime: String,
    /// What the create passes as `gpu` (`paravirtual`).
    pub id: String,
    /// "GPU acceleration".
    pub label: String,
    /// Shown as "(Experimental)".
    pub experimental: bool,
    /// Works on this machine now.
    pub supported: bool,
    /// Why not, one line.
    #[serde(default)]
    pub reason: Option<String>,
    /// The page "Learn more" opens.
    #[serde(default)]
    pub learn_more: Option<String>,
}

/// The Resources step's GPU row: a checkbox, "(Experimental)", and a small
/// "Learn more"; disabled with the reason where this machine cannot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuRow {
    /// "GPU acceleration (Experimental)".
    pub label: String,
    /// Checked.
    pub on: bool,
    /// It can be checked here.
    pub enabled: bool,
    /// Why not, one line (a disabled row).
    pub reason: Option<String>,
    /// "Learn more".
    pub learn_more_label: String,
    /// The page it opens.
    pub learn_more_url: Option<String>,
}

/// Space on the volume a local engine writes to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageVolume {
    /// Bytes available.
    pub available_bytes: u64,
    /// Volume size in bytes.
    pub total_bytes: u64,
    /// `Macintosh HD`, `Colima`, `Docker Desktop`.
    pub name: String,
}

/// Where local Spaces are written and what is already pulled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalStorage {
    /// Bytes the SDK keeps free on a volume (a create that would leave
    /// less fails).
    pub reserve_bytes: u64,
    /// Lume's VM storage (macOS).
    pub lume: Option<StorageVolume>,
    /// The cua home (QEMU disks and the image cache).
    pub qemu: Option<StorageVolume>,
    /// The container engine's data disk.
    pub container: Option<StorageVolume>,
    /// Catalog refs already pulled here.
    pub pulled: Vec<String>,
}

/// Create or connect by address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WizardMode {
    /// The four steps.
    Create,
    /// Connect by address.
    Address,
}

/// The "Connect by address" form.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddressForm {
    /// `host:port`.
    pub url: String,
    /// spacesd token.
    pub token: String,
    /// Optional name.
    pub name: String,
    /// The handshake is running.
    pub busy: bool,
    /// The last error, in words.
    pub error: Option<String>,
}

/// The wizard's state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WizardState {
    /// Create or address.
    pub mode: WizardMode,
    /// 0..=3.
    pub step: u32,
    /// Where it runs.
    pub placement: Location,
    /// The person chose a placement (the default no longer follows Settings).
    pub picked: bool,
    /// The preset the Space is based on (its OS, kind and engines). With a
    /// custom ref typed, the Space runs that ref the same way.
    pub image_ref: String,
    /// The image field as typed: a preset's ref or any other reference.
    #[serde(default)]
    pub image_text: String,
    /// The suggestion list is open.
    #[serde(default)]
    pub suggest_open: bool,
    /// The list filters by the text (typing); opening it shows every preset.
    #[serde(default)]
    pub suggest_filter: bool,
    /// The highlighted row (keyboard), over the visible rows in order.
    #[serde(default)]
    pub suggest_index: Option<u32>,
    /// vCPUs as chosen (the view keeps them within where it runs).
    pub cpus: u32,
    /// Memory GB as chosen (the view keeps it within where it runs).
    pub memory_gb: u32,
    /// Disk GB as chosen (local VMs that can grow); `None`: the image's.
    #[serde(default)]
    pub disk_gb: Option<u32>,
    /// Name as typed.
    pub name: String,
    /// Open the desktop when ready.
    pub open_when_ready: bool,
    /// Advanced shown.
    pub advanced: bool,
    /// Runtime as chosen (the effective one is in the view).
    pub runtime_choice: Runtime,
    /// The connected cloud chosen for "Your cloud" (`aws`); `None`: the
    /// default one.
    #[serde(default)]
    pub cloud: Option<String>,
    /// The machine chosen for `Location::Host` ([`SpaceHost::id`]).
    #[serde(default)]
    pub host: Option<String>,
    /// The GPU box is checked (it applies where the effective runtime
    /// offers a GPU).
    #[serde(default)]
    pub gpu: bool,
    /// The address form.
    pub address: AddressForm,
}

/// An input to the wizard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum WizardAction {
    /// The configured default location arrived (ignored once picked).
    SyncDefault {
        /// Location.
        location: Location,
    },
    /// An OS tile.
    ChooseOs {
        /// OS.
        os: SpaceOs,
    },
    /// A preset picked from the suggestions (fills the field).
    ChooseImage {
        /// Image ref.
        #[serde(rename = "ref")]
        image_ref: String,
    },
    /// The image field changed (typing): filters the suggestions; a preset's
    /// ref selects it, anything else is used as typed.
    SetImageText {
        /// Text.
        text: String,
    },
    /// Show every preset (the field's menu button, or focus).
    OpenImageSuggestions,
    /// Up (-1) or Down (+1) in the suggestions; opens them when closed.
    MoveImageSuggestion {
        /// Rows to move.
        delta: i32,
    },
    /// Return: picks the highlighted preset, or keeps the typed text.
    PickImageSuggestion,
    /// Escape, or the field lost focus.
    DismissImageSuggestions,
    /// A "Run on" tile.
    SetPlacement {
        /// Location.
        placement: Location,
    },
    /// A connected cloud: creates there.
    ChooseCloud {
        /// The provider's word (`aws`).
        cloud: String,
    },
    /// An entry of the "Run on" menu ([`PlacementOption::id`]): `local`,
    /// `cloud`, `host:<machine>` or a connected cloud's word. A disabled
    /// entry (offline, at its limit) is ignored.
    ChoosePlacement {
        /// The entry's id.
        on: String,
    },
    /// Show or hide Advanced.
    ToggleAdvanced,
    /// A kind tile (switches to the family's image of that kind).
    ChooseKind {
        /// Kind.
        kind: SpaceKind,
    },
    /// The runtime menu.
    SetRuntime {
        /// Runtime.
        runtime: Runtime,
    },
    /// CPU slider.
    SetCpus {
        /// Cores.
        cpus: u32,
    },
    /// Memory slider.
    SetMemory {
        /// GB.
        #[serde(rename = "memoryGb")]
        memory_gb: u32,
    },
    /// Disk slider (VMs whose disk can grow).
    SetDisk {
        /// GB.
        #[serde(rename = "diskGb")]
        disk_gb: u32,
    },
    /// The disk slider's Reset: back to the image's own disk.
    ResetDisk,
    /// The GPU checkbox.
    SetGpu {
        /// Checked.
        on: bool,
    },
    /// Name field.
    SetName {
        /// Text.
        name: String,
    },
    /// "Open when ready".
    SetOpenWhenReady {
        /// On.
        on: bool,
    },
    /// Continue.
    Next,
    /// Back.
    Back,
    /// "Connect by address..."
    ShowAddress,
    /// Back to create.
    HideAddress,
    /// Address field.
    SetAddress {
        /// Text.
        url: String,
    },
    /// Token field.
    SetToken {
        /// Text.
        token: String,
    },
    /// Address name field.
    SetAddressName {
        /// Text.
        name: String,
    },
    /// "Add Space" pressed (the shell runs the handshake when the view's
    /// `address.canSubmit` was true).
    SubmitAddress,
    /// The handshake failed with this raw error.
    AddressFailed {
        /// Raw error.
        error: String,
    },
}

/// `location`, or this Mac when the apps do not offer it
/// ([`crate::model::CLOUD_SPACES_OFFERED`]), no cloud of the user's is
/// offered ([`offered_clouds`]), or it names no machine (a default is
/// never one of your machines).
fn offered_location(location: Location, env: &WizardEnv) -> Location {
    match location {
        Location::Cloud if !crate::model::CLOUD_SPACES_OFFERED => Location::Local,
        Location::Yours if offered_clouds(env).is_empty() => Location::Local,
        Location::Host => Location::Local,
        l => l,
    }
}

/// The first state.
pub fn initial(env: &WizardEnv) -> WizardState {
    let first = catalog()
        .images
        .first()
        .cloned()
        .expect("at least one published image");
    WizardState {
        mode: WizardMode::Create,
        step: 0,
        placement: offered_location(env.default_location, env),
        picked: false,
        cpus: default_cpus(&first),
        memory_gb: default_memory_gb(&first),
        disk_gb: None,
        image_text: first.image_ref.clone(),
        image_ref: first.image_ref,
        suggest_open: false,
        suggest_filter: false,
        suggest_index: None,
        name: String::new(),
        open_when_ready: true,
        advanced: false,
        runtime_choice: Runtime::Auto,
        cloud: None,
        host: None,
        gpu: false,
        address: AddressForm::default(),
    }
}

/// The preset the Space is based on.
fn base_of(state: &WizardState) -> SandboxImage {
    find_image(&state.image_ref)
        .or_else(|| catalog().images.first().cloned())
        .expect("at least one published image")
}

/// A typed ref that is not a preset.
fn custom_ref(state: &WizardState) -> Option<&str> {
    let t = state.image_text.trim();
    (find_image(t).is_none()).then_some(t)
}

/// The image the Space runs: the preset, or the typed ref run like it.
fn image_of(state: &WizardState) -> SandboxImage {
    let base = base_of(state);
    match custom_ref(state) {
        Some(r) => SandboxImage {
            image_ref: r.to_string(),
            group: "custom".into(),
            name: r.to_string(),
            summary: String::new(),
            tier: None,
            published: false,
            distro: None,
            arch: vec![],
            sizes: None,
            ..base
        },
        None => base,
    }
}

/// The OS whose tile is pressed: the preset's, until a custom ref is typed
/// (typing leaves the presets, so no OS is chosen).
fn selected_os(state: &WizardState) -> Option<SpaceOs> {
    custom_ref(state).is_none().then(|| base_of(state).os)
}

/// The suggestion rows as shown, in order. Typing filters every preset by
/// the text; otherwise the list shows the selected OS's presets (all of
/// them when a custom ref is in the field).
fn suggestion_rows(state: &WizardState) -> Vec<(String, SandboxImage)> {
    let (query, os) = if state.suggest_filter {
        (state.image_text.as_str(), None)
    } else {
        ("", selected_os(state))
    };
    image_suggestions(query)
        .into_iter()
        .flat_map(|g| {
            let label = g.label;
            g.images.into_iter().map(move |i| (label.clone(), i))
        })
        .filter(|(_, i)| os.is_none_or(|os| i.os == os))
        .collect()
}

fn open_suggestions(state: &mut WizardState) {
    state.suggest_open = true;
    state.suggest_filter = false;
    // Like a native combo box: the current preset starts highlighted.
    state.suggest_index = if custom_ref(state).is_some() {
        None
    } else {
        suggestion_rows(state)
            .iter()
            .position(|(_, i)| i.image_ref == state.image_ref)
            .map(|i| i as u32)
    };
}

fn close_suggestions(state: &mut WizardState) {
    state.suggest_open = false;
    state.suggest_filter = false;
    state.suggest_index = None;
}

/// Picks a preset: the field shows its ref.
fn choose_image(state: &mut WizardState, image_ref: &str) {
    if find_image(image_ref).is_some() {
        state.image_text = image_ref.to_string();
    }
    rebase(state, image_ref);
}

/// Makes `image_ref` the preset the Space is based on (the field keeps its
/// text, so a custom ref stays).
fn rebase(state: &mut WizardState, image_ref: &str) {
    let Some(next) = find_image(image_ref) else {
        return;
    };
    state.image_ref = next.image_ref.clone();
    state.cpus = default_cpus(&next);
    state.memory_gb = default_memory_gb(&next);
    state.disk_gb = None;
    // Follow the image to where it can run (the choice when possible).
    if !can_place(&next, state.placement) {
        let other = state.placement.other();
        if can_place(&next, other) {
            state.placement = other;
            state.picked = true;
        }
    }
}

/// Advances the wizard. `env` decides whether Continue is allowed.
pub fn reduce(state: &WizardState, action: &WizardAction, env: &WizardEnv) -> WizardState {
    let mut s = state.clone();
    match action {
        WizardAction::SyncDefault { location } => {
            if !s.picked {
                s.placement = offered_location(*location, env);
            }
        }
        WizardAction::ChooseOs { os } => {
            let images = &catalog().images;
            let first = images
                .iter()
                .find(|i| i.os == *os && can_place(i, s.placement))
                .or_else(|| images.iter().find(|i| i.os == *os))
                .map(|i| i.image_ref.clone());
            if let Some(r) = first {
                choose_image(&mut s, &r);
            }
        }
        WizardAction::ChooseImage { image_ref } => {
            choose_image(&mut s, image_ref);
            close_suggestions(&mut s);
        }
        // The same text is no edit (a field echoing its value on focus).
        WizardAction::SetImageText { text } if *text == s.image_text => {}
        WizardAction::SetImageText { text } => {
            s.image_text = text.clone();
            s.suggest_open = true;
            s.suggest_filter = true;
            s.suggest_index = None;
            if find_image(text.trim()).is_some() {
                rebase(&mut s, text.trim());
            }
        }
        WizardAction::OpenImageSuggestions => open_suggestions(&mut s),
        WizardAction::MoveImageSuggestion { delta } => {
            if !s.suggest_open {
                open_suggestions(&mut s);
            } else {
                let n = suggestion_rows(&s).len() as i64;
                if n > 0 {
                    let d = i64::from(*delta);
                    let next = match s.suggest_index {
                        None if d >= 0 => 0,
                        None => n - 1,
                        Some(i) => (i64::from(i) + d).clamp(0, n - 1),
                    };
                    s.suggest_index = Some(next as u32);
                }
            }
        }
        WizardAction::PickImageSuggestion => {
            let rows = suggestion_rows(&s);
            if s.suggest_open
                && let Some((_, image)) = s.suggest_index.and_then(|i| rows.get(i as usize))
            {
                choose_image(&mut s, &image.image_ref.clone());
            }
            close_suggestions(&mut s);
        }
        WizardAction::DismissImageSuggestions => close_suggestions(&mut s),
        WizardAction::SetPlacement { placement } => {
            let image = image_of(&s);
            let open = match placement {
                Location::Yours => !offered_clouds(env).is_empty(),
                Location::Host => selected_host(&s, env).is_some(),
                _ => true,
            };
            if open && can_place(&image, *placement) {
                s.placement = *placement;
                s.picked = true;
            }
        }
        WizardAction::ChooseCloud { cloud } => {
            if offered_clouds(env).iter().any(|c| &c.name == cloud) {
                s.cloud = Some(cloud.clone());
                s.placement = Location::Yours;
                s.picked = true;
            }
        }
        WizardAction::ChoosePlacement { on } => {
            let chosen = placement_options(&s, env)
                .into_iter()
                .any(|o| o.id == *on && o.enabled);
            if chosen {
                s.picked = true;
                if let Some(id) = on.strip_prefix("host:") {
                    s.placement = Location::Host;
                    s.host = Some(id.to_string());
                } else if on == "local" {
                    s.placement = Location::Local;
                } else if on == "cloud" {
                    s.placement = Location::Cloud;
                } else {
                    s.placement = Location::Yours;
                    s.cloud = Some(on.clone());
                }
            }
        }
        WizardAction::ToggleAdvanced => s.advanced = !s.advanced,
        WizardAction::ChooseKind { kind } => {
            let image = base_of(&s);
            if let Some(o) = kind_options(&image, s.placement)
                .into_iter()
                .find(|o| o.kind == *kind)
            {
                if custom_ref(&s).is_some() {
                    rebase(&mut s, &o.image.image_ref);
                } else {
                    choose_image(&mut s, &o.image.image_ref);
                }
            }
        }
        WizardAction::SetRuntime { runtime } => s.runtime_choice = *runtime,
        WizardAction::SetCpus { cpus } => {
            let ((lo, hi), _) = size_ranges(s.placement, env);
            s.cpus = (*cpus).clamp(lo, hi);
        }
        WizardAction::SetMemory { memory_gb } => {
            let (_, (lo, hi)) = size_ranges(s.placement, env);
            s.memory_gb = (*memory_gb).clamp(lo, hi);
        }
        WizardAction::SetDisk { disk_gb } => {
            if let Some((min, max)) = disk_bounds(&image_of(&s), env) {
                s.disk_gb = Some((*disk_gb).clamp(min, max));
            }
        }
        WizardAction::ResetDisk => s.disk_gb = None,
        WizardAction::SetGpu { on } => s.gpu = *on,
        WizardAction::SetName { name } => s.name = name.clone(),
        WizardAction::SetOpenWhenReady { on } => s.open_when_ready = *on,
        WizardAction::Next => {
            if s.step < 3 && view(&s, env).can_continue {
                s.step += 1;
            }
        }
        WizardAction::Back => s.step = s.step.saturating_sub(1),
        WizardAction::ShowAddress => s.mode = WizardMode::Address,
        WizardAction::HideAddress => s.mode = WizardMode::Create,
        WizardAction::SetAddress { url } => s.address.url = url.clone(),
        WizardAction::SetToken { token } => s.address.token = token.clone(),
        WizardAction::SetAddressName { name } => s.address.name = name.clone(),
        WizardAction::SubmitAddress => {
            if looks_like_address(&s.address.url) && !s.address.busy {
                s.address.busy = true;
                s.address.error = None;
            }
        }
        WizardAction::AddressFailed { error } => {
            s.address.busy = false;
            s.address.error = Some(friendly_add_error(error));
        }
    }
    s
}

/// How far along a step is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepState {
    /// Done.
    Done,
    /// Current.
    Current,
    /// Not reached.
    Todo,
}

/// One step of the step list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepView {
    /// Title.
    pub label: String,
    /// State.
    pub state: StepState,
}

/// A selectable tile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tile {
    /// Stable id (`linux`, `cloud`, `vm`, ...).
    pub id: String,
    /// Title.
    pub title: String,
    /// Tooltip.
    pub detail: String,
    /// Selected.
    pub pressed: bool,
    /// Enabled.
    pub enabled: bool,
}

/// A menu entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuOption {
    /// Value.
    pub value: String,
    /// Title.
    pub label: String,
}

/// What `Create Space` sends: one `create_space` call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatePlan {
    /// Where.
    pub placement: Location,
    /// For "Your cloud": the connected cloud's word (`aws`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud: Option<String>,
    /// For one of your machines: its id (`on="host:<id>"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// The image (its variant is the kind).
    pub image: SandboxImage,
    /// Engine (`auto` unless chosen).
    pub runtime: Runtime,
    /// Name, when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// vCPUs (local and Cua Cloud).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpus: Option<u32>,
    /// Memory MB (local and Cua Cloud).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mb: Option<u32>,
    /// Local VMs only: grow the disk to this many GB (larger than the
    /// image's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_gb: Option<u32>,
    /// Open the desktop when ready.
    pub open_when_ready: bool,
    /// Open the desktop once created: asked for and the image streams.
    #[serde(default)]
    pub open_desktop: bool,
    /// A GPU option of the runtime (`paravirtual`), when checked and
    /// offered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu: Option<String>,
}

/// The notice while a plan is being created.
pub fn creating_text(plan: &CreatePlan) -> String {
    format!(
        "Creating {}. It appears in the list when it is ready.",
        plan.name.as_deref().unwrap_or(&plan.image.name)
    )
}

/// The notice when `create_space` failed with `error`.
pub fn create_failed_text(error: &str) -> String {
    format!("Could not create the Space: {error}")
}

/// The SDK arguments of a plan (`create_space`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSpaceArgs {
    /// Image ref.
    pub image: String,
    /// The SDK's `on`: `local`, `cloud`, `host:<machine>`, or a connected
    /// cloud's word (`aws`, `gcp`, `modal`).
    pub on: String,
    /// `container` / `vm`.
    pub kind: SpaceKind,
    /// Engine.
    pub runtime: Runtime,
    /// Name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// vCPUs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpus: Option<u32>,
    /// Memory MB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mb: Option<u32>,
    /// Disk GB (local VMs; the image's size when unset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_gb: Option<u32>,
    /// Runs cua-spacesd.
    pub spacesd: bool,
    /// A GPU option of the runtime (`paravirtual`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu: Option<String>,
}

/// The SDK call a plan makes.
pub fn create_args(plan: &CreatePlan) -> CreateSpaceArgs {
    let local = plan.placement == Location::Local;
    CreateSpaceArgs {
        image: plan.image.image_ref.clone(),
        on: match plan.placement {
            Location::Yours => plan.cloud.clone().unwrap_or_default(),
            Location::Host => format!("host:{}", plan.host.as_deref().unwrap_or_default()),
            p => p.as_str().to_string(),
        },
        kind: plan.image.variant,
        runtime: plan.runtime,
        name: plan.name.clone(),
        cpus: plan.cpus,
        memory_mb: plan.memory_mb,
        disk_gb: if local { plan.disk_gb } else { None },
        spacesd: plan.image.spacesd,
        gpu: plan.gpu.clone(),
    }
}

/// The runtime whose GPU option applies: the local engine of `image` (a
/// container's by its runtime), or the cloud's.
fn gpu_runtime(image: &SandboxImage, placement: Location, runtime: Runtime) -> &'static str {
    match placement {
        Location::Cloud => "fleet",
        // Your cloud's machine types carry no GPU option (yet): no row.
        Location::Yours => "yours",
        // Nor do your other machines (their runtimes are not this Mac's).
        Location::Host => "host",
        Location::Local => match image.local {
            Some(LocalEngine::Lume) => "lume",
            Some(LocalEngine::Qemu) => "qemu",
            Some(LocalEngine::Container) if runtime == Runtime::Gvisor => "gvisor",
            _ => "container",
        },
    }
}

/// The GPU row for the effective runtime, and the option a create passes
/// (checked and offered).
pub fn gpu_row(
    state: &WizardState,
    env: &WizardEnv,
    image: &SandboxImage,
    runtime: Runtime,
) -> (Option<GpuRow>, Option<String>) {
    let want = gpu_runtime(image, state.placement, runtime);
    let Some(choice) = env
        .gpus
        .as_ref()
        .and_then(|all| all.iter().find(|g| g.runtime == want))
    else {
        return (None, None);
    };
    let on = state.gpu && choice.supported;
    let row = GpuRow {
        label: if choice.experimental {
            format!("{} (Experimental)", choice.label)
        } else {
            choice.label.clone()
        },
        on,
        enabled: choice.supported,
        reason: (!choice.supported)
            .then(|| choice.reason.clone().filter(|r| !r.is_empty()))
            .flatten(),
        learn_more_label: "Learn more".into(),
        learn_more_url: choice.learn_more.clone(),
    };
    (Some(row), on.then(|| choice.id.clone()))
}

/// The address form as drawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddressView {
    /// The address has the right shape.
    pub valid: bool,
    /// Mark the field invalid.
    pub show_invalid: bool,
    /// "Add Space" is enabled.
    pub can_submit: bool,
    /// "Add Space" / "Connecting...".
    pub submit_label: String,
    /// Error line.
    pub error: Option<String>,
    /// What "Add Space" sends (trimmed), when it can be pressed.
    pub submit: Option<AddressSubmit>,
}

/// The `add` call "Add Space" makes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddressSubmit {
    /// `host:port`, trimmed.
    pub url: String,
    /// Token, when given.
    pub token: Option<String>,
    /// Name, when given.
    pub name: Option<String>,
}

/// One preset in the image suggestions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageSuggestion {
    /// Image ref (what picking it fills in).
    #[serde(rename = "ref")]
    pub image_ref: String,
    /// The image's name, shown after the ref.
    pub label: String,
    /// The keyboard highlight.
    pub highlighted: bool,
    /// The field holds this preset.
    pub selected: bool,
}

/// A catalog group of suggestions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuggestionGroup {
    /// Group id.
    pub id: String,
    /// Title.
    pub label: String,
    /// Matching presets.
    pub rows: Vec<ImageSuggestion>,
}

/// The image field: an editable reference with preset suggestions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageFieldView {
    /// The text.
    pub text: String,
    /// Show the suggestions (open and something matches).
    pub open: bool,
    /// The suggestions, filtered while typing.
    pub groups: Vec<SuggestionGroup>,
    /// One line when the ref cannot be used.
    pub error: Option<String>,
    /// The text is not a preset.
    pub custom: bool,
}

/// One field of the current step, in display order: what each shell
/// renders with its own control for `id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WizardField {
    /// `os`, `image`, `placement`, `kind`, `runtime`, `connect-cloud`,
    /// `connect-by-address`, `cpus`, `memory`, `disk`, `name`,
    /// `open-when-ready`, `address`, `token`, `address-name`.
    pub id: String,
    /// Label (or the link's title).
    pub label: String,
    /// Placeholder.
    pub placeholder: Option<String>,
    /// One line under the field.
    pub error: Option<String>,
    /// Inside "Advanced".
    pub advanced: bool,
}

/// The shells' fixed words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WizardLabels {
    /// "Cancel".
    pub cancel: String,
    /// "Back".
    pub back: String,
    /// "Advanced".
    pub advanced: String,
}

/// The wizard as drawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WizardView {
    /// Create or address.
    pub mode: WizardMode,
    /// 0..=3.
    pub step: u32,
    /// Step list.
    pub steps: Vec<StepView>,
    /// Step heading.
    pub title: String,
    /// OS tiles.
    pub os_tiles: Vec<Tile>,
    /// The image field.
    pub image_field: ImageFieldView,
    /// The image.
    pub image: SandboxImage,
    /// The "Run on" menu: This Mac, your machines that provide Spaces,
    /// then your connected clouds (with the "Your cloud" experiment).
    pub placements: Vec<PlacementOption>,
    /// The chosen entry's id ([`PlacementOption::id`]).
    pub placement_id: String,
    /// The connected cloud "Your cloud" creates in (`aws`).
    pub cloud: Option<String>,
    /// The machine of yours it creates on ([`SpaceHost::id`]).
    pub host: Option<String>,
    /// Why the chosen placement cannot be used.
    pub placement_error: Option<String>,
    /// Advanced shown.
    pub advanced: bool,
    /// Kind tiles.
    pub kind_tiles: Vec<Tile>,
    /// Runtime menu.
    pub runtimes: Vec<MenuOption>,
    /// Effective runtime.
    pub runtime: Runtime,
    /// The runtime menu has a choice.
    pub runtime_enabled: bool,
    /// Cores.
    pub cpus: u32,
    /// Slider min.
    pub min_cpus: u32,
    /// Slider max.
    pub max_cpus: u32,
    /// Memory slider min, GB.
    pub min_memory_gb: u32,
    /// Memory slider max, GB.
    pub max_memory_gb: u32,
    /// "2 cores".
    pub cpus_text: String,
    /// Memory GB.
    pub memory_gb: u32,
    /// "4 GB".
    pub memory_text: String,
    /// The disk slider shows (a local VM whose disk can grow).
    pub disk_editable: bool,
    /// Disk GB (the slider's value).
    pub disk_gb: u32,
    /// Disk slider min, GB (the image's size).
    pub min_disk_gb: u32,
    /// Disk slider max, GB.
    pub max_disk_gb: u32,
    /// "150 GB".
    pub disk_text: String,
    /// One quiet line under the slider: a VM disk is sparse, so it uses
    /// the unpacked size at first, up to the chosen size; and a resize
    /// when the chosen size is not the image's.
    pub disk_note: Option<String>,
    /// "Reset" while the chosen size is not the image's.
    pub disk_reset_label: Option<String>,
    /// The slider's tooltip: why it cannot go below the image's disk.
    pub disk_help: Option<String>,
    /// What it costs, in one line: `About $0.18/hour` (cloud, at this
    /// account's rates); `None` on the user's own hardware (this Mac, or a
    /// machine added by address) and when the cloud rates are unknown.
    pub price: Option<String>,
    /// The Resources step's lines under the sliders: kind, architecture,
    /// disk (or download), available.
    pub resource_facts: Vec<WizardFact>,
    /// One line when the Space does not fit where it is written (blocks
    /// Continue).
    pub resources_error: Option<String>,
    /// The GPU row (Resources), where the runtime offers a GPU.
    #[serde(default)]
    pub gpu: Option<GpuRow>,
    /// Name as typed.
    pub name: String,
    /// The name breaks the DNS-label rule.
    pub name_invalid: bool,
    /// Why, in one line.
    pub name_error: Option<String>,
    /// Open when ready.
    pub open_when_ready: bool,
    /// "No desktop stream for this image yet."
    pub stream_note: Option<String>,
    /// The Summary list.
    pub summary: Vec<Fact>,
    /// Continue is enabled.
    pub can_continue: bool,
    /// Back shows.
    pub show_back: bool,
    /// "Continue" or "Create Space".
    pub primary_label: String,
    /// What Create sends.
    pub plan: CreatePlan,
    /// The address form.
    pub address: AddressView,
    /// The fields of this step (or the address form), in order.
    pub fields: Vec<WizardField>,
    /// Fixed words.
    pub labels: WizardLabels,
}

fn runtime_word(r: Runtime, variant: SpaceKind) -> String {
    format!(
        "{} {} on this Mac",
        r.label(),
        if variant == SpaceKind::Vm {
            "virtual machine"
        } else {
            "container"
        }
    )
}

/// The Space name rule, in one line.
pub const NAME_RULE: &str = "Lowercase letters, digits and dashes";

fn image_field(state: &WizardState, error: Option<String>, custom: bool) -> ImageFieldView {
    let rows = suggestion_rows(state);
    let highlight = state.suggest_index.map(|i| i as usize);
    let mut groups: Vec<SuggestionGroup> = Vec::new();
    for (index, (label, image)) in rows.iter().enumerate() {
        let row = ImageSuggestion {
            image_ref: image.image_ref.clone(),
            label: image.name.clone(),
            highlighted: highlight == Some(index),
            selected: !custom && image.image_ref == state.image_ref,
        };
        match groups.last_mut() {
            Some(g) if g.id == image.group => g.rows.push(row),
            _ => groups.push(SuggestionGroup {
                id: image.group.clone(),
                label: label.clone(),
                rows: vec![row],
            }),
        }
    }
    ImageFieldView {
        text: state.image_text.clone(),
        open: state.suggest_open && !groups.is_empty(),
        groups,
        error,
        custom,
    }
}

fn field(id: &str, label: &str) -> WizardField {
    WizardField {
        id: id.into(),
        label: label.into(),
        placeholder: None,
        error: None,
        advanced: false,
    }
}

/// The current step's fields, in order.
fn fields(
    state: &WizardState,
    env: &WizardEnv,
    image_error: Option<String>,
    placement_error: Option<String>,
    name_error: Option<String>,
    disk_editable: bool,
) -> Vec<WizardField> {
    let yours = state.placement == Location::Yours;
    match (state.mode, state.step) {
        (WizardMode::Address, _) => vec![
            WizardField {
                placeholder: Some("10.0.0.5:3211".into()),
                ..field("address", "Address")
            },
            WizardField {
                placeholder: Some("CUA_ENV_TOKEN".into()),
                ..field("token", "Token")
            },
            WizardField {
                placeholder: Some("Optional".into()),
                ..field("address-name", "Name")
            },
        ],
        (WizardMode::Create, 0) => vec![
            field("os", "System"),
            WizardField {
                placeholder: Some(IMAGE_PLACEHOLDER.into()),
                error: image_error,
                ..field("image", "Image")
            },
            WizardField {
                error: placement_error,
                ..field("placement", "Run on")
            },
            WizardField {
                advanced: true,
                ..field("kind", "Kind")
            },
            WizardField {
                advanced: true,
                ..field("runtime", "Runtime")
            },
        ]
        .into_iter()
        // Connecting a cloud only with the "Your cloud" experiment.
        .chain(
            env.experiments
                .your_cloud
                .then(|| field("connect-cloud", CONNECT_CLOUD_LABEL)),
        )
        .chain([field("connect-by-address", "Connect by address\u{2026}")])
        .collect(),
        // The cloud picks the machine: no sliders, its facts only.
        (WizardMode::Create, 1) if yours => vec![],
        (WizardMode::Create, 1) => {
            let mut f = vec![field("cpus", "CPU cores"), field("memory", "Memory")];
            if disk_editable {
                f.push(field("disk", "Disk"));
            }
            f
        }
        (WizardMode::Create, 2) => vec![
            WizardField {
                placeholder: Some("Optional".into()),
                error: name_error,
                ..field("name", "Name")
            },
            field("open-when-ready", "Open when ready"),
        ],
        _ => vec![],
    }
}

/// One GiB: disks and memory are sized in binary units, labeled GB.
const GIB: u64 = 1 << 30;
/// The disk slider's ceiling, GB.
pub const MAX_DISK_GB: u32 = 1024;
/// The symbol a warning fact shows (SF Symbols; the web app maps it).
pub use crate::model::WARNING_SYMBOL;

/// `560 MB`, `1.2 GB`, `150 GB` (binary units, like the memory slider).
pub fn size_text(bytes: u64) -> String {
    let gb = bytes as f64 / GIB as f64;
    let mb = (bytes as f64 / (1u64 << 20) as f64).round();
    if mb < 1000.0 {
        format!("{} MB", mb.max(1.0) as u64)
    } else if gb < 10.0 {
        format!("{:.1} GB", (gb * 10.0).round() / 10.0)
    } else {
        format!("{} GB", gb.round() as u64)
    }
}

/// `ARM`, `x64` (an OCI or Rust architecture name; others as they are).
pub fn arch_label(arch: &str) -> String {
    crate::model::arch_label(arch).unwrap_or(arch).to_string()
}

/// The platform that runs ([`crate::model::run_arch`]).
fn run_arch(image: &SandboxImage, placement: Location, host: Option<&str>) -> Option<String> {
    crate::model::run_arch(&image.arch, placement == Location::Local, host)
}

/// The image's sizes on `arch` (else its first platform).
fn platform_size<'a>(image: &'a SandboxImage, arch: Option<&str>) -> Option<&'a PlatformSize> {
    let s = image.sizes.as_ref()?;
    arch.and_then(|a| s.platforms.iter().find(|p| p.arch == a))
        .or_else(|| s.platforms.first())
}

/// Whether the guest sees a larger disk: Lume grows a macOS clone's APFS
/// container before its first boot, and the Linux VM images grow their
/// root partition at boot (cloud-init growpart). Windows guests keep their
/// partition size, and a container's disk is its engine's.
pub fn disk_can_grow(image: &SandboxImage) -> bool {
    image.sizes.is_some()
        && match image.local {
            Some(LocalEngine::Lume) => true,
            Some(LocalEngine::Qemu) => image.os == SpaceOs::Linux,
            _ => false,
        }
}

/// The volume a local Space of `image` is written to.
fn volume_of<'a>(image: &SandboxImage, env: &'a WizardEnv) -> Option<&'a StorageVolume> {
    let s = env.storage.as_ref()?;
    match image.local? {
        LocalEngine::Lume => s.lume.as_ref(),
        LocalEngine::Qemu => s.qemu.as_ref(),
        LocalEngine::Container => s.container.as_ref(),
    }
}

/// The disk slider's range, GB: the image's size up to the smaller of
/// [`MAX_DISK_GB`] and the volume. `None` when the disk cannot grow.
fn disk_bounds(image: &SandboxImage, env: &WizardEnv) -> Option<(u32, u32)> {
    if !disk_can_grow(image) {
        return None;
    }
    let arch = run_arch(image, Location::Local, env.host_arch.as_deref());
    let min = platform_size(image, arch.as_deref())?.disk.div_ceil(GIB) as u32;
    let volume = volume_of(image, env).map_or(u32::MAX, |v| (v.total_bytes / GIB) as u32);
    Some((min, MAX_DISK_GB.min(volume).max(min)))
}

/// One line of the Resources step: a label and its value, with an optional
/// warning symbol and its tooltip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WizardFact {
    /// `kind`, `arch`, `disk`, `download`, `available`.
    pub id: String,
    /// Label.
    pub label: String,
    /// Value.
    pub value: String,
    /// A symbol after the value (`exclamationmark.triangle`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    /// The symbol's tooltip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
}

fn fact_line(id: &str, label: &str, value: String) -> WizardFact {
    WizardFact {
        id: id.into(),
        label: label.into(),
        value,
        symbol: None,
        help: None,
    }
}

/// What the Resources step says about disk and room, and whether the
/// Space fits.
struct DiskView {
    /// The slider's range, when the disk can grow here.
    bounds: Option<(u32, u32)>,
    /// The disk as it will be, GB.
    disk_gb: Option<u32>,
    /// The Resources facts (kind, architecture, disk, download, available).
    facts: Vec<WizardFact>,
    /// One line when the Space would not fit.
    error: Option<String>,
    /// The emulation tooltip, when the platform is not the host's.
    emulated: Option<String>,
    /// The platform that runs.
    arch: Option<String>,
    /// The line under the slider (the slider shows).
    note: Option<String>,
    /// The chosen size is not the image's.
    resized: bool,
}

fn disk_view(state: &WizardState, image: &SandboxImage, env: &WizardEnv) -> DiskView {
    let local = state.placement == Location::Local;
    let host = env.host_arch.as_deref();
    // A machine of yours runs its own platform: not guessed here.
    let arch = if state.placement == Location::Host {
        None
    } else {
        run_arch(image, state.placement, host)
    };
    let mut facts = vec![fact_line("kind", "Kind", image.variant.label().into())];
    let emulated = crate::model::emulation_warning(local, host, arch.as_deref());
    if let Some(a) = &arch {
        facts.push(WizardFact {
            symbol: emulated.as_ref().map(|_| WARNING_SYMBOL.to_string()),
            help: emulated.clone(),
            ..fact_line("arch", "Architecture", arch_label(a))
        });
    }
    let size = platform_size(image, arch.as_deref());
    let bounds = if local { disk_bounds(image, env) } else { None };
    let disk_gb = bounds
        .map(|(min, max)| state.disk_gb.unwrap_or(min).clamp(min, max))
        .or_else(|| size.map(|s| s.disk.div_ceil(GIB) as u32));
    // What is pulled is known with the storage probe; without it nothing
    // is said about a download.
    let pulled = env
        .storage
        .as_ref()
        .map(|s| s.pulled.contains(&image.image_ref));
    let download = size
        .filter(|_| local && pulled == Some(false))
        .map(|s| s.download);
    if let Some(s) = size {
        if bounds.is_none() {
            let value = match download {
                Some(d) => format!("{} (download {})", size_text(s.disk), size_text(d)),
                None => size_text(s.disk),
            };
            facts.push(fact_line("disk", "Disk", value));
        } else if let Some(d) = download {
            facts.push(fact_line("download", "Download", size_text(d)));
        }
    }
    let mut error = None;
    if local && let Some(v) = volume_of(image, env) {
        facts.push(fact_line(
            "available",
            "Available",
            format!("{} on {}", size_text(v.available_bytes), v.name),
        ));
        let reserve = env.storage.as_ref().map_or(0, |s| s.reserve_bytes);
        let need = size
            .filter(|_| pulled == Some(false))
            .map_or(0, |s| s.download.saturating_add(s.unpacked));
        if need > 0 && v.available_bytes < need.saturating_add(reserve) {
            error = Some(format!(
                "Not enough space on {}: needs {}, {} available.",
                v.name,
                size_text(need.saturating_add(reserve)),
                size_text(v.available_bytes)
            ));
        }
    }
    let resized = bounds.is_some_and(|(min, _)| disk_gb.is_some_and(|d| d != min));
    let note = bounds.zip(disk_gb).zip(size).map(|((_, d), s)| {
        let uses = format!(
            "Uses about {} on this Mac at first, up to {d} GB as the Space fills it.",
            size_text(s.unpacked)
        );
        if resized {
            format!("The disk will be resized to {d} GB after downloading. {uses}")
        } else {
            uses
        }
    });
    DiskView {
        bounds,
        disk_gb,
        facts,
        error,
        emulated,
        arch,
        note,
        resized,
    }
}

/// `state` with the placement it can have now: a cloud no longer offered
/// (the "Your cloud" experiment turned off) or a machine no longer listed
/// is this Mac again.
fn effective(state: &WizardState, env: &WizardEnv) -> WizardState {
    let placement = match state.placement {
        Location::Yours if offered_clouds(env).is_empty() => Location::Local,
        Location::Host if selected_host(state, env).is_none() => Location::Local,
        p => p,
    };
    WizardState {
        placement,
        ..state.clone()
    }
}

/// This Mac's line: its engine for `image`, or why it cannot.
fn local_detail(image: &SandboxImage, env: &WizardEnv) -> String {
    if !env.local_available {
        env.local_reason
            .clone()
            .filter(|r| !r.is_empty())
            .unwrap_or_else(|| "No local runtime found (Docker or Lume).".into())
    } else {
        image
            .local
            .map(|l| l.label().to_string())
            .unwrap_or_else(|| "Not available for this image.".into())
    }
}

/// A connected cloud's line for `image`: its machine and price, or why not.
fn your_cloud_detail(image: &SandboxImage, c: &ConnectedCloud) -> String {
    match cloud_offer(image, c).filter(|o| o.supported) {
        Some(o) => match cloud_price_text(o.usd_per_hour) {
            Some(p) => format!("{}, {}", o.machine_type, p.to_lowercase()),
            None => o.machine_type.clone(),
        },
        None => cloud_refusal(image, c).unwrap_or_default(),
    }
}

/// How a machine of yours is reached, in words.
fn via_text(host: &SpaceHost) -> &'static str {
    if host.via == "direct" {
        "Over Tailscale or your network"
    } else {
        "Through the Cua relay"
    }
}

/// The "Run on" menu: This Mac, each machine of yours that provides Spaces
/// (disabled with why while offline or at its limit), then each connected
/// cloud while the "Your cloud" experiment is on (Cua Cloud first, while
/// the apps offer it).
pub fn placement_options(state: &WizardState, env: &WizardEnv) -> Vec<PlacementOption> {
    let state = &effective(state, env);
    let image = image_of(state);
    let placement = state.placement;
    let mut out = vec![PlacementOption {
        id: "local".into(),
        label: Location::Local.label().into(),
        group: "this-mac".into(),
        selected: placement == Location::Local,
        enabled: image.local.is_some(),
        detail: local_detail(&image, env),
    }];
    for h in &env.hosts {
        let why = host_refusal(&image, h);
        out.push(PlacementOption {
            id: format!("host:{}", h.id),
            label: match &why {
                Some((word, _)) => format!("{} ({word})", h.name),
                None => h.name.clone(),
            },
            group: "hosts".into(),
            selected: placement == Location::Host && state.host.as_deref() == Some(h.id.as_str()),
            enabled: why.is_none(),
            detail: why
                .map(|(_, line)| line)
                .unwrap_or_else(|| via_text(h).into()),
        });
    }
    if crate::model::CLOUD_SPACES_OFFERED {
        out.push(PlacementOption {
            id: "cloud".into(),
            label: Location::Cloud.label().into(),
            group: "clouds".into(),
            selected: placement == Location::Cloud,
            enabled: can_place(&image, Location::Cloud),
            detail: if !env.cloud_available {
                "Sign in to Cua to create cloud Spaces.".to_string()
            } else {
                image
                    .cloud
                    .map(|c| c.label().to_string())
                    .unwrap_or_else(|| "Not available for this image yet.".into())
            },
        });
    }
    let chosen = selected_cloud(state, env).map(|c| c.name.as_str());
    for c in offered_clouds(env) {
        out.push(PlacementOption {
            id: c.name.clone(),
            label: c.label.clone(),
            group: "clouds".into(),
            selected: placement == Location::Yours && chosen == Some(c.name.as_str()),
            enabled: true,
            detail: your_cloud_detail(&image, c),
        });
    }
    out
}

/// The wizard as drawn.
pub fn view(state: &WizardState, env: &WizardEnv) -> WizardView {
    let eff = effective(state, env);
    let state = &eff;
    let image = image_of(state);
    let placement = state.placement;
    let engines = runtime_options(&image, placement);
    let runtime = if engines.contains(&state.runtime_choice) {
        state.runtime_choice
    } else {
        engines.first().copied().unwrap_or(Runtime::Auto)
    };
    let kinds = kind_options(&base_of(state), placement);
    let cloud = selected_cloud(state, env);
    let yours = placement == Location::Yours;
    let host = selected_host(state, env).filter(|_| placement == Location::Host);
    let host_why = host.and_then(|h| host_refusal(&image, h)).map(|(_, l)| l);
    let offer = cloud
        .and_then(|c| cloud_offer(&image, c))
        .filter(|o| o.supported);
    let refusal = |i: &SandboxImage| match cloud {
        Some(c) => cloud_refusal(i, c),
        None => Some(YOUR_CLOUD_CONNECT_DETAIL.to_string()),
    };
    let placeable = can_place(&image, placement);
    let local_ready = env.local_available
        && env
            .local_backends
            .as_ref()
            .is_none_or(|b| local_runtime_ready(&image, b));
    let placement_ready = match placement {
        Location::Cloud => env.cloud_available,
        Location::Local => local_ready,
        Location::Yours => offer.is_some(),
        Location::Host => host.is_some() && host_why.is_none(),
    };
    let name = state.name.trim().to_string();
    let name_invalid = !name.is_empty() && !valid_space_name(&name);
    let custom = custom_ref(state).is_some();
    let image_error = validate_image_ref(&state.image_text);
    let disk = disk_view(state, &image, env);
    let can_continue = match state.step {
        0 => placeable && placement_ready && image_error.is_none(),
        1 => disk.error.is_none(),
        2 => !name_invalid,
        _ => true,
    };
    let images = &catalog().images;
    let pressed_os = selected_os(state);
    let os_tiles = [
        (SpaceOs::Linux, "Linux", "Ubuntu desktop with cua-spacesd"),
        (SpaceOs::Windows, "Windows", "Windows Server VM"),
        (SpaceOs::Macos, "macOS", "Apple silicon VM through Lume"),
    ]
    .into_iter()
    .map(|(os, title, detail)| {
        // In your cloud, a system it cannot run is greyed out with why.
        let why = yours
            .then(|| {
                let mut of_os = images.iter().filter(|i| i.os == os).peekable();
                of_os.peek()?;
                let reasons: Vec<String> = of_os.map(&refusal).collect::<Option<_>>()?;
                reasons.into_iter().next()
            })
            .flatten();
        Tile {
            id: os.as_str().into(),
            title: title.into(),
            detail: why.clone().unwrap_or_else(|| detail.into()),
            pressed: pressed_os == Some(os),
            enabled: images.iter().any(|i| i.os == os) && why.is_none(),
        }
    })
    .collect();
    let placements = placement_options(state, env);
    let placement_id = placements
        .iter()
        .find(|o| o.selected)
        .map(|o| o.id.clone())
        .unwrap_or_else(|| "local".into());
    let placement_error = (!placement_ready && placeable).then(|| match placement {
        Location::Cloud => "Sign in to use Cua Cloud.".to_string(),
        Location::Yours => match cloud {
            Some(c) => cloud_refusal(&image, c)
                .unwrap_or_else(|| format!("{} does not run this image.", c.title)),
            None => YOUR_CLOUD_CONNECT_DETAIL.to_string(),
        },
        Location::Host => host_why
            .clone()
            .unwrap_or_else(|| "Choose a machine.".to_string()),
        Location::Local => {
            if env.local_available {
                let what = match image.local {
                    Some(LocalEngine::Lume) => "Lume",
                    Some(LocalEngine::Qemu) => "QEMU",
                    _ => "container",
                };
                match local_engine_detail(&image, env) {
                    Some(why) => format!("This Mac cannot run {what} Spaces: {why}."),
                    None => format!("This Mac cannot run {what} Spaces."),
                }
            } else {
                env.local_reason
                    .clone()
                    .filter(|r| !r.is_empty())
                    .unwrap_or_else(|| "No local runtime found.".into())
            }
        }
    });
    let kind_tiles = [SpaceKind::Container, SpaceKind::Vm]
        .into_iter()
        .map(|kind| {
            let option = kinds.iter().find(|k| k.kind == kind);
            // In your cloud, a kind it cannot run is greyed out with why.
            let why = option.filter(|_| yours).and_then(|o| refusal(&o.image));
            Tile {
                id: kind.as_str().into(),
                title: kind.label().into(),
                detail: why.clone().unwrap_or_default(),
                pressed: image.variant == kind,
                enabled: option.is_some() && why.is_none(),
            }
        })
        .collect();
    let engine_text = match placement {
        Location::Cloud => image
            .cloud
            .map(|c| c.label().to_string())
            .unwrap_or_else(|| "Not available in Cua Cloud yet".into()),
        Location::Local => match image.local {
            Some(l) if runtime == Runtime::Auto => l.label().to_string(),
            Some(_) => runtime_word(runtime, image.variant),
            None => "Not available on this Mac".into(),
        },
        Location::Yours => match offer {
            Some(o) if !o.machine_type.is_empty() => {
                format!("{} on {}", image.variant.label(), o.machine_type)
            }
            _ => image.variant.label().to_string(),
        },
        // The machine picks its own engine.
        Location::Host => match host {
            Some(h) => format!("{} on {}", image.variant.label(), h.name),
            None => image.variant.label().to_string(),
        },
    };
    let cores = |n: u32| format!("{n} {}", if n == 1 { "core" } else { "cores" });
    let local = placement == Location::Local;
    let (cpus, memory_gb) = effective_size(state, env);
    let ((min_cpus, max_cpus), (min_memory_gb, max_memory_gb)) = size_ranges(placement, env);
    // No price on the user's own hardware (this Mac, one of your machines,
    // or a machine added by address): only placements that bill show one.
    let price = if local || placement == Location::Host || state.mode == WizardMode::Address {
        None
    } else if yours {
        offer.and_then(|o| cloud_price_text(o.usd_per_hour))
    } else {
        price_text(env.cloud_pricing.as_ref(), cpus, memory_gb)
    };
    // When a Space in your cloud deletes itself.
    let lifetime = cloud.filter(|_| yours).map(|c| ttl_text(c.ttl_hours));
    let mut summary = vec![
        Fact {
            label: "System".into(),
            value: if custom {
                image.os.label().to_string()
            } else {
                image.name.clone()
            },
            copy: None,
            help: None,
            warning: None,
        },
        Fact {
            label: "Image".into(),
            value: image.image_ref.clone(),
            copy: None,
            help: None,
            warning: None,
        },
        Fact {
            label: "Runs on".into(),
            value: match (cloud.filter(|_| yours), host) {
                (Some(c), _) => c.label.clone(),
                (None, Some(h)) => h.name.clone(),
                _ => placement.label().into(),
            },
            copy: None,
            help: None,
            warning: None,
        },
        Fact {
            label: "Kind".into(),
            value: image.variant.label().into(),
            copy: None,
            help: None,
            warning: None,
        },
    ];
    // The cloud's machine decides its architecture: not guessed here.
    if let Some(a) = disk.arch.as_ref().filter(|_| !yours) {
        summary.push(Fact {
            label: "Architecture".into(),
            value: match &disk.emulated {
                Some(_) => format!("{} (emulated)", arch_label(a)),
                None => arch_label(a),
            },
            copy: None,
            help: disk.emulated.clone(),
            warning: None,
        });
    }
    summary.extend([Fact {
        label: "Runtime".into(),
        value: engine_text,
        copy: None,
        help: None,
        warning: None,
    }]);
    if !yours {
        summary.push(Fact {
            label: "Resources".into(),
            value: match disk
                .disk_gb
                .filter(|_| local && image.variant == SpaceKind::Vm)
            {
                Some(d) => format!("{}, {memory_gb} GB memory, {d} GB disk", cores(cpus)),
                None => format!("{}, {memory_gb} GB memory", cores(cpus)),
            },
            copy: None,
            help: None,
            warning: None,
        });
    }
    // One short line; nothing when the cloud rates are unknown.
    if let Some(value) = price.clone() {
        summary.push(Fact {
            label: "Cost".into(),
            value,
            copy: None,
            help: None,
            warning: None,
        });
    }
    if let Some(value) = lifetime.clone() {
        summary.push(Fact {
            label: "Deletes itself".into(),
            value,
            copy: None,
            help: None,
            warning: None,
        });
    }
    let (gpu, gpu_option) = gpu_row(state, env, &image, runtime);
    if let Some(row) = gpu.as_ref().filter(|r| r.on) {
        summary.push(Fact {
            label: "GPU".into(),
            value: row.label.clone(),
            copy: None,
            help: None,
            warning: None,
        });
    }
    summary.push(Fact {
        label: "Name".into(),
        value: if name.is_empty() {
            "Chosen automatically".into()
        } else {
            name.clone()
        },
        copy: None,
        help: None,
        warning: None,
    });
    summary.push(Fact {
        label: "When ready".into(),
        value: if state.open_when_ready {
            "Open the desktop"
        } else {
            "Stay in the list"
        }
        .into(),
        copy: None,
        help: None,
        warning: None,
    });
    let plan = CreatePlan {
        placement,
        cloud: cloud.filter(|_| yours).map(|c| c.name.clone()),
        host: host.map(|h| h.id.clone()),
        image: image.clone(),
        runtime,
        name: (!name.is_empty()).then(|| name.clone()),
        // In your cloud the machine type sets the size.
        cpus: (!yours).then_some(cpus),
        memory_mb: (!yours).then_some(memory_gb * 1024),
        // Only a disk larger than the image's is asked for.
        disk_gb: disk
            .bounds
            .zip(disk.disk_gb)
            .filter(|((min, _), d)| d > min)
            .map(|(_, d)| d),
        open_when_ready: state.open_when_ready,
        open_desktop: state.open_when_ready && image.spacesd,
        gpu: gpu_option,
    };
    let name_error = name_invalid.then(|| NAME_RULE.to_string());
    let placement_error_for_fields = placement_error.clone();
    let addr = &state.address;
    let valid = looks_like_address(&addr.url);
    WizardView {
        mode: state.mode,
        step: state.step,
        steps: STEPS
            .iter()
            .enumerate()
            .map(|(i, l)| StepView {
                label: (*l).into(),
                state: match (i as u32).cmp(&state.step) {
                    std::cmp::Ordering::Less => StepState::Done,
                    std::cmp::Ordering::Equal => StepState::Current,
                    std::cmp::Ordering::Greater => StepState::Todo,
                },
            })
            .collect(),
        title: match state.mode {
            WizardMode::Address => "Connect by address",
            WizardMode::Create => match state.step {
                0 => "Choose a system",
                1 => "Resources",
                2 => "Options",
                _ => "Summary",
            },
        }
        .into(),
        os_tiles,
        image_field: image_field(state, image_error.clone(), custom),
        image: image.clone(),
        placements,
        placement_id,
        cloud: cloud.map(|c| c.name.clone()),
        host: host.map(|h| h.id.clone()),
        placement_error,
        advanced: state.advanced,
        kind_tiles,
        runtimes: engines
            .iter()
            .map(|r| MenuOption {
                value: r.as_str().into(),
                label: r.label().into(),
            })
            .collect(),
        runtime,
        runtime_enabled: engines.len() >= 2,
        cpus,
        min_cpus,
        max_cpus,
        min_memory_gb,
        max_memory_gb,
        cpus_text: cores(cpus),
        memory_gb,
        memory_text: format!("{memory_gb} GB"),
        disk_editable: disk.bounds.is_some(),
        disk_gb: disk.disk_gb.unwrap_or(0),
        min_disk_gb: disk.bounds.map_or(0, |b| b.0),
        max_disk_gb: disk.bounds.map_or(0, |b| b.1),
        disk_text: disk.disk_gb.map(|d| format!("{d} GB")).unwrap_or_default(),
        disk_note: disk.note.clone(),
        disk_reset_label: disk.resized.then(|| "Reset".into()),
        disk_help: disk
            .bounds
            .map(|(min, _)| format!("A VM disk cannot shrink below the image's {min} GB.")),
        price,
        resource_facts: if yours {
            let mut f = vec![fact_line("kind", "Kind", image.variant.label().into())];
            if let Some(o) = offer.filter(|o| !o.machine_type.is_empty()) {
                f.push(fact_line("machine", "Machine", o.machine_type.clone()));
            }
            if let Some(l) = lifetime.clone() {
                f.push(fact_line("lifetime", "Deletes itself", l));
            }
            f
        } else {
            disk.facts.clone()
        },
        resources_error: disk.error.clone(),
        gpu,
        name: state.name.clone(),
        name_invalid,
        name_error: name_error.clone(),
        open_when_ready: state.open_when_ready,
        stream_note: (!image.spacesd).then(|| "No desktop stream for this image yet.".into()),
        summary,
        can_continue,
        show_back: state.step > 0,
        primary_label: if state.step < 3 {
            "Continue"
        } else {
            "Create Space"
        }
        .into(),
        plan,
        address: AddressView {
            valid,
            show_invalid: !addr.url.is_empty() && !valid,
            can_submit: valid && !addr.busy,
            submit_label: if addr.busy {
                "Connecting\u{2026}"
            } else {
                "Add Space"
            }
            .into(),
            error: addr.error.clone(),
            submit: (valid && !addr.busy).then(|| {
                let opt = |t: &str| Some(t.trim().to_string()).filter(|t| !t.is_empty());
                AddressSubmit {
                    url: addr.url.trim().to_string(),
                    token: opt(&addr.token),
                    name: opt(&addr.name),
                }
            }),
        },
        fields: fields(
            state,
            env,
            image_error,
            placement_error_for_fields,
            name_error,
            disk.bounds.is_some(),
        ),
        labels: WizardLabels {
            cancel: "Cancel".into(),
            back: "Back".into(),
            advanced: "Advanced".into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `s` placed in Cua Cloud, which the apps do not offer
    /// ([`crate::model::CLOUD_SPACES_OFFERED`]) and `reduce` refuses: the
    /// cloud paths stay covered for when they do.
    fn in_cloud(s: &WizardState) -> WizardState {
        WizardState {
            placement: Location::Cloud,
            picked: true,
            ..s.clone()
        }
    }

    fn env() -> WizardEnv {
        WizardEnv {
            default_location: Location::Local,
            cloud_available: false,
            local_available: true,
            local_reason: None,
            local_backends: Some(vec!["docker".into()]),
            local_details: None,
            max_cpus: 8,
            host_arch: Some("arm64".into()),
            storage: None,
            cloud_pricing: None,
            clouds: vec![],
            hosts: vec![],
            experiments: crate::experiments::Experiments::default(),
            gpus: None,
        }
    }

    /// The "Your cloud" experiment on.
    fn your_cloud_on() -> crate::experiments::Experiments {
        crate::experiments::Experiments {
            your_cloud: true,
            ..Default::default()
        }
    }

    fn lume_gpu(supported: bool) -> GpuChoice {
        GpuChoice {
            runtime: "lume".into(),
            id: "paravirtual".into(),
            label: "GPU acceleration".into(),
            experimental: true,
            supported,
            reason: (!supported).then(|| "Needs a Mac with Apple silicon".into()),
            learn_more: Some("https://cua.ai/docs/lume/guides/gpu-passthrough".into()),
        }
    }

    #[test]
    fn the_gpu_row_shows_where_the_runtime_offers_a_gpu() {
        let mut e = env();
        e.local_backends = Some(vec!["docker".into(), "lume".into()]);
        e.gpus = Some(vec![lume_gpu(true)]);
        let s = reduce(
            &initial(&e),
            &WizardAction::ChooseOs { os: SpaceOs::Macos },
            &e,
        );
        let row = view(&s, &e).gpu.expect("a macOS VM on Lume has one");
        assert_eq!(row.label, "GPU acceleration (Experimental)");
        assert!(row.enabled && !row.on && row.reason.is_none());
        assert_eq!(row.learn_more_label, "Learn more");
        assert_eq!(
            row.learn_more_url.as_deref(),
            Some("https://cua.ai/docs/lume/guides/gpu-passthrough")
        );
        assert_eq!(view(&s, &e).plan.gpu, None, "off until checked");
        let s = reduce(&s, &WizardAction::SetGpu { on: true }, &e);
        let v = view(&s, &e);
        assert!(v.gpu.as_ref().unwrap().on);
        assert_eq!(create_args(&v.plan).gpu.as_deref(), Some("paravirtual"));
        assert!(
            v.summary
                .iter()
                .any(|f| f.label == "GPU" && f.value == "GPU acceleration (Experimental)")
        );
        // A Linux container here has no GPU: no row, and nothing passed.
        let linux = reduce(&s, &WizardAction::ChooseOs { os: SpaceOs::Linux }, &e);
        let v = view(&linux, &e);
        assert_eq!((v.gpu, v.plan.gpu), (None, None));
        // Where the host cannot: disabled with the reason, never passed.
        e.gpus = Some(vec![lume_gpu(false)]);
        let v = view(&s, &e);
        let row = v.gpu.unwrap();
        assert!(!row.enabled && !row.on);
        assert_eq!(
            row.reason.as_deref(),
            Some("Needs a Mac with Apple silicon")
        );
        assert_eq!(v.plan.gpu, None);
    }

    fn vol(gb: u64, name: &str) -> StorageVolume {
        StorageVolume {
            available_bytes: gb * GIB,
            total_bytes: 500 * GIB,
            name: name.into(),
        }
    }

    /// This Mac: 212 GB free on the startup disk, 40 GB in Colima.
    fn storage_env() -> WizardEnv {
        WizardEnv {
            cloud_available: true,
            local_backends: None,
            storage: Some(LocalStorage {
                reserve_bytes: 5 * GIB,
                lume: Some(vol(212, "Macintosh HD")),
                qemu: Some(vol(212, "Macintosh HD")),
                container: Some(vol(40, "Colima")),
                pulled: vec![],
            }),
            ..env()
        }
    }

    /// A container image on a Mac whose container engine is not ready says
    /// why (the runtime doctor's detail), not only that it cannot.
    #[test]
    fn a_local_engine_that_is_not_ready_says_why() {
        let image = "ghcr.io/trycua/linux:24.04";
        let mut e = WizardEnv {
            local_backends: Some(vec!["lume".into(), "qemu".into()]),
            ..env()
        };
        let s = reduce(
            &initial(&e),
            &WizardAction::ChooseImage {
                image_ref: image.into(),
            },
            &e,
        );
        let plain = view(&s, &e).placement_error;
        assert_eq!(
            plain.as_deref(),
            Some("This Mac cannot run container Spaces."),
            "{plain:?}"
        );
        e.local_details = Some(
            [(
                "container".to_string(),
                "Docker isn't running; missing: a running container engine; setup would: boot \
                 the managed cua-runtime VM"
                    .to_string(),
            )]
            .into(),
        );
        assert_eq!(
            view(&s, &e).placement_error.as_deref(),
            Some(
                "This Mac cannot run container Spaces: Docker isn't running; missing: a running \
                 container engine."
            )
        );
    }

    fn on(image_ref: &str, e: &WizardEnv) -> WizardState {
        let s = reduce(
            &initial(e),
            &WizardAction::ChooseImage {
                image_ref: image_ref.into(),
            },
            e,
        );
        reduce(&s, &WizardAction::Next, e)
    }

    fn facts(v: &WizardView) -> Vec<(String, String)> {
        v.resource_facts
            .iter()
            .map(|f| (f.label.clone(), f.value.clone()))
            .collect()
    }

    fn sizes(image_ref: &str, arch: &str) -> PlatformSize {
        let i = find_image(image_ref).unwrap();
        platform_size(&i, Some(arch)).unwrap().clone()
    }

    #[test]
    fn sizes_read_like_finder() {
        assert_eq!(size_text(560 << 20), "560 MB");
        assert_eq!(size_text(1012 << 20), "1.0 GB");
        assert_eq!(size_text(1_288_490_189), "1.2 GB");
        assert_eq!(size_text(150 * GIB), "150 GB");
        assert_eq!(size_text(9 * GIB + GIB / 2), "9.5 GB");
        assert_eq!(arch_label("arm64"), "ARM");
        assert_eq!(arch_label("x86_64"), "x64");
    }

    #[test]
    fn every_offered_image_has_sizes() {
        for i in picker_images()
            .into_iter()
            .filter(|i| i.image_ref != "ghcr.io/trycua/macos:15")
        {
            let s = i
                .sizes
                .as_ref()
                .unwrap_or_else(|| panic!("{} has no sizes", i.image_ref));
            assert!(
                !s.platforms.is_empty() && !i.arch.is_empty(),
                "{}",
                i.image_ref
            );
        }
    }

    #[test]
    fn a_container_shows_its_size_and_the_engines_room() {
        let e = storage_env();
        let s = on("ghcr.io/trycua/linux:24.04", &e);
        let v = view(&s, &e);
        let p = sizes("ghcr.io/trycua/linux:24.04", "arm64");
        assert!(!v.disk_editable);
        assert_eq!(
            facts(&v),
            [
                ("Kind".into(), "Container".into()),
                ("Architecture".into(), "ARM".into()),
                (
                    "Disk".into(),
                    format!("{} (download {})", size_text(p.disk), size_text(p.download))
                ),
                ("Available".into(), "40 GB on Colima".into()),
            ]
        );
        assert!(v.resource_facts.iter().all(|f| f.symbol.is_none()));
        assert!(v.can_continue && v.resources_error.is_none());
        assert_eq!(v.plan.disk_gb, None);
        // Pulled already: nothing to download.
        let mut pulled = e.clone();
        pulled.storage.as_mut().unwrap().pulled = vec!["ghcr.io/trycua/linux:24.04".into()];
        let v = view(&s, &pulled);
        assert_eq!(v.resource_facts[2].value, size_text(p.disk));
    }

    #[test]
    fn a_space_that_does_not_fit_blocks_continue() {
        let mut e = storage_env();
        e.storage.as_mut().unwrap().container = Some(vol(3, "Colima"));
        let s = on("ghcr.io/trycua/linux:24.04", &e);
        let v = view(&s, &e);
        let p = sizes("ghcr.io/trycua/linux:24.04", "arm64");
        assert_eq!(
            v.resources_error,
            Some(format!(
                "Not enough space on Colima: needs {}, 3.0 GB available.",
                size_text(p.download + p.unpacked + 5 * GIB)
            ))
        );
        assert!(!v.can_continue);
        assert_eq!(reduce(&s, &WizardAction::Next, &e).step, 1);
        // Pulled, it needs nothing new.
        e.storage.as_mut().unwrap().pulled = vec!["ghcr.io/trycua/linux:24.04".into()];
        assert!(view(&s, &e).can_continue);
    }

    #[test]
    fn a_linux_vm_disk_grows_with_the_slider() {
        let e = storage_env();
        let mut s = on("ghcr.io/trycua/linux:24.04-disk", &e);
        let v = view(&s, &e);
        let p = sizes("ghcr.io/trycua/linux:24.04-disk", "arm64");
        let min = p.disk.div_ceil(GIB) as u32;
        assert!(v.disk_editable);
        assert_eq!((v.min_disk_gb, v.max_disk_gb, v.disk_gb), (min, 500, min));
        assert_eq!(
            v.fields.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
            ["cpus", "memory", "disk"]
        );
        assert_eq!(
            facts(&v),
            [
                ("Kind".into(), "Virtual machine".into()),
                ("Architecture".into(), "ARM".into()),
                ("Download".into(), size_text(p.download)),
                ("Available".into(), "212 GB on Macintosh HD".into()),
            ]
        );
        assert_eq!(v.plan.disk_gb, None, "the image's size is not asked for");
        let note = v.disk_note.clone().unwrap();
        assert!(
            note.starts_with("Uses about ")
                && note.ends_with(&format!("up to {min} GB as the Space fills it.")),
            "{note}"
        );
        assert!(v.disk_reset_label.is_none());
        assert!(v.disk_help.unwrap().contains(&format!("{min} GB")));
        s = reduce(&s, &WizardAction::SetDisk { disk_gb: 64 }, &e);
        let resized = view(&s, &e);
        assert!(
            resized
                .disk_note
                .unwrap()
                .starts_with("The disk will be resized to 64 GB after downloading. Uses about ")
        );
        assert_eq!(resized.disk_reset_label.as_deref(), Some("Reset"));
        assert_eq!(
            view(&reduce(&s, &WizardAction::ResetDisk, &e), &e).disk_gb,
            min,
            "Reset goes back to the image's disk"
        );
        let v = view(&s, &e);
        assert_eq!((v.disk_gb, v.disk_text.as_str()), (64, "64 GB"));
        assert_eq!(create_args(&v.plan).disk_gb, Some(64));
        s = reduce(&s, &WizardAction::SetDisk { disk_gb: 1 }, &e);
        assert_eq!(view(&s, &e).disk_gb, min, "never below the image");
        s = reduce(&s, &WizardAction::SetDisk { disk_gb: 9000 }, &e);
        assert_eq!(view(&s, &e).disk_gb, 500, "never above the volume");
        let summary = view(
            &reduce(
                &reduce(&s, &WizardAction::Next, &e),
                &WizardAction::Next,
                &e,
            ),
            &e,
        )
        .summary;
        assert!(
            summary
                .iter()
                .any(|f| f.label == "Resources" && f.value == "2 cores, 4 GB memory, 500 GB disk"),
            "{summary:?}"
        );
        // Another image starts from its own size; the cloud sizes its own.
        let other = reduce(&s, &WizardAction::Back, &e);
        let other = reduce(
            &other,
            &WizardAction::ChooseImage {
                image_ref: "ghcr.io/trycua/linux:24.04-slim-disk".into(),
            },
            &e,
        );
        assert_eq!(other.disk_gb, None);
        let cloud = in_cloud(&s);
        let v = view(&cloud, &e);
        assert!(!v.disk_editable && v.plan.disk_gb.is_none());
        assert_eq!(
            facts(&v),
            [
                ("Kind".into(), "Virtual machine".into()),
                ("Architecture".into(), "x64".into()),
                (
                    "Disk".into(),
                    size_text(sizes("ghcr.io/trycua/linux:24.04-disk", "amd64").disk)
                ),
            ]
        );
    }

    #[test]
    fn macos_grows_through_lume_and_windows_keeps_its_disk() {
        let e = storage_env();
        let v = view(&on("ghcr.io/trycua/macos:26", &e), &e);
        assert!(v.disk_editable);
        assert_eq!(v.min_disk_gb, 150);
        assert_eq!(
            v.resource_facts.last().unwrap().value,
            "212 GB on Macintosh HD"
        );
        let w = view(&on("ghcr.io/trycua/windows:2022", &e), &e);
        let p = sizes("ghcr.io/trycua/windows:2022", "amd64");
        assert!(!w.disk_editable);
        assert!(w.fields.iter().all(|f| f.id != "disk"));
        assert!(facts(&w).contains(&(
            "Disk".into(),
            format!("{} (download {})", size_text(p.disk), size_text(p.download))
        )));
    }

    #[test]
    fn an_image_without_a_build_for_this_mac_warns_it_is_emulated() {
        let e = storage_env();
        let s = on("ghcr.io/trycua/omarchy:edge", &e);
        let v = view(&s, &e);
        let arch = v.resource_facts.iter().find(|f| f.id == "arch").unwrap();
        assert_eq!(arch.value, "x64");
        assert_eq!(arch.symbol.as_deref(), Some("exclamationmark.triangle"));
        assert_eq!(
            arch.help.as_deref(),
            Some("Emulated on this Mac\u{2019}s ARM processor. Performance may be degraded.")
        );
        let summary = view(
            &reduce(
                &reduce(&s, &WizardAction::Next, &e),
                &WizardAction::Next,
                &e,
            ),
            &e,
        )
        .summary;
        let a = summary.iter().find(|f| f.label == "Architecture").unwrap();
        assert_eq!(a.value, "x64 (emulated)");
        // No warning in the cloud, or on an x64 Mac.
        let cloud = in_cloud(&s);
        assert!(
            view(&cloud, &e)
                .resource_facts
                .iter()
                .all(|f| f.symbol.is_none())
        );
        let intel = WizardEnv {
            host_arch: Some("amd64".into()),
            ..e.clone()
        };
        assert!(
            view(&s, &intel)
                .resource_facts
                .iter()
                .all(|f| f.symbol.is_none())
        );
        // A custom ref: no sizes, no architecture, only the room.
        let custom = reduce(
            &initial(&e),
            &WizardAction::SetImageText {
                text: "ghcr.io/acme/desktop:1".into(),
            },
            &e,
        );
        let v = view(&reduce(&custom, &WizardAction::Next, &e), &e);
        assert_eq!(
            facts(&v),
            [
                ("Kind".into(), "Container".into()),
                ("Available".into(), "40 GB on Colima".into()),
            ]
        );
    }

    #[test]
    fn catalog_is_the_shared_image_list() {
        let images = picker_images();
        assert!(images.iter().all(|i| i.published));
        assert_eq!(images[0].image_ref, "ghcr.io/trycua/linux:24.04");
        assert!(picker_groups().iter().all(|g| !g.images.is_empty()));
    }

    #[test]
    fn runtimes_follow_the_image() {
        let linux = find_image("ghcr.io/trycua/linux:24.04").unwrap();
        assert_eq!(
            runtime_options(&linux, Location::Local),
            [Runtime::Auto, Runtime::Gvisor, Runtime::Runc]
        );
        assert_eq!(runtime_options(&linux, Location::Cloud), [Runtime::Gvisor]);
        let kinds = kind_options(&linux, Location::Local);
        assert_eq!(kinds.len(), 2);
        assert_eq!(kinds[1].image.image_ref, "ghcr.io/trycua/linux:24.04-disk");
    }

    #[test]
    fn walks_to_a_local_plan() {
        let e = env();
        let mut s = initial(&e);
        for a in [
            WizardAction::Next,
            WizardAction::SetCpus { cpus: 4 },
            WizardAction::Next,
            WizardAction::SetName {
                name: "Bad Name".into(),
            },
        ] {
            s = reduce(&s, &a, &e);
        }
        assert!(view(&s, &e).name_invalid);
        s = reduce(&s, &WizardAction::Next, &e);
        assert_eq!(s.step, 2, "an invalid name blocks Continue");
        s = reduce(
            &s,
            &WizardAction::SetName {
                name: "demo".into(),
            },
            &e,
        );
        s = reduce(&s, &WizardAction::Next, &e);
        let v = view(&s, &e);
        assert_eq!(v.primary_label, "Create Space");
        let args = create_args(&v.plan);
        assert_eq!(args.on, "local");
        assert_eq!(args.cpus, Some(4));
        assert_eq!(args.memory_mb, Some(4096));
        assert_eq!(args.name.as_deref(), Some("demo"));
    }

    /// AWS, connected: Linux and its slim tier run on small machines;
    /// Windows and macOS do not.
    fn aws() -> ConnectedCloud {
        let offer =
            |image: &str, supported: bool, machine: &str, usd: f64, reason: &str| CloudOffer {
                image: image.into(),
                kind: "container".into(),
                supported,
                reason: reason.into(),
                machine_type: machine.into(),
                usd_per_hour: usd,
            };
        ConnectedCloud {
            name: "aws".into(),
            title: "AWS".into(),
            label: "AWS \u{b7} us-west-2".into(),
            is_default: false,
            ttl_hours: 8,
            offers: vec![
                offer("linux", true, "t4g.medium", 0.0368, ""),
                offer("linux-slim", true, "t4g.small", 0.0184, ""),
                offer(
                    "linux-vm",
                    false,
                    "",
                    0.0,
                    "Linux VMs need a nested-virtualization machine; not offered yet.",
                ),
                offer(
                    "windows",
                    false,
                    "",
                    0.0,
                    "Windows on AWS is not offered yet.",
                ),
                offer(
                    "macos",
                    false,
                    "",
                    0.0,
                    "macOS on AWS: EC2 Mac, 24 h minimum; not offered.",
                ),
            ],
        }
    }

    /// Fleet's default rates (trycua/cloud `DefaultUsageVCPUHourPriceUSD`,
    /// `DefaultUsageMemoryGiBHourPriceUSD`).
    const RATES: CloudPricing = CloudPricing {
        vcpu_hour_usd: 0.044625,
        memory_gib_hour_usd: 0.0223125,
    };

    fn cloud_env(pricing: Option<CloudPricing>) -> WizardEnv {
        WizardEnv {
            default_location: Location::Cloud,
            cloud_available: true,
            cloud_pricing: pricing,
            ..env()
        }
    }

    #[test]
    fn your_cloud_is_disabled_until_one_is_connected() {
        // A Cua Cloud default still starts on this Mac, and there is no
        // Cua Cloud tile while the apps do not offer it.
        let e = WizardEnv {
            default_location: Location::Cloud,
            cloud_available: true,
            ..env()
        };
        let s = initial(&e);
        assert_eq!(s.placement, Location::Local);
        let v = view(&s, &e);
        let ids: Vec<&str> = v.placements.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(
            ids,
            ["local"],
            "This Mac only: no Cua Cloud, nothing connected"
        );
        assert!(v.placements[0].selected && v.placements[0].enabled);
        assert_eq!(v.placement_id, "local");
        assert!(
            v.fields.iter().all(|f| f.id != "connect-cloud"),
            "Connect a cloud only with the experiment"
        );
        let with = WizardEnv {
            experiments: your_cloud_on(),
            ..e.clone()
        };
        assert!(
            view(&s, &with)
                .fields
                .iter()
                .any(|f| f.id == "connect-cloud")
        );
        let picked = reduce(
            &s,
            &WizardAction::SetPlacement {
                placement: Location::Yours,
            },
            &e,
        );
        assert_eq!(picked.placement, Location::Local);
        let synced = reduce(
            &s,
            &WizardAction::SyncDefault {
                location: Location::Cloud,
            },
            &e,
        );
        assert_eq!(synced.placement, Location::Local);
        let yours_default = WizardEnv {
            default_location: Location::Yours,
            ..e
        };
        assert_eq!(initial(&yours_default).placement, Location::Local);
    }

    #[test]
    fn a_connected_cloud_runs_what_it_offers_with_its_cost_and_lifetime() {
        let e = WizardEnv {
            clouds: vec![aws()],
            experiments: your_cloud_on(),
            ..env()
        };
        let s = initial(&e);
        let v = view(&s, &e);
        let aws_entry = v.placements.iter().find(|o| o.id == "aws").unwrap();
        assert_eq!(aws_entry.label, "AWS \u{b7} us-west-2");
        assert_eq!(aws_entry.group, "clouds");
        assert!(aws_entry.enabled && !aws_entry.selected);
        assert_eq!(aws_entry.detail, "t4g.medium, about $0.04/hour");
        let s = reduce(&s, &WizardAction::ChoosePlacement { on: "aws".into() }, &e);
        assert_eq!(s.placement, Location::Yours);
        let v = view(&s, &e);
        assert!(v.can_continue);
        assert_eq!(v.placement_id, "aws");
        assert!(
            v.fields.iter().all(|f| f.id != "cloud"),
            "each cloud is its own entry: no second menu"
        );
        // Systems and kinds it cannot run are greyed out with why.
        let win = v.os_tiles.iter().find(|t| t.id == "windows").unwrap();
        assert!(!win.enabled);
        assert!(
            win.detail
                .starts_with("Windows on AWS is not offered yet. Ask your agent to deploy the ")
        );
        assert!(
            win.detail
                .ends_with(" image on AWS and add it with cua spaces add.")
        );
        let mac = v.os_tiles.iter().find(|t| t.id == "macos").unwrap();
        assert!(!mac.enabled && mac.detail.contains("24 h minimum"));
        let vm = v.kind_tiles.iter().find(|t| t.id == "vm").unwrap();
        assert!(!vm.enabled && vm.detail.contains("nested-virtualization"));
        // The machine picks the size: no sliders, its facts and cost.
        let s1 = reduce(&s, &WizardAction::Next, &e);
        let v1 = view(&s1, &e);
        assert!(v1.fields.is_empty());
        assert_eq!(v1.price.as_deref(), Some("About $0.04/hour"));
        let facts: Vec<String> = v1
            .resource_facts
            .iter()
            .map(|f| format!("{}={}", f.id, f.value))
            .collect();
        assert_eq!(
            facts,
            [
                "kind=Container",
                "machine=t4g.medium",
                "lifetime=After 8 hours"
            ]
        );
        let summary: Vec<(String, String)> = v1
            .summary
            .iter()
            .map(|f| (f.label.clone(), f.value.clone()))
            .collect();
        assert!(summary.contains(&("Runs on".into(), "AWS \u{b7} us-west-2".into())));
        assert!(summary.contains(&("Cost".into(), "About $0.04/hour".into())));
        assert!(summary.contains(&("Deletes itself".into(), "After 8 hours".into())));
        assert!(!summary.iter().any(|(l, _)| l == "Resources"));
        // Create: on the provider's word, no size.
        let args = create_args(&v1.plan);
        assert_eq!(args.on, "aws");
        assert_eq!(
            (args.cpus, args.memory_mb, args.disk_gb),
            (None, None, None)
        );
    }

    #[test]
    fn several_clouds_are_entries_and_an_unrun_image_says_why() {
        let gcp = ConnectedCloud {
            name: "gcp".into(),
            title: "Google Cloud".into(),
            label: "Google Cloud \u{b7} cua-byoc-test".into(),
            is_default: true,
            ttl_hours: 0,
            offers: vec![CloudOffer {
                image: "linux".into(),
                kind: "container".into(),
                supported: true,
                reason: String::new(),
                machine_type: "e2-medium".into(),
                usd_per_hour: 0.0335,
            }],
        };
        let e = WizardEnv {
            default_location: Location::Yours,
            clouds: vec![aws(), gcp],
            experiments: your_cloud_on(),
            ..env()
        };
        let s = initial(&e);
        assert_eq!(s.placement, Location::Yours);
        let v = view(&s, &e);
        assert_eq!(v.cloud.as_deref(), Some("gcp"), "the default one");
        assert_eq!(v.placement_id, "gcp");
        let clouds: Vec<&str> = v
            .placements
            .iter()
            .filter(|o| o.group == "clouds")
            .map(|o| o.id.as_str())
            .collect();
        assert_eq!(clouds, ["aws", "gcp"]);
        let s = reduce(
            &s,
            &WizardAction::ChooseCloud {
                cloud: "aws".into(),
            },
            &e,
        );
        assert_eq!(view(&s, &e).plan.cloud.as_deref(), Some("aws"));
        let s = reduce(
            &s,
            &WizardAction::ChooseCloud {
                cloud: "nope".into(),
            },
            &e,
        );
        assert_eq!(s.cloud.as_deref(), Some("aws"));
        // Google Cloud does not price the slim tier: it says so.
        let s = reduce(
            &s,
            &WizardAction::ChooseCloud {
                cloud: "gcp".into(),
            },
            &e,
        );
        let slim = catalog()
            .images
            .iter()
            .find(|i| cloud_family(i) == "linux-slim")
            .map(|i| i.image_ref.clone())
            .unwrap();
        let s = reduce(&s, &WizardAction::ChooseImage { image_ref: slim }, &e);
        let v = view(&s, &e);
        assert!(!v.can_continue);
        let err = v.placement_error.as_deref().unwrap();
        assert!(
            err.starts_with("Google Cloud does not run this image. Ask your agent to deploy the ")
        );
        assert!(err.ends_with(" image on Google Cloud and add it with cua spaces add."));
        assert_eq!(ttl_text(0), "Never");
    }

    fn host(id: &str, name: &str, via: &str, online: bool) -> SpaceHost {
        SpaceHost {
            id: id.into(),
            name: name.into(),
            via: via.into(),
            online,
            os: "macos".into(),
            limits: vec![],
        }
    }

    /// A Mac mini on the relay at its two macOS VMs, a Linux box over
    /// Tailscale and an offline studio.
    fn hosts() -> Vec<SpaceHost> {
        vec![
            SpaceHost {
                limits: vec![
                    HostLimit {
                        resource: "spaces".into(),
                        used: 2,
                        limit: 4,
                        reason: String::new(),
                    },
                    HostLimit {
                        resource: "macos_vms".into(),
                        used: 2,
                        limit: 2,
                        reason: "Apple's macOS license allows two macOS VMs per Mac".into(),
                    },
                ],
                ..host("m-mini", "Mac mini", "relay", true)
            },
            SpaceHost {
                os: "linux".into(),
                ..host("lab", "lab", "direct", true)
            },
            host("m-studio", "Studio", "relay", false),
        ]
    }

    fn menu(v: &WizardView) -> Vec<String> {
        v.placements
            .iter()
            .map(|o| {
                format!(
                    "{} [{}] {}{}{}",
                    o.id,
                    o.group,
                    o.label,
                    if o.enabled { "" } else { " (disabled)" },
                    if o.selected { " *" } else { "" }
                )
            })
            .collect()
    }

    /// The "Run on" menu lists This Mac and your machines; your clouds
    /// only with the "Your cloud" experiment. Disabled machines say why.
    #[test]
    fn the_run_on_menu_lists_every_machine_and_clouds_only_with_the_experiment() {
        let mut e = WizardEnv {
            hosts: hosts(),
            clouds: vec![aws()],
            ..env()
        };
        let s = initial(&e);
        let v = view(&s, &e);
        assert_eq!(
            menu(&v),
            [
                "local [this-mac] This Mac *",
                "host:m-mini [hosts] Mac mini",
                "host:lab [hosts] lab",
                "host:m-studio [hosts] Studio (offline) (disabled)",
            ],
            "no cloud without the experiment, though AWS is connected"
        );
        let studio = &v.placements[3];
        assert_eq!(studio.detail, "Studio is offline.");
        assert_eq!(v.placements[1].detail, "Through the Cua relay");
        assert_eq!(v.placements[2].detail, "Over Tailscale or your network");
        // A cloud cannot be chosen while hidden, nor an offline machine.
        let s1 = reduce(&s, &WizardAction::ChoosePlacement { on: "aws".into() }, &e);
        assert_eq!(s1.placement, Location::Local);
        let s1 = reduce(
            &s1,
            &WizardAction::ChoosePlacement {
                on: "host:m-studio".into(),
            },
            &e,
        );
        assert_eq!(s1.placement, Location::Local);
        // With the experiment: the connected cloud, after the machines.
        e.experiments = your_cloud_on();
        let v = view(&s, &e);
        assert_eq!(
            menu(&v).last().map(String::as_str),
            Some("aws [clouds] AWS \u{b7} us-west-2")
        );
        assert_eq!(v.placements.len(), 5);
        // Off again while AWS is chosen: back on this Mac, AWS untouched.
        let on_aws = reduce(&s, &WizardAction::ChoosePlacement { on: "aws".into() }, &e);
        assert_eq!(view(&on_aws, &e).placement_id, "aws");
        e.experiments = Default::default();
        let v = view(&on_aws, &e);
        assert_eq!(v.placement_id, "local");
        assert_eq!(create_args(&v.plan).on, "local");
        assert_eq!(e.clouds.len(), 1, "still connected");
    }

    /// Choosing a machine creates there (`host:<id>`) with its own engine,
    /// no price and no guessed architecture; a machine at its limit for
    /// macOS says so and blocks Continue.
    #[test]
    fn a_machine_of_yours_hosts_the_space() {
        let e = WizardEnv {
            hosts: hosts(),
            local_backends: Some(vec!["docker".into(), "lume".into()]),
            ..env()
        };
        let s = reduce(
            &initial(&e),
            &WizardAction::ChoosePlacement {
                on: "host:lab".into(),
            },
            &e,
        );
        assert_eq!(
            (s.placement, s.host.as_deref()),
            (Location::Host, Some("lab"))
        );
        let v = view(&s, &e);
        assert!(v.can_continue, "{:?}", v.placement_error);
        assert_eq!(v.placement_id, "host:lab");
        assert_eq!(v.runtimes.len(), 1, "the machine picks its engine");
        let s3 = reduce(
            &reduce(&s, &WizardAction::Next, &e),
            &WizardAction::Next,
            &e,
        );
        let s3 = reduce(&s3, &WizardAction::Next, &e);
        let v = view(&s3, &e);
        let fact = |l: &str| {
            v.summary
                .iter()
                .find(|f| f.label == l)
                .map(|f| f.value.clone())
        };
        assert_eq!(fact("Runs on").as_deref(), Some("lab"));
        assert_eq!(fact("Runtime").as_deref(), Some("Container on lab"));
        assert_eq!(fact("Architecture"), None);
        assert_eq!(fact("Cost"), None);
        assert_eq!(v.price, None);
        assert_eq!(v.gpu, None);
        let args = create_args(&v.plan);
        assert_eq!(args.on, "host:lab");
        assert_eq!((args.cpus, args.memory_mb), (Some(2), Some(4096)));
        // macOS: the Linux box is not a Mac, the Mac mini is at its limit.
        let mac = reduce(&s, &WizardAction::ChooseOs { os: SpaceOs::Macos }, &e);
        let v = view(&mac, &e);
        assert!(!v.can_continue);
        assert_eq!(
            v.placement_error.as_deref(),
            Some("lab cannot run macOS: it is not a Mac.")
        );
        let labels: Vec<&str> = v.placements.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "This Mac",
                "Mac mini (at its limit)",
                "lab (not a Mac)",
                "Studio (offline)"
            ]
        );
        assert_eq!(
            v.placements[1].detail,
            "Mac mini is at its limit: Apple's macOS license allows two macOS VMs per Mac."
        );
        // Linux on the Mac mini is fine (two of four Spaces in use).
        let mini = reduce(
            &initial(&e),
            &WizardAction::ChoosePlacement {
                on: "host:m-mini".into(),
            },
            &e,
        );
        assert!(view(&mini, &e).can_continue);
        // The machine leaves the list: this Mac again.
        let gone = WizardEnv {
            hosts: vec![],
            ..e.clone()
        };
        assert_eq!(view(&mini, &gone).placement_id, "local");
    }

    #[test]
    fn cloud_spaces_are_sized_within_the_everyday_range_and_priced() {
        let e = cloud_env(Some(RATES));
        let mut s = in_cloud(&reduce(&initial(&e), &WizardAction::Next, &e));
        let v = view(&s, &e);
        assert_eq!(v.step, 1);
        let ids: Vec<&str> = v.fields.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["cpus", "memory"], "sliders, no disk in the cloud");
        assert_eq!((v.min_cpus, v.max_cpus), CLOUD_CPU_RANGE);
        assert_eq!((v.min_memory_gb, v.max_memory_gb), CLOUD_MEMORY_GB_RANGE);
        // Linux defaults: 2 vCPUs + 4 GB = $0.08925 + $0.08925.
        assert_eq!(v.price.as_deref(), Some("About $0.18/hour"));
        s = reduce(&s, &WizardAction::SetCpus { cpus: 64 }, &e);
        s = reduce(&s, &WizardAction::SetMemory { memory_gb: 1024 }, &e);
        let v = view(&s, &e);
        assert_eq!(
            (v.cpus, v.memory_gb),
            (8, 32),
            "clamped to the everyday cloud range"
        );
        assert_eq!(v.price.as_deref(), Some("About $1.07/hour"));
        s = reduce(&s, &WizardAction::SetCpus { cpus: 0 }, &e);
        s = reduce(&s, &WizardAction::SetMemory { memory_gb: 0 }, &e);
        let v = view(&s, &e);
        assert_eq!((v.cpus, v.memory_gb), (1, 2));
        assert_eq!(v.price.as_deref(), Some("About $0.09/hour"));
        // The plan and the SDK call carry the cloud size.
        let args = create_args(&v.plan);
        assert_eq!(
            (args.cpus, args.memory_mb, args.disk_gb),
            (Some(1), Some(2048), None)
        );
        s.step = 3;
        let v = view(&s, &e);
        let fact = |l: &str| {
            v.summary
                .iter()
                .find(|f| f.label == l)
                .map(|f| f.value.clone())
        };
        assert_eq!(fact("Resources").as_deref(), Some("1 core, 2 GB memory"));
        assert_eq!(fact("Cost").as_deref(), Some("About $0.09/hour"));
    }

    #[test]
    fn no_cloud_rates_no_estimate_and_no_price_locally() {
        let e = cloud_env(None);
        let mut s = in_cloud(&reduce(&initial(&e), &WizardAction::Next, &e));
        assert_eq!(view(&s, &e).price, None, "never a guessed price");
        s.step = 3;
        assert!(view(&s, &e).summary.iter().all(|f| f.label != "Cost"));
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let p = CloudPricing {
                vcpu_hour_usd: bad,
                ..RATES
            };
            assert_eq!(price_text(Some(&p), 2, 4), None, "{bad}");
        }
        let e = WizardEnv {
            cloud_pricing: Some(RATES),
            ..env()
        };
        let s = reduce(&initial(&e), &WizardAction::Next, &e);
        assert_eq!(view(&s, &e).price, None, "no price on your own hardware");
        // A machine added by address is your hardware too.
        let s = reduce(&s, &WizardAction::ShowAddress, &e);
        assert_eq!(view(&s, &e).price, None);
    }

    #[test]
    fn a_size_follows_the_placement_limits() {
        // 16 GB chosen locally is kept in the cloud; 12 vCPUs fit neither
        // this 8-core Mac nor the cloud cap.
        let e = WizardEnv {
            cloud_available: true,
            max_cpus: 12,
            cloud_pricing: Some(RATES),
            ..env()
        };
        let mut s = reduce(&initial(&e), &WizardAction::SetCpus { cpus: 12 }, &e);
        s = reduce(&s, &WizardAction::SetMemory { memory_gb: 16 }, &e);
        assert_eq!((view(&s, &e).cpus, view(&s, &e).memory_gb), (12, 16));
        s = in_cloud(&s);
        let v = view(&s, &e);
        assert_eq!((v.cpus, v.memory_gb), (CLOUD_CPU_RANGE.1, 16));
        assert_eq!(v.plan.cpus, Some(CLOUD_CPU_RANGE.1));
        // 8 x 0.044625 + 16 x 0.0223125 = 0.714.
        assert_eq!(v.price.as_deref(), Some("About $0.71/hour"));
    }

    #[test]
    fn address_shapes() {
        assert!(looks_like_address("10.0.0.5:3211"));
        assert!(looks_like_address("https://host.example/"));
        assert!(looks_like_address("[::1]:3211"));
        assert!(!looks_like_address("host name"));
        assert!(!looks_like_address(""));
        assert!(!looks_like_address("host:abc"));
        assert!(friendly_add_error("status: Unauthenticated").starts_with("The Space rejected"));
    }

    fn act(s: &WizardState, a: WizardAction) -> WizardState {
        reduce(s, &a, &env())
    }

    fn rows(v: &WizardView) -> Vec<String> {
        v.image_field
            .groups
            .iter()
            .flat_map(|g| g.rows.iter().map(|r| r.image_ref.clone()))
            .collect()
    }

    #[test]
    fn image_refs_validate() {
        for ok in [
            "ghcr.io/trycua/linux:24.04",
            "ghcr.io/org/app:tag",
            "ubuntu",
            "library/ubuntu:22.04",
            "localhost:5000/team/app",
            "registry.example.com:443/a/b-c__d.e:v1.2-rc_3",
            "ghcr.io/org/app@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "  ghcr.io/org/app:tag  ",
        ] {
            assert_eq!(validate_image_ref(ok), None, "{ok}");
        }
        let err = |r: &str| validate_image_ref(r).unwrap();
        assert_eq!(err(""), "Enter an image, like ghcr.io/org/app:tag.");
        assert_eq!(
            err("ghcr.io/org/app:tag x"),
            "An image reference has no spaces."
        );
        assert_eq!(
            err("ghcr.io/Org/App:1"),
            "Use lowercase letters in the image name."
        );
        // Any catalog entry that is not published yet (none once all are).
        if let Some(unpublished) = catalog().all.iter().find(|i| !i.published) {
            assert_eq!(
                err(&unpublished.image_ref),
                "This image is not published yet."
            );
        }
        for bad in [
            "ghcr.io/org/app:",
            "ghcr.io//app",
            "ghcr.io/org/app:-tag",
            "ghcr.io/org/-app",
            "ghcr.io/org/app@sha256:xyz",
            "a..b/c",
            ":tag",
            "/app",
        ] {
            assert_eq!(
                err(bad),
                "Not an image reference, like ghcr.io/org/app:tag.",
                "{bad}"
            );
        }
    }

    #[test]
    fn suggestions_filter_by_every_word() {
        let all: usize = image_suggestions("").iter().map(|g| g.images.len()).sum();
        assert_eq!(all, picker_images().len());
        let refs = |q: &str| -> Vec<String> {
            image_suggestions(q)
                .into_iter()
                .flat_map(|g| g.images.into_iter().map(|i| i.image_ref))
                .collect()
        };
        // Derived from the catalog, so publishing an image changes nothing
        // here: a query keeps exactly the published images whose ref or
        // name has every word (any case).
        let expect = |q: &str| -> Vec<String> {
            let words: Vec<String> = q.split_whitespace().map(str::to_lowercase).collect();
            catalog()
                .groups
                .iter()
                .flat_map(|g| g.images.iter())
                .filter(|i| {
                    let hay = format!("{} {}", i.image_ref, i.name).to_lowercase();
                    words.iter().all(|w| hay.contains(w.as_str()))
                })
                .map(|i| i.image_ref.clone())
                .collect()
        };
        assert!(!refs("WINDOWS").is_empty());
        assert_eq!(refs("WINDOWS"), expect("windows"));
        assert!(
            refs("ubuntu vm")
                .iter()
                .all(|r| refs("ubuntu").contains(r) && refs("vm").contains(r))
        );
        assert_eq!(
            refs("ubuntu vm"),
            refs("ubuntu")
                .into_iter()
                .filter(|r| refs("vm").contains(r))
                .collect::<Vec<_>>(),
            "every word must match, the name counts"
        );
        assert!(!refs("ubuntu vm").is_empty());
        assert!(refs("macos").iter().all(|r| r.contains("macos")));
        assert!(refs("no-such-image").is_empty());
        // Only published catalog entries are ever suggested (tiers join once CI publishes them).
        let catalog: serde_json::Value =
            serde_json::from_str(include_str!("../../../../images/sandbox-images.json")).unwrap();
        let published: std::collections::HashSet<String> = catalog["images"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|i| i["published"].as_bool() == Some(true))
            .filter_map(|i| i["ref"].as_str().map(str::to_string))
            .collect();
        for r in refs("slim").iter().chain(refs("").iter()) {
            assert!(published.contains(r), "unpublished image suggested: {r}");
        }
    }

    fn pressed_os(v: &WizardView) -> Vec<String> {
        v.os_tiles
            .iter()
            .filter(|t| t.pressed)
            .map(|t| t.id.clone())
            .collect()
    }

    #[test]
    fn benchmark_images_stay_out_of_the_pickers() {
        assert!(picker_images().iter().all(|i| i.group != "benchmark"));
        assert!(picker_groups().iter().all(|g| g.id != "benchmark"));
    }

    #[test]
    fn image_field_keyboard_and_custom_refs() {
        let s = initial(&env());
        let v = view(&s, &env());
        assert!(!v.image_field.open);
        assert_eq!(v.image_field.text, "ghcr.io/trycua/linux:24.04");
        // Opening shows the selected OS's presets (Linux), the current one
        // highlighted, and an echo of the same text does not start filtering.
        let s = act(&s, WizardAction::OpenImageSuggestions);
        let s = act(
            &s,
            WizardAction::SetImageText {
                text: "ghcr.io/trycua/linux:24.04".into(),
            },
        );
        let v = view(&s, &env());
        assert!(v.image_field.open);
        let linux: Vec<String> = picker_images()
            .into_iter()
            .filter(|i| i.os == SpaceOs::Linux)
            .map(|i| i.image_ref)
            .collect();
        assert_eq!(rows(&v), linux);
        assert!(
            v.image_field.groups[0].rows[0].highlighted && v.image_field.groups[0].rows[0].selected
        );
        assert_eq!(pressed_os(&v), ["linux"]);
        // Down twice, up once, Return: the second preset.
        let mut s = s;
        for d in [1, 1, -1] {
            s = act(&s, WizardAction::MoveImageSuggestion { delta: d });
        }
        s = act(&s, WizardAction::PickImageSuggestion);
        let v = view(&s, &env());
        assert!(!v.image_field.open);
        assert_eq!(v.image.image_ref, "ghcr.io/trycua/linux:24.04-disk");
        assert_eq!(
            v.image_field.text, v.image.image_ref,
            "picking fills the field"
        );
        // Typing is custom: no OS is chosen, every OS's presets filter by
        // the text, nothing is highlighted; Down highlights the first match.
        s = act(&s, WizardAction::SetImageText { text: "win".into() });
        let v = view(&s, &env());
        assert_eq!(rows(&v), ["ghcr.io/trycua/windows:2022"]);
        assert!(pressed_os(&v).is_empty(), "typing unselects the OS");
        assert!(!v.image_field.groups[0].rows[0].highlighted);
        s = act(&s, WizardAction::MoveImageSuggestion { delta: 1 });
        s = act(&s, WizardAction::MoveImageSuggestion { delta: 5 });
        s = act(&s, WizardAction::PickImageSuggestion);
        let v = view(&s, &env());
        assert_eq!(v.image.image_ref, "ghcr.io/trycua/windows:2022");
        assert_eq!(pressed_os(&v), ["windows"], "a preset selects its OS again");
        // Opening now lists only Windows presets.
        let open = act(&s, WizardAction::OpenImageSuggestions);
        assert!(
            rows(&view(&open, &env()))
                .iter()
                .all(|r| r.contains("/windows:"))
        );
        // A malformed ref blocks Continue with one line.
        s = act(
            &s,
            WizardAction::SetImageText {
                text: "ghcr.io/Acme/x".into(),
            },
        );
        let v = view(&s, &env());
        assert!(!v.can_continue);
        assert_eq!(
            v.fields[1].error.as_deref(),
            Some("Use lowercase letters in the image name.")
        );
        // Escape closes.
        s = act(&s, WizardAction::DismissImageSuggestions);
        assert!(!view(&s, &env()).image_field.open);
        // Back to Linux, then a custom ref: used as typed, run like the preset.
        s = act(&s, WizardAction::ChooseOs { os: SpaceOs::Linux });
        s = act(
            &s,
            WizardAction::SetImageText {
                text: "ghcr.io/acme/desktop:1.2".into(),
            },
        );
        s = act(&s, WizardAction::PickImageSuggestion);
        let v = view(&s, &env());
        assert!(v.image_field.custom && v.image_field.error.is_none() && v.can_continue);
        assert_eq!(v.image.image_ref, "ghcr.io/acme/desktop:1.2");
        assert!(pressed_os(&v).is_empty());
        // Opening with a custom ref shows every OS's presets.
        let open = act(&s, WizardAction::OpenImageSuggestions);
        assert_eq!(rows(&view(&open, &env())).len(), picker_images().len());
        assert_eq!(v.image.variant, SpaceKind::Container);
        // Kind keeps the typed ref and switches the engine.
        s = act(
            &s,
            WizardAction::ChooseKind {
                kind: SpaceKind::Vm,
            },
        );
        let v = view(&s, &env());
        assert_eq!(v.image.image_ref, "ghcr.io/acme/desktop:1.2");
        assert_eq!(v.image.local, Some(LocalEngine::Qemu));
        let args = create_args(&v.plan);
        assert_eq!(args.image, "ghcr.io/acme/desktop:1.2");
        assert_eq!(args.kind, SpaceKind::Vm);
        // Typing a preset's ref exactly selects it.
        s = act(
            &s,
            WizardAction::SetImageText {
                text: "ghcr.io/trycua/macos:26".into(),
            },
        );
        let v = view(&s, &env());
        assert!(!v.image_field.custom);
        assert_eq!(v.image.os, SpaceOs::Macos);
    }

    #[test]
    fn fields_per_step_and_address_submit() {
        let e = env();
        let s = initial(&e);
        let ids = |s: &WizardState| -> Vec<String> {
            view(s, &e).fields.into_iter().map(|f| f.id).collect()
        };
        assert_eq!(
            ids(&s),
            [
                "os",
                "image",
                "placement",
                "kind",
                "runtime",
                "connect-by-address"
            ]
        );
        let s1 = reduce(&s, &WizardAction::Next, &e);
        assert_eq!(
            ids(&s1),
            ["cpus", "memory"],
            "a container's disk is its engine's"
        );
        let s2 = reduce(&s1, &WizardAction::Next, &e);
        assert_eq!(ids(&s2), ["name", "open-when-ready"]);
        let mut a = reduce(&s, &WizardAction::ShowAddress, &e);
        assert_eq!(ids(&a), ["address", "token", "address-name"]);
        for act in [
            WizardAction::SetAddress {
                url: " 10.0.0.5:3211 ".into(),
            },
            WizardAction::SetToken { token: "  ".into() },
            WizardAction::SetAddressName {
                name: " lab ".into(),
            },
        ] {
            a = reduce(&a, &act, &e);
        }
        assert_eq!(
            view(&a, &e).address.submit,
            Some(AddressSubmit {
                url: "10.0.0.5:3211".into(),
                token: None,
                name: Some("lab".into())
            })
        );
    }
}
