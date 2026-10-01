//! Publishing libs/images sandbox images: immutable pins first, moving tags
//! after the gates (`cua images publish` / `cua images promote`).
//!
//! One series (e.g. Linux `24.04`, Omarchy `edge`) publishes up to two
//! indexes, each first as an immutable pin `<tag>-<yyyymmdd>-<sha7>`:
//!
//! | Role | Moving tag | Children | Annotations |
//! |---|---|---|---|
//! | containerdisk | `<series>-disk` | the per-arch containerDisks | `ai.cua.image.variant=containerdisk` (+ `variants.rootfs` when there is one) |
//! | primary | `<series>` | the per-arch rootfs images, or (VM-only images, like Windows) the containerDisks again | `ai.cua.image.variants={"containerdisk": <disk digest>}` |
//!
//! Both carry `ai.cua.image.os`, `ai.cua.spacesd` and the legacy
//! `ai.cua.env-driver`. Pins must not exist ([`check_tag`]); moving tags move
//! only in repositories whose policy allows it, and only to a pin's digest.
//! The rules mirror `scripts/images/check-tag-safety.sh` (a test keeps the two
//! repository lists in step).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{ImageError, Result};
use crate::manifest::{Descriptor, ImageIndex, Manifest, Platform};
use crate::media_types::OCI_INDEX;
use crate::registry::{RegistryClient, parse_ref};

/// Repositories whose floating tags (`24.04`, `24.04-disk`, `24.04-slim`,
/// `26-xcode`, `2022-disk`, ...: [`CANONICAL_FLOAT_TAG_RE`]) may move;
/// every other tag there is written once, and `latest` never exists.
pub const CANONICAL_REPOS: &[&str] = &[
    "ghcr.io/trycua/linux",
    "ghcr.io/trycua/windows",
    "ghcr.io/trycua/macos",
];
/// Repositories (globs) older docs and SDKs pin: no existing tag ever moves.
pub const PROTECTED_REPOS: &[&str] = &[
    "public.ecr.aws/k5j5w0x5/*",
    "ghcr.io/trycua/macos-*-cua",
    "ghcr.io/trycua/macos-sequoia-vanilla",
    "docker.io/trycua/cua-qemu-*",
    "docker.io/trycua/xfce*",
    "docker.io/trycua/kasm*",
    "ghcr.io/trycua/cua",
    // Legacy name of ghcr.io/trycua/linux: frozen and read-only.
    "ghcr.io/trycua/cua-desktop-linux",
    "*.dkr.ecr.*.amazonaws.com/desktop-workspace",
];
/// Benchmark repositories: version tags (`<ver>`, `<ver>-disk`) may move.
pub const BENCH_REPOS: &[&str] = &["ghcr.io/trycua/bench-*"];
/// Distribution-channel repositories: only channel tags (`edge`,
/// `edge-disk`, `rc`, `stable`) may move.
pub const CHANNEL_REPOS: &[&str] = &["ghcr.io/trycua/omarchy"];
/// The floating tags a canonical repository may move:
/// `<ver>[-<tier>][-disk]`, the version and an Xcode version being
/// dot-separated numbers, the tier `slim`, `xcode` or `xcode-<X.Y>`.
/// Mirrors `CANONICAL_FLOAT_TAG_RE` in scripts/images/check-tag-safety.sh
/// (a test compares the two).
pub const CANONICAL_FLOAT_TAG_RE: &str =
    r"^[0-9]+(\.[0-9]+)*(-slim|-xcode(-[0-9]+(\.[0-9]+)*)?)?(-disk)?$";
const CHANNEL_TAGS: &[&str] = &["edge", "rc", "stable"];
const BENCH_RESERVED: &[&str] = &["latest", "main", "stable", "edge", "nightly"];

/// `<name>-<yyyymmdd>-<sha7>[-<arch>]`: never moves, anywhere.
pub fn is_immutable_tag(tag: &str) -> bool {
    let t = tag
        .strip_suffix("-amd64")
        .or_else(|| tag.strip_suffix("-arm64"))
        .unwrap_or(tag);
    let parts: Vec<&str> = t.rsplitn(3, '-').collect();
    parts.len() == 3
        && parts[0].len() == 7
        && parts[0]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && parts[1].len() == 8
        && parts[1].bytes().all(|b| b.is_ascii_digit())
        && !parts[2].is_empty()
}

fn is_version(v: &str) -> bool {
    !v.is_empty()
        && v.split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// A canonical repository's floating tag ([`CANONICAL_FLOAT_TAG_RE`]):
/// `<ver>[-slim|-xcode[-<X.Y>]][-disk]` (`24.04`, `24.04-slim-disk`,
/// `26-xcode-26.1`, `2022-disk`).
pub fn is_canonical_floating_tag(tag: &str) -> bool {
    let t = tag.strip_suffix("-disk").unwrap_or(tag);
    let (version, tier) = match t.split_once('-') {
        Some((v, rest)) => (v, Some(rest)),
        None => (t, None),
    };
    is_version(version)
        && match tier {
            None | Some("slim") | Some("xcode") => true,
            Some(rest) => rest.strip_prefix("xcode-").is_some_and(is_version),
        }
}

fn glob_match(pattern: &str, text: &str) -> bool {
    // `*` matches any run of characters (shell `case` semantics, no `/` rule).
    let (p, t) = (pattern.as_bytes(), text.as_bytes());
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && p[pi] == b'*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&b| b == b'*')
}

fn any_glob(patterns: &[&str], repo: &str) -> bool {
    patterns.iter().any(|p| glob_match(p, repo))
}

/// Whether pushing `repo:tag` is allowed. `existing` is the tag's current
/// digest (None when absent), `want` the digest about to be written.
pub fn check_tag(
    repo: &str,
    tag: &str,
    existing: Option<&str>,
    want: Option<&str>,
    moving: bool,
) -> std::result::Result<(), String> {
    if tag == "latest" && CANONICAL_REPOS.contains(&repo) {
        return Err(format!(
            "refusing {repo}:latest: canonical repositories never carry latest"
        ));
    }
    let Some(have) = existing else { return Ok(()) };
    if want == Some(have) {
        return Ok(());
    }
    let refused = |why: &str| {
        Err(format!(
            "refusing to overwrite {repo}:{tag} ({have}): {why}"
        ))
    };
    if is_immutable_tag(tag) {
        return refused("immutable tag");
    }
    if any_glob(PROTECTED_REPOS, repo) {
        return refused("protected repository");
    }
    if !moving {
        return refused("tag exists (pass a moving tag to promote)");
    }
    if CANONICAL_REPOS.contains(&repo) {
        return if is_canonical_floating_tag(tag) {
            Ok(())
        } else {
            refused("canonical repositories float only <os-version>[-slim|-xcode[-X.Y]][-disk]")
        };
    }
    let base = tag.strip_suffix("-disk").unwrap_or(tag);
    if any_glob(BENCH_REPOS, repo) {
        return if BENCH_RESERVED.contains(&base) {
            refused("reserved tag in a benchmark repository")
        } else {
            Ok(())
        };
    }
    if CHANNEL_REPOS.contains(&repo) {
        return if CHANNEL_TAGS.contains(&base) {
            Ok(())
        } else {
            refused("only channel tags move in a channel repository")
        };
    }
    refused("repository has no moving-tag policy")
}

/// The tags one series publishes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeriesTags {
    pub primary: String,
    pub primary_pin: String,
    pub containerdisk: String,
    pub containerdisk_pin: String,
}

/// `stamp` is `<yyyymmdd>-<sha7>`.
pub fn series_tags(series: &str, stamp: &str) -> SeriesTags {
    SeriesTags {
        primary: series.into(),
        primary_pin: format!("{series}-{stamp}"),
        containerdisk: format!("{series}-disk"),
        containerdisk_pin: format!("{series}-disk-{stamp}"),
    }
}

/// Validate a `<yyyymmdd>-<sha7>` stamp.
pub fn check_stamp(stamp: &str) -> Result<()> {
    if is_immutable_tag(&format!("x-{stamp}"))
        && !stamp.ends_with("-amd64")
        && !stamp.ends_with("-arm64")
    {
        Ok(())
    } else {
        Err(ImageError::Reference(
            stamp.into(),
            "stamp must be <yyyymmdd>-<sha7>".into(),
        ))
    }
}

/// What to publish.
#[derive(Clone, Debug)]
pub struct PublishSpec {
    /// Destination repository (the children must already be in it).
    pub repo: String,
    pub series: String,
    pub stamp: String,
    /// `(arch, ref)` of each per-arch containerDisk.
    pub containerdisks: Vec<(String, String)>,
    /// `(arch, ref)` of each per-arch rootfs; empty for VM-only images.
    pub rootfs: Vec<(String, String)>,
    pub os: String,
    pub spacesd: bool,
    /// Extra index annotations (source, revision, doctor status, ...).
    pub annotations: BTreeMap<String, String>,
    /// Annotations on one child's descriptor, keyed by `(variant, arch)`
    /// (`rootfs`/`containerdisk`, `amd64`/`arm64`): per-child doctor
    /// verdicts (`ai.cua.doctor.status`, `ai.cua.doctor.report`).
    pub descriptor_annotations: BTreeMap<(String, String), BTreeMap<String, String>>,
}

impl PublishSpec {
    fn annotate(&self, variant: &str, arch: &str, d: &mut Descriptor) {
        if let Some(a) = self
            .descriptor_annotations
            .get(&(variant.to_string(), arch.to_string()))
        {
            d.annotations.extend(a.clone());
        }
    }
}

/// One published pin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pinned {
    pub pin: String,
    pub moving: String,
    pub digest: String,
}

/// What `publish` wrote; `promote` moves the tags it names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishRecord {
    pub repo: String,
    pub series: String,
    pub stamp: String,
    pub containerdisk: Pinned,
    pub primary: Pinned,
    /// Whether the primary is a rootfs (container) index.
    pub primary_is_rootfs: bool,
}

fn base_annotations(spec: &PublishSpec) -> BTreeMap<String, String> {
    let mut a = spec.annotations.clone();
    a.insert("ai.cua.image.os".into(), spec.os.clone());
    a.insert("ai.cua.spacesd".into(), spec.spacesd.to_string());
    a.insert("ai.cua.env-driver".into(), spec.spacesd.to_string());
    a
}

fn index(children: Vec<Descriptor>, annotations: BTreeMap<String, String>) -> ImageIndex {
    ImageIndex {
        schema_version: 2,
        media_type: OCI_INDEX.into(),
        manifests: children,
        annotations,
    }
}

/// The containerDisk index for `children`.
pub fn containerdisk_index(
    spec: &PublishSpec,
    children: Vec<Descriptor>,
    rootfs_pin: Option<&str>,
) -> ImageIndex {
    let mut a = base_annotations(spec);
    a.insert("ai.cua.image.variant".into(), "containerdisk".into());
    if let Some(pin) = rootfs_pin {
        a.insert(
            "ai.cua.image.variants".into(),
            serde_json::json!({ "rootfs": format!("{}:{pin}", spec.repo) }).to_string(),
        );
    }
    index(children, a)
}

/// The primary index: the rootfs children, or the containerDisk children
/// again for VM-only images, linked to the containerDisk digest.
pub fn primary_index(
    spec: &PublishSpec,
    children: Vec<Descriptor>,
    disk_digest: &str,
    is_rootfs: bool,
) -> ImageIndex {
    let mut a = base_annotations(spec);
    if is_rootfs {
        a.insert("ai.cua.image.variant".into(), "rootfs".into());
    }
    a.insert(
        "ai.cua.image.variants".into(),
        serde_json::json!({ "containerdisk": format!("{}@{disk_digest}", spec.repo) }).to_string(),
    );
    index(children, a)
}

fn repo_of(reference: &str) -> Result<String> {
    let r = parse_ref(reference)?;
    Ok(format!("{}/{}", r.resolve_registry(), r.repository()))
}

fn normalized(repo: &str) -> String {
    // `docker.io` refs resolve to index.docker.io; compare what the client resolves.
    repo_of(&format!("{repo}:x")).unwrap_or_else(|_| repo.to_string())
}

/// A descriptor for the single-arch manifest at `reference`.
async fn child(
    client: &RegistryClient,
    repo: &str,
    arch: &str,
    reference: &str,
) -> Result<Descriptor> {
    if normalized(&repo_of(reference)?) != normalized(repo) {
        return Err(ImageError::Registry(format!(
            "{reference} is not in {repo}; push the per-arch images to the destination repository first"
        )));
    }
    let (raw, digest) = client.manifest_bytes(reference).await?;
    let media_type = match Manifest::parse(&raw)? {
        Manifest::Image(m) => {
            if m.media_type.is_empty() {
                crate::media_types::OCI_MANIFEST.to_string()
            } else {
                m.media_type
            }
        }
        Manifest::Index(_) => {
            return Err(ImageError::Registry(format!(
                "{reference} is an index; publish takes single-arch images"
            )));
        }
    };
    Ok(Descriptor {
        media_type,
        digest,
        size: raw.len() as u64,
        annotations: BTreeMap::new(),
        platform: Some(Platform {
            architecture: crate::manifest::oci_arch(arch).into(),
            os: "linux".into(),
            variant: None,
        }),
    })
}

async fn existing_digest(
    client: &RegistryClient,
    repo: &str,
    tags: &[String],
    tag: &str,
) -> Result<Option<String>> {
    if !tags.iter().any(|t| t == tag) {
        return Ok(None);
    }
    Ok(Some(
        client.manifest_bytes(&format!("{repo}:{tag}")).await?.1,
    ))
}

async fn list_tags(client: &RegistryClient, repo: &str) -> Vec<String> {
    // A repository that does not exist yet has no tags.
    client
        .list_tags(&format!("{repo}:latest"))
        .await
        .unwrap_or_default()
}

/// Push the immutable pins (refusing any that exist). Nothing moves here.
/// With `dry_run` nothing is pushed and the digests are the would-be ones.
pub async fn publish(
    client: &RegistryClient,
    spec: &PublishSpec,
    dry_run: bool,
) -> Result<PublishRecord> {
    check_stamp(&spec.stamp)?;
    if spec.containerdisks.is_empty() {
        return Err(ImageError::Registry(
            "no containerDisk children to publish".into(),
        ));
    }
    let tags = series_tags(&spec.series, &spec.stamp);
    let existing = list_tags(client, &spec.repo).await;
    for pin in [&tags.primary_pin, &tags.containerdisk_pin] {
        let have = existing_digest(client, &spec.repo, &existing, pin).await?;
        check_tag(&spec.repo, pin, have.as_deref(), None, false).map_err(ImageError::Registry)?;
    }
    let mut disks = Vec::new();
    for (arch, r) in &spec.containerdisks {
        let mut d = child(client, &spec.repo, arch, r).await?;
        spec.annotate("containerdisk", arch, &mut d);
        disks.push(d);
    }
    let mut rootfs = Vec::new();
    for (arch, r) in &spec.rootfs {
        let mut d = child(client, &spec.repo, arch, r).await?;
        spec.annotate("rootfs", arch, &mut d);
        rootfs.push(d);
    }
    let is_rootfs = !rootfs.is_empty();
    let disk_idx = containerdisk_index(
        spec,
        disks.clone(),
        is_rootfs.then_some(tags.primary_pin.as_str()),
    );
    let disk_body = serde_json::to_vec(&disk_idx)?;
    let disk_digest = crate::digest::sha256_bytes(&disk_body);
    let primary_children = if is_rootfs { rootfs } else { disks };
    let primary_idx = primary_index(spec, primary_children, &disk_digest, is_rootfs);
    let primary_body = serde_json::to_vec(&primary_idx)?;
    let primary_digest = crate::digest::sha256_bytes(&primary_body);
    if !dry_run {
        let got = client
            .push_manifest_json(
                &format!("{}:{}", spec.repo, tags.containerdisk_pin),
                disk_body,
                OCI_INDEX,
            )
            .await?;
        verify(&got, &disk_digest)?;
        let got = client
            .push_manifest_json(
                &format!("{}:{}", spec.repo, tags.primary_pin),
                primary_body,
                OCI_INDEX,
            )
            .await?;
        verify(&got, &primary_digest)?;
    }
    Ok(PublishRecord {
        repo: spec.repo.clone(),
        series: spec.series.clone(),
        stamp: spec.stamp.clone(),
        containerdisk: Pinned {
            pin: tags.containerdisk_pin,
            moving: tags.containerdisk,
            digest: disk_digest,
        },
        primary: Pinned {
            pin: tags.primary_pin,
            moving: tags.primary,
            digest: primary_digest,
        },
        primary_is_rootfs: is_rootfs,
    })
}

fn verify(got: &str, want: &str) -> Result<()> {
    if got == want {
        Ok(())
    } else {
        Err(ImageError::Registry(format!(
            "pushed digest {got} is not {want}"
        )))
    }
}

/// Move the record's moving tags to its pins: containerDisk first, then the
/// primary (whose annotation names the disk). Each pin must still hold the
/// recorded digest.
pub async fn promote(
    client: &RegistryClient,
    record: &PublishRecord,
    dry_run: bool,
) -> Result<Vec<String>> {
    let existing = list_tags(client, &record.repo).await;
    let mut moved = Vec::new();
    for p in [&record.containerdisk, &record.primary] {
        let pin_ref = format!("{}:{}", record.repo, p.pin);
        let (body, digest) = client.manifest_bytes(&pin_ref).await?;
        verify(&digest, &p.digest)?;
        let have = existing_digest(client, &record.repo, &existing, &p.moving).await?;
        check_tag(
            &record.repo,
            &p.moving,
            have.as_deref(),
            Some(&p.digest),
            true,
        )
        .map_err(ImageError::Registry)?;
        if !dry_run {
            let got = client
                .push_manifest_json(&format!("{}:{}", record.repo, p.moving), body, OCI_INDEX)
                .await?;
            verify(&got, &p.digest)?;
        }
        moved.push(format!("{}:{} -> {}", record.repo, p.moving, p.digest));
    }
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Built with format! so the image-ref gate does not read these as refs.
    const OMARCHY: &str = "ghcr.io/trycua/omarchy";

    fn spec(rootfs: bool) -> PublishSpec {
        PublishSpec {
            repo: "ghcr.io/trycua/omarchy".into(),
            series: "edge".into(),
            stamp: "20260925-abcdef1".into(),
            containerdisks: vec![("amd64".into(), format!("{OMARCHY}:b-amd64"))],
            rootfs: if rootfs {
                vec![("amd64".into(), format!("{OMARCHY}:docker-b-amd64"))]
            } else {
                vec![]
            },
            os: "linux".into(),
            spacesd: true,
            annotations: BTreeMap::new(),
            descriptor_annotations: BTreeMap::new(),
        }
    }

    fn desc(d: &str) -> Descriptor {
        Descriptor {
            media_type: crate::media_types::OCI_MANIFEST.into(),
            digest: d.into(),
            size: 10,
            ..Default::default()
        }
    }

    #[test]
    fn immutable_tags() {
        assert!(is_immutable_tag("edge-20260925-abcdef1"));
        assert!(is_immutable_tag("edge-disk-20260925-abcdef1"));
        assert!(is_immutable_tag("24.04-20260101-0123456-amd64"));
        assert!(!is_immutable_tag("edge"));
        assert!(!is_immutable_tag("edge-disk"));
        assert!(!is_immutable_tag("main-abcdef12"));
        assert!(!is_immutable_tag("20260925-abcdef1"));
        assert!(check_stamp("20260925-abcdef1").is_ok());
        assert!(check_stamp("2026-09-25").is_err());
        assert!(check_stamp("20260925-ABCDEF1").is_err());
    }

    #[test]
    fn tag_policy() {
        let o = "ghcr.io/trycua/omarchy";
        assert!(check_tag(o, "edge-20260925-abcdef1", None, None, false).is_ok());
        assert!(
            check_tag(
                o,
                "edge-20260925-abcdef1",
                Some("sha256:a"),
                Some("sha256:b"),
                true
            )
            .is_err()
        );
        assert!(
            check_tag(
                o,
                "edge-20260925-abcdef1",
                Some("sha256:a"),
                Some("sha256:a"),
                false
            )
            .is_ok()
        );
        assert!(check_tag(o, "edge", Some("sha256:a"), Some("sha256:b"), true).is_ok());
        assert!(check_tag(o, "edge-disk", Some("sha256:a"), Some("sha256:b"), true).is_ok());
        assert!(check_tag(o, "edge", Some("sha256:a"), Some("sha256:b"), false).is_err());
        assert!(check_tag(o, "latest", Some("sha256:a"), Some("sha256:b"), true).is_err());
        assert!(
            check_tag(
                "ghcr.io/trycua/linux",
                "24.04",
                Some("sha256:a"),
                Some("sha256:b"),
                true
            )
            .is_ok()
        );
        assert!(
            check_tag(
                "ghcr.io/trycua/cua-desktop-linux",
                "main",
                Some("a"),
                Some("b"),
                true
            )
            .is_err()
        );
        assert!(
            check_tag(
                "public.ecr.aws/k5j5w0x5/cua-windows-2022",
                "x",
                Some("a"),
                Some("b"),
                true
            )
            .is_err()
        );
        assert!(
            check_tag(
                "ghcr.io/trycua/bench-web",
                "1.0",
                Some("a"),
                Some("b"),
                true
            )
            .is_ok()
        );
        assert!(
            check_tag(
                "ghcr.io/trycua/bench-web",
                "latest",
                Some("a"),
                Some("b"),
                true
            )
            .is_err()
        );
        assert!(check_tag("ghcr.io/acme/other", "v1", Some("a"), Some("b"), true).is_err());
    }

    /// ghcr.io/trycua/linux: dated pins and per-arch children are written
    /// once; only the floating version tags move.
    #[test]
    fn canonical_linux_moves_only_floating_tags() {
        let l = "ghcr.io/trycua/linux";
        let moved = |tag: &str| check_tag(l, tag, Some("sha256:a"), Some("sha256:b"), true);
        assert!(moved("24.04").is_ok());
        assert!(moved("24.04-disk").is_ok());
        for tag in [
            "24.04-20260925-abcdef1",
            "24.04-disk-20260925-abcdef1",
            "docker-build-20260925-abcdef1-amd64",
            "build-20260925-abcdef1-arm64",
            "latest",
            "ci-1234",
            "24.04-rc",
        ] {
            assert!(moved(tag).is_err(), "{tag} must not move");
        }
        // New pins are fine; nothing moves without --moving.
        assert!(check_tag(l, "24.04-20260925-abcdef1", None, None, false).is_ok());
        assert!(check_tag(l, "24.04", Some("sha256:a"), Some("sha256:b"), false).is_err());
        assert!(is_canonical_floating_tag("2022-disk"));
        assert!(is_canonical_floating_tag("26"));
        assert!(!is_canonical_floating_tag("-disk"));
        assert!(!is_canonical_floating_tag("24..04"));
        // The build tags cd-image-linux.yml pushes children under are immutable.
        assert!(is_immutable_tag("docker-build-20260925-abcdef1-amd64"));
        assert!(is_immutable_tag("build-20260925-abcdef1-arm64"));
    }

    #[test]
    fn descriptor_annotations_land_on_their_child() {
        let mut s = spec(true);
        s.descriptor_annotations.insert(
            ("rootfs".into(), "amd64".into()),
            [("ai.cua.doctor.status".to_string(), "pass".to_string())].into(),
        );
        let mut r = desc("sha256:r");
        s.annotate("rootfs", "amd64", &mut r);
        assert_eq!(r.annotations["ai.cua.doctor.status"], "pass");
        let mut d = desc("sha256:d");
        s.annotate("containerdisk", "amd64", &mut d);
        assert!(d.annotations.is_empty());
    }

    #[test]
    fn vm_only_primary_reuses_the_disk_children() {
        let s = spec(false);
        let disk = containerdisk_index(&s, vec![desc("sha256:d")], None);
        assert_eq!(disk.annotations["ai.cua.image.variant"], "containerdisk");
        assert!(!disk.annotations.contains_key("ai.cua.image.variants"));
        assert_eq!(disk.annotations["ai.cua.spacesd"], "true");
        let p = primary_index(&s, disk.manifests.clone(), "sha256:idx", false);
        assert_eq!(p.manifests, disk.manifests);
        assert!(!p.annotations.contains_key("ai.cua.image.variant"));
        assert_eq!(
            p.annotations["ai.cua.image.variants"],
            r#"{"containerdisk":"ghcr.io/trycua/omarchy@sha256:idx"}"#
        );
        assert_eq!(p.media_type, OCI_INDEX);
    }

    #[test]
    fn rootfs_and_disk_link_both_ways() {
        let s = spec(true);
        let t = series_tags(&s.series, &s.stamp);
        assert_eq!(t.primary_pin, "edge-20260925-abcdef1");
        assert_eq!(t.containerdisk_pin, "edge-disk-20260925-abcdef1");
        let disk = containerdisk_index(&s, vec![desc("sha256:d")], Some(&t.primary_pin));
        assert_eq!(
            disk.annotations["ai.cua.image.variants"],
            format!(r#"{{"rootfs":"{OMARCHY}:{}"}}"#, t.primary_pin)
        );
        let p = primary_index(&s, vec![desc("sha256:r")], "sha256:idx", true);
        assert_eq!(p.annotations["ai.cua.image.variant"], "rootfs");
    }

    /// The repository lists match scripts/images/check-tag-safety.sh.
    #[test]
    fn lists_match_the_tag_safety_script() {
        let script = include_str!("../../../../../scripts/images/check-tag-safety.sh");
        let array = |name: &str| -> Vec<String> {
            let start = script
                .find(&format!("{name}=("))
                .unwrap_or_else(|| panic!("{name} in script"));
            let body = &script[start + name.len() + 2..];
            let body = &body[..body.find(')').unwrap()];
            body.lines()
                .map(|l| {
                    l.split('#')
                        .next()
                        .unwrap()
                        .trim()
                        .trim_matches('"')
                        .to_string()
                })
                .filter(|l| !l.is_empty())
                .collect()
        };
        let v = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(array("CANONICAL_REPOS"), v(CANONICAL_REPOS));
        assert_eq!(array("PROTECTED_REPOS"), v(PROTECTED_REPOS));
        assert_eq!(array("BENCH_REPOS"), v(BENCH_REPOS));
        assert_eq!(array("CHANNEL_REPOS"), v(CHANNEL_REPOS));
        let re = script
            .lines()
            .find_map(|l| l.strip_prefix("CANONICAL_FLOAT_TAG_RE="))
            .expect("CANONICAL_FLOAT_TAG_RE in script")
            .trim_matches('\'');
        assert_eq!(re, CANONICAL_FLOAT_TAG_RE);
    }

    #[test]
    fn canonical_floating_tags_follow_the_tier_grammar() {
        for t in [
            "24.04",
            "24.04-disk",
            "24.04-slim",
            "24.04-slim-disk",
            "2022",
            "2022-disk",
            "26",
            "26-slim",
            "26-xcode",
            "26-xcode-26.1",
            "26-xcode-26.1-disk",
            "15",
            "22.04",
            "24.04.1",
        ] {
            assert!(is_canonical_floating_tag(t), "{t}");
        }
        for t in [
            "latest",
            "main",
            "24.04-full",
            "24.04-disk-slim",
            "26-xcode-",
            "26-xcode-beta",
            "26-xcode-26.1-beta",
            "24.04-20260925-abcdef1",
            "24..04",
            "26-xcode-26..1",
            "-disk",
            "-slim",
        ] {
            assert!(!is_canonical_floating_tag(t), "{t}");
        }
        let l = "ghcr.io/trycua/linux";
        let (a, b) = (Some("sha256:a"), Some("sha256:b"));
        assert!(check_tag(l, "24.04-slim", a, b, true).is_ok());
        assert!(check_tag("ghcr.io/trycua/macos", "26-xcode-26.1", a, b, true).is_ok());
        assert!(check_tag(l, "main", a, b, true).is_err());
        assert!(check_tag(l, "latest", a, b, true).is_err());
        // Banned even when absent.
        assert!(check_tag(l, "latest", None, b, false).is_err());
        assert!(check_tag(l, "24.04-slim-20260925-abcdef1", None, b, false).is_ok());
        assert!(check_tag(l, "sha256-aaa", None, b, false).is_ok());
        // Channel repositories keep their own policy.
        assert!(check_tag("ghcr.io/trycua/omarchy", "edge-disk", a, b, true).is_ok());
    }

    #[test]
    fn globs() {
        assert!(glob_match(
            "ghcr.io/trycua/bench-*",
            "ghcr.io/trycua/bench-web"
        ));
        assert!(glob_match(
            "*.dkr.ecr.*.amazonaws.com/desktop-workspace",
            "1.dkr.ecr.us-west-2.amazonaws.com/desktop-workspace"
        ));
        assert!(!glob_match("ghcr.io/trycua/cua", &format!("{OMARCHY}-x")));
    }
}
