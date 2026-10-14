//! The canonical cua images and their aliases: one public ghcr repository
//! per OS, one tag per OS version. Every SDK (Python `Image.linux()`, the
//! `cua` bindings, the CLI's `linux`/`windows`/`macos` aliases, Spaces)
//! resolves its defaults here, then through [`crate::resolve`], which picks
//! the variant (`-disk` containerDisk, Lume, ...) a backend runs.
//!
//! | OS | default | other versions |
//! |---|---|---|
//! | Linux | `ghcr.io/trycua/linux:24.04` | `ghcr.io/trycua/linux:<v>` |
//! | Windows | `ghcr.io/trycua/windows:2022` | `ghcr.io/trycua/windows:<v>` |
//! | macOS | `ghcr.io/trycua/macos:26` (tahoe) | `macos:15` (sequoia), `macos:<v>` |
//!
//! Tiers (`<os-version>[-<tier>][-disk]`): no suffix is the full default
//! (dev tooling), `-slim` the minimum that passes `cua-spacesd doctor
//! --strict` with every feature (CI and benchmarks use it), and on macOS
//! `-xcode[-X.Y]` adds one pinned Xcode with its iOS simulator runtime.
//! [`tier_image`] builds them; entries the catalog marks unpublished fail
//! with [`ImageError::NotPublished`]. [`omarchy_image`] names the Omarchy
//! distribution image (`ghcr.io/trycua/omarchy:edge`, an amd64 VM).
//!
//! One override per OS: `CUA_IMAGE_LINUX`, `CUA_IMAGE_WINDOWS`,
//! `CUA_IMAGE_MACOS` (replaces the default version only). The older names
//! `CUA_DEFAULT_LINUX_IMAGE`, `CUA_SANDBOX_LINUX_CONTAINER_IMAGE`,
//! `CUA_DEFAULT_WINDOWS_IMAGE` and `CUA_DEFAULT_MACOS_IMAGE` are accepted as
//! deprecated aliases.

use crate::error::{ImageError, Result};

/// Canonical Linux repository.
pub const LINUX_REPO: &str = "ghcr.io/trycua/linux";
/// The Omarchy distribution repository (channel tags: `edge`, `rc`, `stable`).
pub const OMARCHY_REPO: &str = "ghcr.io/trycua/omarchy";
/// Default Omarchy channel.
pub const OMARCHY_DEFAULT_CHANNEL: &str = "edge";
/// Canonical Windows repository.
pub const WINDOWS_REPO: &str = "ghcr.io/trycua/windows";
/// Canonical macOS repository.
pub const MACOS_REPO: &str = "ghcr.io/trycua/macos";

/// Default Linux version tag.
pub const LINUX_DEFAULT_VERSION: &str = "24.04";
/// Default Windows version tag.
pub const WINDOWS_DEFAULT_VERSION: &str = "2022";
/// Default macOS version tag (tahoe).
pub const MACOS_DEFAULT_VERSION: &str = "26";

/// A guest OS with a canonical image.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CanonicalOs {
    Linux,
    Windows,
    Macos,
}

impl CanonicalOs {
    /// Parses `linux`/`ubuntu`, `windows`, `macos`/`darwin`/`osx`.
    pub fn parse(os: &str) -> Result<Self> {
        match os.trim().to_ascii_lowercase().as_str() {
            "linux" | "ubuntu" => Ok(Self::Linux),
            "windows" | "win" => Ok(Self::Windows),
            "macos" | "darwin" | "osx" | "mac" => Ok(Self::Macos),
            other => Err(ImageError::Reference(
                other.into(),
                "unknown OS; expected linux, windows or macos".into(),
            )),
        }
    }

    /// Lowercase name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Windows => "windows",
            Self::Macos => "macos",
        }
    }

    /// The override variable, then its deprecated aliases.
    pub fn env_vars(self) -> &'static [&'static str] {
        match self {
            Self::Linux => &[
                "CUA_IMAGE_LINUX",
                "CUA_DEFAULT_LINUX_IMAGE",
                "CUA_SANDBOX_LINUX_CONTAINER_IMAGE",
            ],
            Self::Windows => &["CUA_IMAGE_WINDOWS", "CUA_DEFAULT_WINDOWS_IMAGE"],
            Self::Macos => &["CUA_IMAGE_MACOS", "CUA_DEFAULT_MACOS_IMAGE"],
        }
    }

    fn repo(self) -> &'static str {
        match self {
            Self::Linux => LINUX_REPO,
            Self::Windows => WINDOWS_REPO,
            Self::Macos => MACOS_REPO,
        }
    }

    fn default_version(self) -> &'static str {
        match self {
            Self::Linux => LINUX_DEFAULT_VERSION,
            Self::Windows => WINDOWS_DEFAULT_VERSION,
            Self::Macos => MACOS_DEFAULT_VERSION,
        }
    }

    fn version_tag(self, version: &str) -> String {
        let v = version.trim().to_ascii_lowercase();
        match (self, v.as_str()) {
            (Self::Macos, "tahoe") => "26".into(),
            (Self::Macos, "sequoia") => "15".into(),
            (Self::Linux, "noble") => "24.04".into(),
            _ => version.trim().to_string(),
        }
    }
}

/// An image tier: what a canonical image carries beyond the OS.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub enum Tier {
    /// The minimum that passes `cua-spacesd doctor --strict` with every
    /// feature (`-slim`). CI and benchmarks use it.
    Slim,
    /// The default: slim plus dev tooling (no tag suffix).
    #[default]
    Full,
    /// macOS only: full plus one pinned Xcode (`-xcode`, or `-xcode-X.Y`
    /// for that Xcode version).
    Xcode(Option<String>),
}

impl Tier {
    /// Parses `slim`, `full` (or empty), `xcode`, `xcode-26.1`.
    pub fn parse(tier: &str) -> Result<Self> {
        let t = tier.trim().to_ascii_lowercase();
        match t.as_str() {
            "" | "full" | "default" => Ok(Self::Full),
            "slim" => Ok(Self::Slim),
            "xcode" => Ok(Self::Xcode(None)),
            _ => match t.strip_prefix("xcode-") {
                Some(v) if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit() || b == b'.') => {
                    Ok(Self::Xcode(Some(v.into())))
                }
                _ => Err(ImageError::Reference(
                    tier.into(),
                    "unknown tier; expected slim, full, xcode or xcode-<X.Y>".into(),
                )),
            },
        }
    }

    /// The tag suffix (`""`, `-slim`, `-xcode`, `-xcode-26.1`).
    pub fn suffix(&self) -> String {
        match self {
            Self::Slim => "-slim".into(),
            Self::Full => String::new(),
            Self::Xcode(None) => "-xcode".into(),
            Self::Xcode(Some(v)) => format!("-xcode-{v}"),
        }
    }

    /// The catalog's `tier` value (`slim`, `full`, `xcode`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Slim => "slim",
            Self::Full => "full",
            Self::Xcode(_) => "xcode",
        }
    }
}

/// The override for `os` from the environment, if set (`CUA_IMAGE_<OS>`,
/// then the deprecated names, with a warning).
pub fn env_override(os: CanonicalOs) -> Option<String> {
    env_override_with(os, &|n| std::env::var(n).ok())
}

/// [`env_override`] with an explicit environment lookup.
pub fn env_override_with(os: CanonicalOs, env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    for (i, name) in os.env_vars().iter().enumerate() {
        if let Some(v) = env(name) {
            let v = v.trim();
            if v.is_empty() {
                continue;
            }
            if i > 0 {
                tracing::warn!("{name} is deprecated; set {} instead", os.env_vars()[0]);
            }
            return Some(v.to_string());
        }
    }
    None
}

/// The canonical image for `os` (`linux`, `windows`, `macos`) and optional
/// `version` (`None` = the default version, which the `CUA_IMAGE_<OS>`
/// override replaces). macOS accepts `tahoe`/`sequoia` for 26/15.
pub fn canonical_image(os: &str, version: Option<&str>) -> Result<String> {
    let os = CanonicalOs::parse(os)?;
    Ok(canonical_for(os, version))
}

/// [`canonical_image`] for a parsed OS.
pub fn canonical_for(os: CanonicalOs, version: Option<&str>) -> String {
    canonical_for_with(os, version, &|n| std::env::var(n).ok())
}

/// [`canonical_for`] with an explicit environment lookup.
pub fn canonical_for_with(
    os: CanonicalOs,
    version: Option<&str>,
    env: &dyn Fn(&str) -> Option<String>,
) -> String {
    let version = version.map(str::trim).filter(|v| !v.is_empty());
    let is_default = version.is_none_or(|v| os.version_tag(v) == os.default_version());
    if is_default && let Some(v) = env_override_with(os, env) {
        return v;
    }
    let tag = version
        .map(|v| os.version_tag(v))
        .unwrap_or_else(|| os.default_version().to_string());
    format!("{}:{tag}", os.repo())
}

/// The canonical image for `os`, `version` and `tier` (`None` = full, the
/// default). `tier_image("linux", None, Some("slim"))` is
/// `ghcr.io/trycua/linux:24.04-slim`; `-disk` siblings resolve from there.
/// The `CUA_IMAGE_<OS>` override replaces the default version's full tier
/// only. Raises [`ImageError::NotPublished`] for a catalog entry that is not
/// published yet, and a reference error for `xcode` outside macOS.
pub fn tier_image(os: &str, version: Option<&str>, tier: Option<&str>) -> Result<String> {
    let os = CanonicalOs::parse(os)?;
    let tier = tier.map(Tier::parse).transpose()?.unwrap_or_default();
    tier_for_with(os, version, &tier, &|n| std::env::var(n).ok())
}

/// [`tier_image`] for a parsed OS and tier, with an explicit environment.
pub fn tier_for_with(
    os: CanonicalOs,
    version: Option<&str>,
    tier: &Tier,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<String> {
    if matches!(tier, Tier::Xcode(_)) && os != CanonicalOs::Macos {
        return Err(ImageError::Reference(
            os.as_str().into(),
            "the xcode tier exists only for macOS".into(),
        ));
    }
    if *tier == Tier::Full {
        let version = version.map(str::trim).filter(|v| !v.is_empty());
        let is_default = version.is_none_or(|v| os.version_tag(v) == os.default_version());
        if is_default && let Some(v) = env_override_with(os, env) {
            return Ok(v);
        }
    }
    let base = canonical_for_with(os, version, &|_| None);
    crate::catalog::require_published(format!("{base}{}", tier.suffix()))
}

/// The Omarchy image (`ghcr.io/trycua/omarchy:<channel>`, default `edge`):
/// Omarchy (Arch Linux, Hyprland) as an amd64 VM with cua-spacesd.
/// Raises [`ImageError::NotPublished`] until CI publishes the channel.
pub fn omarchy_image(channel: Option<&str>) -> Result<String> {
    let word = format!("omarchy:{}", channel.unwrap_or_default());
    Ok(resolve_word_with(&word, None, &|_| None)?
        .expect("omarchy is a word")
        .0)
}

/// A CLI image word with an optional `--tier`: the canonical aliases
/// ([`alias_with`]; the tier picks `-slim`/`-xcode`), `omarchy[:<channel>]`,
/// or `Ok(None)` for a plain reference (a tier then is an error). Returns
/// the reference and the guest OS. Unpublished catalog entries fail with
/// [`ImageError::NotPublished`].
pub fn resolve_word_with(
    name: &str,
    tier: Option<&str>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<(String, CanonicalOs)>> {
    match word_reference_with(name, tier, env)? {
        Some((r, os, true)) => Ok(Some((crate::catalog::require_published(r)?, os))),
        Some((r, os, false)) => Ok(Some((r, os))),
        None => Ok(None),
    }
}

/// [`resolve_word_with`] without the published check: the reference a
/// word names (for catalog lookups), its OS, and whether the catalog gate
/// applies (not for a `CUA_IMAGE_<OS>` override).
pub fn word_reference_with(
    name: &str,
    tier: Option<&str>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<(String, CanonicalOs, bool)>> {
    let word = name.trim();
    let (head, rest) = match word.split_once(':') {
        Some((h, r)) => (h, Some(r)),
        None => (word, None),
    };
    let tier = tier.map(Tier::parse).transpose()?.unwrap_or_default();
    if head.eq_ignore_ascii_case("omarchy") {
        if tier != Tier::Full {
            return Err(ImageError::Reference(
                word.into(),
                "Omarchy has no tiers".into(),
            ));
        }
        let channel = rest
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .unwrap_or(OMARCHY_DEFAULT_CHANNEL);
        if !["edge", "rc", "stable"].contains(&channel) {
            return Err(ImageError::Reference(
                channel.into(),
                "unknown Omarchy channel; expected edge, rc or stable".into(),
            ));
        }
        return Ok(Some((
            format!("{OMARCHY_REPO}:{channel}"),
            CanonicalOs::Linux,
            true,
        )));
    }
    let Some((reference, os)) = alias_with(word, env) else {
        return if tier == Tier::Full {
            Ok(None)
        } else {
            Err(ImageError::Reference(
                word.into(),
                "--tier applies to the aliases linux, windows and macos only; put the tier in the tag instead".into(),
            ))
        };
    };
    if matches!(tier, Tier::Xcode(_)) && os != CanonicalOs::Macos {
        return Err(ImageError::Reference(
            os.as_str().into(),
            "the xcode tier exists only for macOS".into(),
        ));
    }
    if tier == Tier::Full {
        let overridden = !reference.starts_with(os.repo());
        return Ok(Some((reference, os, !overridden)));
    }
    let base = canonical_for_with(os, rest, &|_| None);
    Ok(Some((format!("{base}{}", tier.suffix()), os, true)))
}

/// The canonical image an alias names, or `None` when `name` is not an
/// alias (then it is an image reference). Aliases: `linux`, `windows`,
/// `macos`, each optionally with `:<version>` (`macos:tahoe`, `linux:22.04`,
/// `windows:2022`), and the bare word `ubuntu`. `ubuntu:<tag>` is a Docker
/// Hub reference (`docker.io/library/ubuntu:<tag>`), never an alias.
///
/// Only the CLI's bare image words and `Image.linux()/windows()/macos()`
/// apply aliases; `Image.from_registry(ref)` is always literal.
pub fn alias(name: &str) -> Option<String> {
    alias_with(name, &|n| std::env::var(n).ok()).map(|(r, _)| r)
}

/// [`alias`] with an explicit environment lookup; also returns the OS.
pub fn alias_with(
    name: &str,
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<(String, CanonicalOs)> {
    let name = name.trim();
    // `<os>-<tier>` (`linux-slim`, `macos-slim`): the default version's
    // tier, the way `cua cloud status` and the apps name image families.
    if !name.contains(':')
        && let Some((os, tier)) = name.split_once('-')
        && let Ok(os) = CanonicalOs::parse(os)
        && let Ok(tier) = Tier::parse(tier)
        && tier != Tier::Full
    {
        return tier_for_with(os, None, &tier, env).ok().map(|r| (r, os));
    }
    let (os, version) = match name.split_once(':') {
        Some((o, v)) => (o, Some(v)),
        None => (name, None),
    };
    let os = match os.to_ascii_lowercase().as_str() {
        "linux" => CanonicalOs::Linux,
        // Bare `ubuntu` only: `ubuntu:24.04` is the Docker Hub image.
        "ubuntu" if version.is_none() => CanonicalOs::Linux,
        "windows" => CanonicalOs::Windows,
        "macos" => CanonicalOs::Macos,
        _ => return None,
    };
    // `ubuntu:22.04`-style versions only; never a path or registry.
    if version.is_some_and(|v| v.is_empty() || v.contains('/') || v.contains('@')) {
        return None;
    }
    Some((canonical_for_with(os, version, env), os))
}

/// Whether `image` is a canonical image: an alias, a reference into one of
/// the canonical repositories (any tag, variant or digest), or the current
/// `CUA_IMAGE_<OS>` override (or a variant of it).
pub fn is_canonical(image: &str) -> bool {
    let image = image.trim();
    if alias(image).is_some() {
        return true;
    }
    let repo_of = |r: &str| {
        crate::resolve::NormalizedRef::parse(r)
            .ok()
            .map(|n| n.repo())
    };
    let Some(repo) = repo_of(image) else {
        return false;
    };
    [LINUX_REPO, WINDOWS_REPO, MACOS_REPO]
        .iter()
        .any(|r| repo == *r)
        || [CanonicalOs::Linux, CanonicalOs::Windows, CanonicalOs::Macos]
            .into_iter()
            .filter_map(env_override)
            .any(|o| repo_of(&o).as_deref() == Some(repo.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_tier_names_are_aliases() {
        let none = |_: &str| None;
        assert_eq!(
            alias_with("linux-slim", &none).map(|(r, _)| r).as_deref(),
            Some("ghcr.io/trycua/linux:24.04-slim")
        );
        assert_eq!(alias_with("linux-full", &none), None);
        assert_eq!(alias_with("linux-nope", &none), None);
        assert_eq!(alias_with("foo-slim", &none), None);
    }

    // Env-var tests mutate process state; keep them in one test.
    #[test]
    fn defaults_aliases_and_overrides() {
        for os in [CanonicalOs::Linux, CanonicalOs::Windows, CanonicalOs::Macos] {
            for v in os.env_vars() {
                unsafe { std::env::remove_var(v) };
            }
        }
        assert_eq!(
            canonical_image("linux", None).unwrap(),
            "ghcr.io/trycua/linux:24.04"
        );
        assert_eq!(
            canonical_image("windows", None).unwrap(),
            "ghcr.io/trycua/windows:2022"
        );
        assert_eq!(
            canonical_image("macos", None).unwrap(),
            "ghcr.io/trycua/macos:26"
        );
        assert_eq!(
            canonical_image("macos", Some("sequoia")).unwrap(),
            "ghcr.io/trycua/macos:15"
        );
        assert_eq!(
            alias("macos:tahoe").as_deref(),
            Some("ghcr.io/trycua/macos:26")
        );
        assert_eq!(
            alias("ubuntu").as_deref(),
            Some("ghcr.io/trycua/linux:24.04")
        );
        assert_eq!(
            alias("windows").as_deref(),
            Some("ghcr.io/trycua/windows:2022")
        );
        assert_eq!(alias("python:3.12-slim"), None);
        // `ubuntu:<tag>` is Docker Hub's ubuntu, not the canonical Linux.
        assert_eq!(alias("ubuntu:24.04"), None);
        assert_eq!(alias("ubuntu:22.04"), None);
        assert_eq!(
            alias("linux:24.04").as_deref(),
            Some("ghcr.io/trycua/linux:24.04")
        );
        assert!(!is_canonical("ubuntu:24.04"));
        assert_eq!(alias("linux/amd64"), None);
        assert!(canonical_image("beos", None).is_err());
        assert!(is_canonical("ghcr.io/trycua/linux:24.04-disk"));
        assert!(is_canonical(&format!(
            "ghcr.io/trycua/windows@sha256:{}",
            "a".repeat(64)
        )));
        assert!(is_canonical("macos:tahoe"));
        assert!(!is_canonical("python:3.12-slim"));
        assert!(!is_canonical("ghcr.io/trycua/cua-desktop-linux:latest"));

        unsafe { std::env::set_var("CUA_DEFAULT_LINUX_IMAGE", "localhost:5000/old:1") };
        assert_eq!(alias("linux").as_deref(), Some("localhost:5000/old:1"));
        unsafe { std::env::set_var("CUA_IMAGE_LINUX", "localhost:5000/new:1") };
        assert_eq!(alias("linux").as_deref(), Some("localhost:5000/new:1"));
        // An explicit non-default version ignores the override.
        assert_eq!(
            alias("linux:22.04").as_deref(),
            Some("ghcr.io/trycua/linux:22.04")
        );
        assert_eq!(
            alias("linux:24.04").as_deref(),
            Some("localhost:5000/new:1")
        );
        unsafe {
            std::env::remove_var("CUA_IMAGE_LINUX");
            std::env::remove_var("CUA_DEFAULT_LINUX_IMAGE");
        }
    }

    #[test]
    fn tiers() {
        let none = |_: &str| None;
        let over = |n: &str| (n == "CUA_IMAGE_LINUX").then(|| "localhost:5000/l:1".to_string());
        let t = |s| Tier::parse(s).unwrap();
        assert_eq!(t(""), Tier::Full);
        assert_eq!(t("SLIM"), Tier::Slim);
        assert_eq!(t("xcode-26.1").suffix(), "-xcode-26.1");
        assert!(Tier::parse("xcode-beta").is_err());
        assert!(Tier::parse("tiny").is_err());
        let full = tier_for_with(CanonicalOs::Linux, None, &Tier::Full, &none).unwrap();
        assert_eq!(full, "ghcr.io/trycua/linux:24.04");
        // The override replaces the default full image only.
        assert_eq!(
            tier_for_with(CanonicalOs::Linux, None, &Tier::Full, &over).unwrap(),
            "localhost:5000/l:1"
        );
        let slim = tier_for_with(CanonicalOs::Linux, None, &Tier::Slim, &over);
        let xcode = tier_for_with(CanonicalOs::Macos, Some("tahoe"), &t("xcode"), &none);
        for (got, want) in [
            (slim, "ghcr.io/trycua/linux:24.04-slim"),
            (xcode, "ghcr.io/trycua/macos:26-xcode"), // <!-- cua-image-unverified --> not published yet
        ] {
            // Unpublished until CI publishes the tier; then the ref itself.
            match crate::catalog::find(want) {
                Some(e) if !e.published => assert!(
                    matches!(&got, Err(ImageError::NotPublished { reference }) if reference == want),
                    "{got:?}"
                ),
                _ => assert_eq!(got.unwrap(), want),
            }
        }
        // Not in the catalog: passes through (the registry decides).
        assert_eq!(
            tier_for_with(CanonicalOs::Macos, Some("15"), &Tier::Slim, &none).unwrap(),
            format!("{MACOS_REPO}:15-slim")
        );
        assert!(tier_for_with(CanonicalOs::Linux, None, &t("xcode"), &none).is_err());
    }

    #[test]
    fn words_and_omarchy() {
        let none = |_: &str| None;
        let w = |n: &str, t: Option<&str>| resolve_word_with(n, t, &none);
        assert_eq!(
            w("linux", None).unwrap(),
            Some(("ghcr.io/trycua/linux:24.04".into(), CanonicalOs::Linux))
        );
        assert_eq!(
            w("macos:tahoe", Some("full")).unwrap().unwrap().0,
            "ghcr.io/trycua/macos:26"
        );
        assert_eq!(w("python:3.12", None).unwrap(), None);
        assert!(w("python:3.12", Some("slim")).is_err());
        assert!(w("linux", Some("xcode")).is_err());
        assert!(w("omarchy", Some("slim")).is_err());
        assert!(w("omarchy:nightly", None).is_err());
        assert_eq!(
            word_reference_with("omarchy", None, &none)
                .unwrap()
                .unwrap()
                .0,
            "ghcr.io/trycua/omarchy:edge"
        );
        assert_eq!(
            word_reference_with("linux", Some("slim"), &none)
                .unwrap()
                .unwrap()
                .0,
            "ghcr.io/trycua/linux:24.04-slim"
        );
        // An override is never gated.
        let over = |n: &str| (n == "CUA_IMAGE_LINUX").then(|| "localhost:5000/l:1".to_string());
        assert_eq!(
            resolve_word_with("linux", None, &over).unwrap().unwrap().0,
            "localhost:5000/l:1"
        );
        let o = w("omarchy", None);
        match crate::catalog::find("ghcr.io/trycua/omarchy:edge") {
            Some(e) if !e.published => {
                let err = o.unwrap_err();
                assert!(matches!(err, ImageError::NotPublished { .. }));
                let m = err.to_string();
                assert!(m.contains("not published yet"), "{m}");
                assert!(
                    m.contains("Image.from_registry(\"ghcr.io/trycua/omarchy:edge\")"),
                    "{m}"
                );
            }
            _ => assert_eq!(o.unwrap().unwrap().0, "ghcr.io/trycua/omarchy:edge"),
        }
        assert_eq!(
            omarchy_image(None).map_err(|e| matches!(e, ImageError::NotPublished { .. })),
            crate::catalog::find("ghcr.io/trycua/omarchy:edge")
                .filter(|e| !e.published)
                .map_or(Ok("ghcr.io/trycua/omarchy:edge".to_string()), |_| Err(true))
        );
    }
}
