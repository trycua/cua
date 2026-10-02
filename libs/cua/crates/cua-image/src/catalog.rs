//! The shared image catalog, `libs/images/sandbox-images.json` (the list the
//! docs, the CLI, the Spaces apps and the samples show), embedded at build
//! time. The canonical constructors consult it so an image that CI has not
//! published yet (`published: false`) fails with
//! [`ImageError::NotPublished`] instead of a registry 404 at pull time.
//!
//! [`catalog_os`] answers the guest OS of a reference: the exact reference
//! first, then the repository (any tag, variant or digest) when every entry
//! of that repository names the same OS. The canonical repositories and
//! their `CUA_IMAGE_<OS>` overrides count as catalog entries too.

use std::sync::OnceLock;

use serde::Deserialize;

use crate::canonical::{self, CanonicalOs};
use crate::error::{ImageError, Result};
use crate::resolve::NormalizedRef;

/// The catalog file, as built into this crate.
pub const CATALOG_JSON: &str = include_str!("../../../../images/sandbox-images.json");

/// One catalog entry (the fields the resolver needs).
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct CatalogEntry {
    /// Full reference (`ghcr.io/trycua/linux:24.04-slim`).
    #[serde(rename = "ref")]
    pub reference: String,
    /// `canonical`, `benchmark` or `distro`.
    pub group: String,
    /// Guest OS.
    pub os: String,
    /// `slim`, `full` or `xcode` (canonical images only).
    #[serde(default)]
    pub tier: Option<String>,
    /// Whether CI published it (only published entries resolve by default).
    #[serde(default)]
    pub published: bool,
    /// The guest's distribution.
    #[serde(default)]
    pub distro: Option<Distro>,
    /// The published digest the moving tag was verified at.
    #[serde(default)]
    pub digest: Option<String>,
    /// Local engine (`container`, `qemu`, `lume`).
    #[serde(default)]
    pub local: Option<String>,
    /// Sizes measured at [`CatalogSizes::digest`]
    /// (`scripts/images/record-image-sizes.py`).
    #[serde(default)]
    pub sizes: Option<CatalogSizes>,
}

/// The guest distribution a catalog entry names (`distro`).
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Distro {
    /// `ubuntu`, `omarchy`, `macos`, `windows`: picks the OS icon.
    pub id: String,
    /// Pretty name ("Ubuntu 24.04", "Omarchy").
    pub name: String,
}

/// How big an image is, per platform.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct CatalogSizes {
    /// The digest these were measured at.
    pub digest: String,
    /// One row per platform.
    pub platforms: Vec<PlatformSize>,
}

/// One platform's sizes, in bytes.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct PlatformSize {
    /// `amd64` or `arm64`.
    pub arch: String,
    /// The platform manifest's digest.
    pub manifest: String,
    /// What a pull downloads.
    pub download: u64,
    /// What the pulled image takes on the host.
    pub unpacked: u64,
    /// The disk the guest sees.
    pub disk: u64,
}

impl CatalogSizes {
    /// The platform for `arch` (`amd64`/`arm64`), else the first one (an
    /// image with no build for this host runs emulated).
    pub fn platform(&self, arch: &str) -> Option<&PlatformSize> {
        self.platforms
            .iter()
            .find(|p| p.arch == arch)
            .or_else(|| self.platforms.first())
    }
}

#[derive(Deserialize)]
struct Catalog {
    images: Vec<CatalogEntry>,
}

/// Every catalog entry, in file order.
pub fn entries() -> &'static [CatalogEntry] {
    static ENTRIES: OnceLock<Vec<CatalogEntry>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        serde_json::from_str::<Catalog>(CATALOG_JSON)
            .expect("sandbox-images.json parses")
            .images
    })
}

/// The entry for `reference`, if the catalog lists it.
pub fn find(reference: &str) -> Option<&'static CatalogEntry> {
    entries().iter().find(|e| e.reference == reference)
}

/// `Ok(reference)` unless the catalog lists it as unpublished. References
/// the catalog does not list pass through (other versions, local builds).
pub fn require_published(reference: String) -> Result<String> {
    match find(&reference) {
        Some(e) if !e.published => Err(ImageError::NotPublished { reference }),
        _ => Ok(reference),
    }
}

/// `(ref, os)` of every catalog entry, for [`catalog_os`].
fn os_pairs() -> &'static [(String, String)] {
    static PAIRS: OnceLock<Vec<(String, String)>> = OnceLock::new();
    PAIRS.get_or_init(|| {
        entries()
            .iter()
            .map(|e| (e.reference.clone(), e.os.clone()))
            .collect()
    })
}

/// The distribution the catalog records for `reference`: the exact
/// reference, else the entries of its repository the reference pins (a
/// digest: every entry; a dated pin such as `24.04-slim-20260926-abc1234`:
/// the tags it extends) when they all name the same one. `None` for images
/// the catalog does not list, or another version of a listed one.
pub fn catalog_distro(reference: &str) -> Option<&'static Distro> {
    catalog_distro_in(reference, entries())
}

/// [`catalog_distro`] over explicit entries (tests).
pub fn catalog_distro_in<'a>(reference: &str, entries: &'a [CatalogEntry]) -> Option<&'a Distro> {
    let wanted = NormalizedRef::parse(reference.trim()).ok()?;
    let listed = || {
        entries
            .iter()
            .filter_map(|e| Some((NormalizedRef::parse(&e.reference).ok()?, e)))
    };
    if let Some((_, e)) = listed().find(|(n, _)| n.full() == wanted.full()) {
        return e.distro.as_ref();
    }
    let pins = |n: &NormalizedRef| match (&wanted.tag, &n.tag) {
        (None, _) => true,
        (Some(t), Some(listed)) => t.starts_with(&format!("{listed}-")),
        (Some(_), None) => false,
    };
    let mut candidates = listed()
        .filter(|(n, _)| n.repo() == wanted.repo() && pins(n))
        .map(|(_, e)| e.distro.as_ref());
    let first = candidates.next()??;
    candidates.all(|d| d == Some(first)).then_some(first)
}

/// The guest OS (`linux`, `windows`, `macos`) the catalog records for
/// `reference`, or `None` when the image is not in the catalog.
pub fn catalog_os(reference: &str) -> Option<String> {
    catalog_os_with(reference, os_pairs(), &|n| std::env::var(n).ok())
}

/// [`catalog_os`] over explicit entries and environment (tests).
pub fn catalog_os_with(
    reference: &str,
    entries: &[(String, String)],
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let reference = reference.trim();
    if let Some((_, os)) = canonical::alias_with(reference, env) {
        return Some(os.as_str().to_string());
    }
    let parsed = NormalizedRef::parse(reference).ok()?;
    let full = parsed.full();
    if let Some((_, os)) = entries.iter().find(|(r, _)| {
        NormalizedRef::parse(r).ok().map(|n| n.full()).as_deref() == Some(full.as_str())
    }) {
        return Some(os.clone());
    }
    let repo = parsed.repo();
    let same_repo: Vec<&String> = entries
        .iter()
        .filter(|(r, _)| {
            NormalizedRef::parse(r).ok().map(|n| n.repo()).as_deref() == Some(repo.as_str())
        })
        .map(|(_, os)| os)
        .collect();
    if let Some(first) = same_repo.first()
        && same_repo.iter().all(|os| os == first)
    {
        return Some((*first).clone());
    }
    for os in [CanonicalOs::Linux, CanonicalOs::Windows, CanonicalOs::Macos] {
        let canonical_repo = canonical::canonical_for_with(os, None, &|_| None);
        let overridden = canonical::env_override_with(os, env);
        let matches = |r: &str| {
            NormalizedRef::parse(r).ok().map(|n| n.repo()).as_deref() == Some(repo.as_str())
        };
        if matches(&canonical_repo) || overridden.as_deref().is_some_and(matches) {
            return Some(os.as_str().to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_gates() {
        assert!(!entries().is_empty());
        let l = find("ghcr.io/trycua/linux:24.04").expect("linux listed");
        assert_eq!(l.tier.as_deref(), Some("full"));
        assert!(require_published("ghcr.io/trycua/linux:24.04".into()).is_ok());
        assert!(require_published("localhost:5000/x:1".into()).is_ok());
        let e = require_published("ghcr.io/trycua/omarchy:edge".into());
        if find("ghcr.io/trycua/omarchy:edge").is_some_and(|e| !e.published) {
            assert!(matches!(e, Err(ImageError::NotPublished { .. })), "{e:?}");
        }
    }

    #[test]
    fn every_entry_names_its_distro() {
        for e in entries() {
            let d = e
                .distro
                .as_ref()
                .unwrap_or_else(|| panic!("{}", e.reference));
            assert!(!d.id.is_empty() && !d.name.is_empty(), "{}", e.reference);
        }
        let name = |r: &str| catalog_distro(r).map(|d| (d.id.as_str(), d.name.as_str()));
        assert_eq!(
            name("ghcr.io/trycua/omarchy:edge"),
            Some(("omarchy", "Omarchy"))
        );
        assert_eq!(
            name("ghcr.io/trycua/linux:24.04-slim-disk"),
            Some(("ubuntu", "Ubuntu 24.04"))
        );
        // A digest of a listed repository.
        let pinned = format!("ghcr.io/trycua/omarchy@sha256:{}", "a".repeat(64));
        assert_eq!(name(&pinned), Some(("omarchy", "Omarchy")));
        // A repository whose entries disagree (macOS 26 and 15) pinned by
        // digest: unknown.
        let mac = format!("ghcr.io/trycua/macos@sha256:{}", "a".repeat(64));
        assert_eq!(name(&mac), None);
        assert_eq!(name("registry.example/app:1"), None);
    }

    #[test]
    fn a_dated_pin_names_its_tag_s_distro_another_version_none() {
        let entry = |r: &str, id: &str, n: &str| CatalogEntry {
            reference: r.into(),
            group: "canonical".into(),
            os: "linux".into(),
            tier: None,
            published: true,
            digest: None,
            local: None,
            sizes: None,
            distro: Some(Distro {
                id: id.into(),
                name: n.into(),
            }),
        };
        let list = [
            entry("registry.example/desk:26", "desk", "Desk 26"),
            entry("registry.example/desk:26-slim", "desk", "Desk 26"),
            entry("registry.example/desk:15", "desk", "Desk 15"),
        ];
        let name = |r: &str| catalog_distro_in(r, &list).map(|d| d.name.as_str());
        assert_eq!(
            name("registry.example/desk:26-slim-20260926-7328110"),
            Some("Desk 26")
        );
        assert_eq!(
            name("registry.example/desk:15-20260101-abcdef0"),
            Some("Desk 15")
        );
        assert_eq!(name("registry.example/desk:14"), None, "another version");
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn built_in_catalog_names_the_canonical_oses() {
        assert!(!os_pairs().is_empty());
        assert_eq!(
            catalog_os("ghcr.io/trycua/windows:2022").as_deref(),
            Some("windows")
        );
        let pinned = format!("ghcr.io/trycua/windows@sha256:{}", "a".repeat(64));
        assert_eq!(
            catalog_os_with(&pinned, os_pairs(), &no_env).as_deref(),
            Some("windows")
        );
        assert_eq!(
            catalog_os_with("ghcr.io/trycua/linux:24.04-disk", os_pairs(), &no_env).as_deref(),
            Some("linux")
        );
        assert_eq!(
            catalog_os_with("windows", os_pairs(), &no_env).as_deref(),
            Some("windows")
        );
        assert_eq!(
            catalog_os_with("registry.example/app:1", os_pairs(), &no_env),
            None
        );
    }

    #[test]
    fn exact_ref_then_repo_then_override() {
        let entries = vec![
            (
                "registry.example/desk:2022".to_string(),
                "windows".to_string(),
            ),
            ("registry.example/mixed:a".to_string(), "linux".to_string()),
            (
                "registry.example/mixed:b".to_string(),
                "windows".to_string(),
            ),
        ];
        // A ref without "windows" in it is Windows because the catalog says so.
        assert_eq!(
            catalog_os_with("registry.example/desk:2022", &entries, &no_env).as_deref(),
            Some("windows")
        );
        assert_eq!(
            catalog_os_with("registry.example/desk:other", &entries, &no_env).as_deref(),
            Some("windows")
        );
        // A repository whose entries disagree decides only by exact ref.
        assert_eq!(
            catalog_os_with("registry.example/mixed:b", &entries, &no_env).as_deref(),
            Some("windows")
        );
        assert_eq!(
            catalog_os_with("registry.example/mixed:c", &entries, &no_env),
            None
        );
        // The CUA_IMAGE_WINDOWS override is a Windows image.
        let env =
            |n: &str| (n == "CUA_IMAGE_WINDOWS").then(|| "registry.example/win:1".to_string());
        assert_eq!(
            catalog_os_with("registry.example/win:2", &[], &env).as_deref(),
            Some("windows")
        );
    }
}
