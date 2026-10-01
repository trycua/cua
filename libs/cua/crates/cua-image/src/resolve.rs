//! The one image resolver: `resolve(ref, backend, arch) -> ResolvedImage`.
//!
//! Every SDK (Python through the native `cua` binding, TypeScript, the CLI,
//! Spaces, the daemon's local placement and cua-fleet) turns an image
//! reference into something a backend runs through this function.
//!
//! 1. The reference is normalised like the docker CLI: a short ref
//!    (`python:3.12-slim`) is `docker.io/library/python:3.12-slim`.
//! 2. Its registry documents are read with the [`crate::auth`] chain
//!    (`CUA_REGISTRY_*`, docker config and credential helpers,
//!    `GITHUB_TOKEN` for ghcr.io, ECR), and classified by
//!    [`crate::detect::classify`] into a [`Variant`].
//! 3. When the backend cannot run that variant, the sibling that it can is
//!    found through the `ai.cua.image.variants` annotation on the primary
//!    index or manifest (a JSON map `variant -> ref`), else the tag
//!    convention: `<tag>-disk` for the KubeVirt containerDisk, `<tag>-lume`
//!    for Lume, and the tag without the suffix for the rootfs. A missing primary tag also falls through to
//!    its siblings (`windows:2022` has only `windows:2022-disk`).
//! 4. The result is pinned: `pinned_ref` is `registry/repo@sha256:…` of the
//!    variant's top-level document (index or manifest).
//!
//! | backend | runs |
//! |---|---|
//! | [`Backend::Container`] (docker, gVisor) | rootfs |
//! | [`Backend::Vm`] (QEMU, KubeVirt, `vm=True`) | containerdisk |
//! | [`Backend::Lume`] | lume |
//! | [`Backend::Local`] | rootfs, containerdisk, lume |
//! | [`Backend::Fleet`] | rootfs, containerdisk (always amd64) |
//! | [`Backend::Auto`] | whatever the reference is |

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::detect::{FullInspection, ImageKind, inspect_full};
use crate::error::{ImageError, Result};
use crate::manifest::oci_arch;
use crate::media_types::LUME_ANNOTATION_OS;
use crate::registry::{ManifestSource, RegistryClient, parse_ref};

/// Annotation on a primary index/manifest linking its variants:
/// a JSON object `{"containerdisk": "<ref>", "lume": "<ref>", ...}`.
pub const VARIANTS_ANNOTATION: &str = "ai.cua.image.variants";
/// Optional annotation (index/manifest) or config label naming the guest OS
/// (`linux`, `windows`, `macos`) when the platform does not say
/// (containerDisks are all `linux` platforms).
pub const OS_ANNOTATION: &str = "ai.cua.image.os";

/// What a resolved image is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Variant {
    /// A container rootfs (docker, gVisor).
    Rootfs,
    /// A KubeVirt containerDisk (`/disk/disk.img`): QEMU locally, KubeVirt on Fleet.
    Containerdisk,
    /// A Lume VM image (Apple Virtualization, macOS hosts).
    Lume,
}

/// Why a macOS image (Lume or not) does not run on Fleet.
pub const FLEET_MACOS_UNSUPPORTED: &str =
    "macOS images run locally with Lume; Fleet does not offer macOS sandboxes in this SDK";

/// The name [`ImageError::UnsupportedVariant`] lists in `found` for a
/// darwin image in no Lume format ([`ImageKind::Macos`]): no backend runs it.
pub const MACOS_FOUND: &str = "macos";

impl Variant {
    /// All variants.
    pub const ALL: [Variant; 3] = [Variant::Rootfs, Variant::Containerdisk, Variant::Lume];

    /// `rootfs`, `containerdisk`, `lume`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rootfs => "rootfs",
            Self::Containerdisk => "containerdisk",
            Self::Lume => "lume",
        }
    }

    /// Parses [`Variant::as_str`] (also `container-disk`, `macos`).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "rootfs" | "container" => Some(Self::Rootfs),
            "containerdisk" | "container-disk" | "disk" => Some(Self::Containerdisk),
            "lume" => Some(Self::Lume),
            _ => None,
        }
    }

    /// The tag suffix of this variant's sibling (`""` for rootfs).
    pub fn tag_suffix(self) -> &'static str {
        match self {
            Self::Rootfs => "",
            Self::Containerdisk => "-disk",
            Self::Lume => "-lume",
        }
    }

    /// From a classified [`ImageKind`] (`Other` has no variant).
    pub fn from_kind(kind: ImageKind) -> Option<Self> {
        match kind {
            ImageKind::Rootfs => Some(Self::Rootfs),
            ImageKind::ContainerDisk => Some(Self::Containerdisk),
            ImageKind::Lume => Some(Self::Lume),
            ImageKind::Macos | ImageKind::Other => None,
        }
    }
}

/// What will run the image.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    /// No preference: the reference's own variant.
    Auto,
    /// This machine: rootfs (container), containerdisk (QEMU) or lume.
    Local,
    /// Fleet: rootfs (gVisor) or containerdisk (KubeVirt).
    Fleet,
    /// docker / gVisor locally, `gvisor` on Fleet.
    Container,
    /// QEMU locally, `kubevirt` on Fleet (`vm=True`).
    Vm,
    /// Lume.
    Lume,
}

impl Backend {
    /// Parses a backend or runtime name: `auto`, `local`, `fleet`,
    /// `container`/`docker`/`gvisor`/`runsc`, `vm`/`qemu`/`kubevirt`, `lume`.
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "" | "auto" => Self::Auto,
            "local" => Self::Local,
            "fleet" | "cloud" => Self::Fleet,
            "container" | "docker" | "gvisor" | "runsc" | "rootfs" => Self::Container,
            "vm" | "qemu" | "kubevirt" | "containerdisk" => Self::Vm,
            "lume" => Self::Lume,
            other => {
                return Err(ImageError::Reference(
                    other.into(),
                    "unknown backend; expected auto, local, fleet, container, vm or lume".into(),
                ));
            }
        })
    }

    /// Lowercase name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Local => "local",
            Self::Fleet => "fleet",
            Self::Container => "container",
            Self::Vm => "vm",
            Self::Lume => "lume",
        }
    }

    /// Variants this backend runs, in order of preference for siblings.
    pub fn accepts(self) -> &'static [Variant] {
        use Variant::*;
        match self {
            Self::Auto => &[Rootfs, Containerdisk, Lume],
            Self::Local => &[Rootfs, Containerdisk, Lume],
            Self::Fleet => &[Rootfs, Containerdisk],
            Self::Container => &[Rootfs],
            Self::Vm => &[Containerdisk],
            Self::Lume => &[Lume],
        }
    }
}

/// A reference resolved for a backend.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedImage {
    /// The reference as given, normalised (`docker.io/library/python:3.12-slim`).
    pub reference: String,
    /// The chosen variant's reference by tag (the input, or its sibling).
    /// Backends that cannot pull by digest (Lume) use this.
    pub variant_ref: String,
    /// `registry/repo@sha256:…` of the chosen variant.
    pub pinned_ref: String,
    /// The digest in `pinned_ref`.
    pub digest: String,
    /// What the chosen image is.
    pub variant: Variant,
    /// Architecture to run (OCI name): the requested one when offered,
    /// else the first one offered (then `emulated` is set).
    pub arch: Option<String>,
    /// Architectures the variant offers.
    pub architectures: Vec<String>,
    /// Whether `arch` differs from the requested architecture.
    pub emulated: bool,
    /// Guest OS: `linux`, `windows` or `macos`.
    pub os: String,
    /// Whether the image carries cua-spacesd: the `ai.cua.spacesd` (or an
    /// older [`LEGACY_SPACESD_LABELS`] one) annotation/label when present
    /// (`true`/`false`), else `Some(true)` when its config exposes port 3211,
    /// `Some(false)` for a container rootfs without it, `None` (unknown) for
    /// VM images without the label.
    #[serde(default)]
    pub spacesd: Option<bool>,
    /// The chosen platform's layers, `(digest, size)`, when its manifest
    /// was read: a pull's progress knows every layer's size (and so its
    /// total, and the layers the engine already has) from the start.
    #[serde(default)]
    pub layers: Vec<(String, u64)>,
}

/// Label (image config) or annotation (index/manifest) saying whether an
/// image runs cua-spacesd: `true` (or its port, `3211`) or `false`.
pub const SPACESD_LABEL: &str = "ai.cua.spacesd";
/// Labels images built before the cua-spacesd rename carry instead of
/// [`SPACESD_LABEL`], newest first: `ai.cua.guestd`, then
/// `ai.cua.env-driver` (still written next to [`SPACESD_LABEL`] for older
/// SDKs). Read as fallbacks; this SDK never writes them.
pub const LEGACY_SPACESD_LABELS: [&str; 2] = ["ai.cua.guestd", "ai.cua.env-driver"];

/// The first of [`SPACESD_LABEL`] and [`LEGACY_SPACESD_LABELS`] that `get`
/// finds.
fn spacesd_marker<'a, V: 'a>(get: impl Fn(&str) -> Option<&'a V>) -> Option<&'a V> {
    std::iter::once(SPACESD_LABEL)
        .chain(LEGACY_SPACESD_LABELS)
        .find_map(get)
}
/// cua-spacesd's default port.
pub const SPACESD_PORT: u16 = 3211;

/// A normalised reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NormalizedRef {
    /// `docker.io`, `ghcr.io`, `localhost:5000`, ...
    pub registry: String,
    /// `library/python`.
    pub repository: String,
    pub tag: Option<String>,
    pub digest: Option<String>,
}

impl NormalizedRef {
    /// Parses like the docker CLI (short refs are `docker.io/library/...`).
    pub fn parse(reference: &str) -> Result<Self> {
        let r = parse_ref(reference.trim())?;
        Ok(Self {
            registry: r.registry().to_string(),
            repository: r.repository().to_string(),
            tag: r.tag().map(str::to_string),
            digest: r.digest().map(str::to_string),
        })
    }

    /// `registry/repository`.
    pub fn repo(&self) -> String {
        format!("{}/{}", self.registry, self.repository)
    }

    /// The full reference (tag and digest when present).
    pub fn full(&self) -> String {
        let mut s = self.repo();
        if let Some(t) = &self.tag {
            s.push(':');
            s.push_str(t);
        }
        if let Some(d) = &self.digest {
            s.push('@');
            s.push_str(d);
        }
        s
    }

    /// `repo@digest`.
    pub fn pinned(&self, digest: &str) -> String {
        format!("{}@{digest}", self.repo())
    }

    /// The sibling reference for `variant` by the tag convention, or `None`
    /// for a digest-only reference.
    pub fn sibling(&self, variant: Variant) -> Option<String> {
        let tag = self.tag.as_deref()?;
        let base = Variant::ALL
            .iter()
            .filter(|v| !v.tag_suffix().is_empty())
            .find_map(|v| tag.strip_suffix(v.tag_suffix()))
            .unwrap_or(tag);
        Some(format!("{}:{base}{}", self.repo(), variant.tag_suffix()))
    }
}

/// Normalise a reference (`python` → `docker.io/library/python:latest`).
pub fn normalize(reference: &str) -> Result<String> {
    let n = NormalizedRef::parse(reference)?;
    Ok(n.full())
}

struct Probe {
    reference: String,
    full: FullInspection,
    variant: Option<Variant>,
}

impl Probe {
    fn variants_annotation(&self) -> BTreeMap<String, String> {
        let raw = self
            .full
            .index
            .as_ref()
            .and_then(|i| i.annotations.get(VARIANTS_ANNOTATION))
            .or_else(|| {
                self.full
                    .manifest
                    .as_ref()
                    .and_then(|m| m.annotations.get(VARIANTS_ANNOTATION))
            });
        raw.and_then(|s| serde_json::from_str::<BTreeMap<String, String>>(s).ok())
            .unwrap_or_default()
    }

    fn os(&self, variant: Variant, reference: &NormalizedRef) -> String {
        if variant == Variant::Lume {
            return "macos".into();
        }
        let annotated = self
            .full
            .index
            .as_ref()
            .and_then(|i| i.annotations.get(OS_ANNOTATION))
            .or_else(|| {
                self.full.manifest.as_ref().and_then(|m| {
                    m.annotations
                        .get(OS_ANNOTATION)
                        .or_else(|| m.annotations.get(LUME_ANNOTATION_OS))
                })
            })
            .cloned()
            .or_else(|| {
                self.full
                    .config
                    .as_ref()
                    .and_then(|c| c.pointer("/config/Labels"))
                    .and_then(|l| l.get(OS_ANNOTATION))
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            });
        if let Some(os) = annotated {
            return normalize_os(&os);
        }
        // Windows ships only as a containerDisk (a `linux` platform), so the
        // repository name is the only other signal.
        let name = reference.repository.rsplit('/').next().unwrap_or("");
        if variant == Variant::Containerdisk && name.to_ascii_lowercase().contains("windows") {
            return "windows".into();
        }
        let config_os = self
            .full
            .config
            .as_ref()
            .and_then(|c| c.get("os"))
            .and_then(|o| o.as_str());
        match config_os {
            Some(os) => normalize_os(os),
            None => "linux".into(),
        }
    }
}

impl Probe {
    /// `ai.cua.spacesd` (else an older [`LEGACY_SPACESD_LABELS`] one) from the
    /// index, the manifest or the config labels (first found wins; `true`/a port = present, `false`/`0` =
    /// absent), then `EXPOSE 3211`. Without either: `Some(false)` for a
    /// container rootfs (its config says everything it runs), else unknown.
    fn spacesd(&self, variant: Variant) -> Option<bool> {
        let cfg = self.full.config.as_ref().and_then(|c| c.get("config"));
        let marked = self
            .full
            .index
            .as_ref()
            .and_then(|i| spacesd_marker(|k| i.annotations.get(k)))
            .or_else(|| {
                self.full
                    .manifest
                    .as_ref()
                    .and_then(|m| spacesd_marker(|k| m.annotations.get(k)))
            })
            .cloned()
            .or_else(|| {
                let labels = cfg.and_then(|c| c.get("Labels"))?;
                spacesd_marker(|k| labels.get(k))
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            });
        if let Some(v) = marked {
            return Some(spacesd_value(&v));
        }
        let exposed = cfg
            .and_then(|c| c.get("ExposedPorts"))
            .and_then(|p| p.as_object())
            .is_some_and(|p| p.contains_key(&format!("{SPACESD_PORT}/tcp")));
        if exposed {
            Some(true)
        } else if variant == Variant::Rootfs && self.full.config.is_some() {
            Some(false)
        } else {
            None
        }
    }
}

/// An `ai.cua.spacesd` (or older label) value: `false`/`no`/`0`/`none`/empty = absent,
/// anything else (`true`, a port) = present.
fn spacesd_value(v: &str) -> bool {
    !matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "" | "false" | "no" | "0" | "none" | "off"
    )
}

fn normalize_os(os: &str) -> String {
    match os.trim().to_ascii_lowercase().as_str() {
        "darwin" | "macos" | "osx" => "macos".into(),
        "windows" | "win" => "windows".into(),
        _ => "linux".into(),
    }
}

async fn probe(src: &dyn ManifestSource, reference: &str, arch: &str) -> Result<Probe> {
    let full = inspect_full(src, reference, arch).await?;
    let variant = Variant::from_kind(full.inspection.kind);
    Ok(Probe {
        reference: reference.to_string(),
        full,
        variant,
    })
}

fn finish(input: &NormalizedRef, p: Probe, variant: Variant, arch: &str) -> Result<ResolvedImage> {
    let vref = NormalizedRef::parse(&p.reference)?;
    let digest = vref.digest.clone().unwrap_or_else(|| p.full.digest.clone());
    let want = oci_arch(arch).to_string();
    let architectures = p.full.inspection.architectures.clone();
    let chosen = if architectures.contains(&want) {
        Some(want.clone())
    } else {
        architectures
            .first()
            .cloned()
            .or(p.full.inspection.arch.clone())
    };
    let emulated = chosen.as_ref().is_some_and(|a| *a != want);
    Ok(ResolvedImage {
        reference: input.full(),
        variant_ref: vref.full(),
        pinned_ref: vref.pinned(&digest),
        digest,
        variant,
        arch: chosen,
        architectures,
        emulated,
        os: p.os(variant, &vref),
        spacesd: p.spacesd(variant),
        layers: p
            .full
            .manifest
            .as_ref()
            .map(|m| {
                m.layers
                    .iter()
                    .map(|l| (l.digest.clone(), l.size))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn unsupported(
    reference: &NormalizedRef,
    backend: Backend,
    found: &[Variant],
    other: bool,
    macos: bool,
) -> ImageError {
    let mut names: Vec<String> = found.iter().map(|v| v.as_str().to_string()).collect();
    if macos {
        names.push(MACOS_FOUND.into());
    }
    let reason = if (macos || found.contains(&Variant::Lume)) && backend == Backend::Fleet {
        FLEET_MACOS_UNSUPPORTED.to_string()
    } else if macos && found.is_empty() {
        "a macOS image that is not a Lume image cannot run here; macOS images run locally with \
         Lume (e.g. ghcr.io/trycua/macos:26) on a Mac"
            .to_string()
    } else if other && found.is_empty() {
        "not a runnable image (neither a container rootfs, a containerDisk, nor a Lume macOS \
         image)"
            .to_string()
    } else {
        let wants: Vec<&str> = backend.accepts().iter().map(|v| v.as_str()).collect();
        format!(
            "no {} variant for backend {} (found: {}); publish one (e.g. `{}`) or pick another \
             backend",
            wants.join("/"),
            backend.as_str(),
            if names.is_empty() {
                "none".into()
            } else {
                names.join(", ")
            },
            reference
                .sibling(backend.accepts()[0])
                .unwrap_or_else(|| reference.full()),
        )
    };
    ImageError::UnsupportedVariant {
        reference: reference.full(),
        found: names,
        backend: backend.as_str().into(),
        reason,
    }
}

/// Resolve `reference` for `backend` on `arch` (`amd64`/`arm64`, or the
/// Rust/uname spellings) reading documents from `src`. See the module docs.
pub async fn resolve_with(
    src: &dyn ManifestSource,
    reference: &str,
    backend: Backend,
    arch: &str,
) -> Result<ResolvedImage> {
    let input = NormalizedRef::parse(reference)?;
    let accepts = backend.accepts();
    let primary = match probe(src, &input.full(), arch).await {
        Ok(p) => Some(p),
        // Docker Hub answers 401 for repositories that do not exist (and for
        // private ones): an image only the local engine has looks like this.
        Err(ImageError::Unauthorized(msg)) if input.registry == "docker.io" => {
            return Err(ImageError::NotFound(format!(
                "{}: not on Docker Hub (or private there: `docker login`): {msg}",
                input.full()
            )));
        }
        // A missing tag may still have its siblings (`windows:2022-disk`).
        Err(ImageError::NotFound(msg)) if input.tag.is_some() && input.digest.is_none() => {
            tracing::debug!(reference = %input.full(), %msg, "tag not found; trying its variants");
            None
        }
        Err(e) => return Err(e),
    };
    let mut found: Vec<Variant> = Vec::new();
    let mut other = false;
    // A darwin image in no Lume format: no variant, named in the error.
    let mut macos = false;
    let mut links = BTreeMap::new();
    if let Some(p) = primary {
        match p.variant {
            Some(v) if accepts.contains(&v) => return finish(&input, p, v, arch),
            Some(v) => found.push(v),
            None => {
                other = true;
                macos |= p.full.inspection.kind == ImageKind::Macos;
            }
        }
        links = p.variants_annotation();
    }
    let mut first_err: Option<ImageError> = None;
    for &want in accepts {
        let mut candidates: Vec<String> = Vec::new();
        if let Some(r) = links.get(want.as_str()) {
            candidates.push(r.clone());
        }
        if let Some(s) = input.sibling(want)
            && s != input.full()
            && !candidates.contains(&s)
        {
            candidates.push(s);
        }
        for c in candidates {
            match probe(src, &c, arch).await {
                Ok(p) => match p.variant {
                    Some(v) if v == want => return finish(&input, p, v, arch),
                    Some(v) => {
                        if !found.contains(&v) {
                            found.push(v);
                        }
                    }
                    None => {
                        other = true;
                        macos |= p.full.inspection.kind == ImageKind::Macos;
                    }
                },
                Err(ImageError::NotFound(_)) => {}
                Err(e) => {
                    first_err.get_or_insert(e);
                }
            }
        }
    }
    if found.is_empty() && !other {
        // Name what does exist (`windows:2022-disk` for a container backend)
        // so the error is "wrong variant", not "not found".
        for v in Variant::ALL.iter().filter(|v| !accepts.contains(v)) {
            let Some(s) = input.sibling(*v).filter(|s| *s != input.full()) else {
                continue;
            };
            if let Ok(p) = probe(src, &s, arch).await
                && let Some(pv) = p.variant
            {
                found.push(pv);
            }
        }
        if !found.is_empty() {
            return Err(unsupported(&input, backend, &found, false, false));
        }
        // Nothing exists under this name at all.
        if let Some(e) = first_err {
            return Err(e);
        }
        return Err(ImageError::NotFound(format!(
            "{}: no such image (and no {} variant)",
            input.full(),
            accepts
                .iter()
                .map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join("/")
        )));
    }
    Err(unsupported(&input, backend, &found, other, macos))
}

type CacheKey = (String, Backend, String);

fn cache() -> &'static Mutex<HashMap<CacheKey, (Instant, ResolvedImage)>> {
    static C: OnceLock<Mutex<HashMap<CacheKey, (Instant, ResolvedImage)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

const CACHE_TTL: Duration = Duration::from_secs(300);

/// Set to `0`/`off`/`false` to make [`resolve`] fail at once without a
/// registry read (hermetic tests; callers then fall back as for an
/// unreadable registry). A source installed with [`set_source`] still runs.
pub const RESOLVE_ENV: &str = "CUA_IMAGE_RESOLVE";

fn disabled() -> bool {
    source_slot().read().unwrap().is_none()
        && std::env::var(RESOLVE_ENV).is_ok_and(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "0" | "off" | "false" | "no"
            )
        })
}

/// [`resolve_with`] against the real registries (credentials from
/// [`crate::auth`]), with a five-minute cache of successful resolutions.
pub async fn resolve(reference: &str, backend: Backend, arch: &str) -> Result<ResolvedImage> {
    if disabled() {
        return Err(ImageError::Registry(format!(
            "image resolution is off ({RESOLVE_ENV}=0)"
        )));
    }
    let key = (
        reference.trim().to_string(),
        backend,
        oci_arch(arch).to_string(),
    );
    if let Some((at, r)) = cache().lock().unwrap().get(&key).cloned()
        && at.elapsed() < CACHE_TTL
    {
        return Ok(r);
    }
    let src = source_slot().read().unwrap().clone();
    let r = match src {
        Some(src) => resolve_with(src.as_ref(), reference, backend, arch).await?,
        None => resolve_with(default_client(), reference, backend, arch).await?,
    };
    cache()
        .lock()
        .unwrap()
        .insert(key, (Instant::now(), r.clone()));
    Ok(r)
}

/// [`resolve`] with explicit registry credentials (a `RegistrySecret`) at
/// the head of the auth chain. Without credentials this is [`resolve`]. A
/// source installed with [`set_source`] still wins (tests); results are not
/// cached (they depend on the credentials).
pub async fn resolve_with_credentials(
    reference: &str,
    backend: Backend,
    arch: &str,
    creds: Option<&crate::RegistryCredentials>,
) -> Result<ResolvedImage> {
    let Some(creds) = creds else {
        return resolve(reference, backend, arch).await;
    };
    if disabled() {
        return Err(ImageError::Registry(format!(
            "image resolution is off ({RESOLVE_ENV}=0)"
        )));
    }
    let src = source_slot().read().unwrap().clone();
    match src {
        Some(src) => resolve_with(src.as_ref(), reference, backend, arch).await,
        None => {
            let client = RegistryClient::with_credentials(creds.clone());
            resolve_with(&client, reference, backend, arch).await
        }
    }
}

fn source_slot() -> &'static std::sync::RwLock<Option<std::sync::Arc<dyn ManifestSource>>> {
    static S: OnceLock<std::sync::RwLock<Option<std::sync::Arc<dyn ManifestSource>>>> =
        OnceLock::new();
    S.get_or_init(|| std::sync::RwLock::new(None))
}

/// Replaces the registry [`resolve`] reads (`None` restores the real one)
/// and clears its cache. Tests and hosts with their own registry use this.
pub fn set_source(src: Option<std::sync::Arc<dyn ManifestSource>>) {
    *source_slot().write().unwrap() = src;
    cache().lock().unwrap().clear();
}

fn default_client() -> &'static RegistryClient {
    static C: OnceLock<RegistryClient> = OnceLock::new();
    C.get_or_init(RegistryClient::default)
}

/// Canonical OS aliases and bare references in one step: `linux`,
/// `windows`, `macos[:version]` (and bare `ubuntu`) become the canonical
/// image ([`crate::canonical::alias`]), anything else is resolved as given.
/// For CLI image words only: `Image.from_registry(ref)` uses [`resolve`].
pub async fn resolve_alias(name: &str, backend: Backend, arch: &str) -> Result<ResolvedImage> {
    let reference = crate::canonical::alias(name).unwrap_or_else(|| name.to_string());
    resolve(&reference, backend, arch).await
}

#[cfg(test)]
mod tests;
