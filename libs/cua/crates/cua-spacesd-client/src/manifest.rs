//! `/etc/cua-image/manifest.json`: what an image claims.
//!
//! Generated at image build time from the image's `image.json` plus build
//! facts (`libs/images/common/tools/cua-image-manifest`). Every doctor
//! (`cua-spacesd doctor`, the doctor shim and the host-side `cua doctor`)
//! derives each check's severity from it: a claimed feature is required, an
//! optional one recommended, anything else informational.

use std::collections::BTreeMap;
use std::path::Path;

use crate::diagnose::Severity;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Default location inside a Linux or macOS image.
pub const DEFAULT_PATH: &str = "/etc/cua-image/manifest.json";

/// Location inside a Windows image (`%ProgramData%\cua-image`).
pub const WINDOWS_PATH: &str = r"C:\ProgramData\cua-image\manifest.json";

/// Where an image built for this platform keeps its manifest (the doctor
/// running in the guest reads this one).
pub fn local_path() -> &'static str {
    if cfg!(windows) {
        WINDOWS_PATH
    } else {
        DEFAULT_PATH
    }
}

/// Manifest schema version this doctor reads.
pub const SCHEMA_VERSION: u32 = 1;

/// The image's claims.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Manifest {
    /// Always 1.
    pub schema_version: u32,
    /// Image name, for example "linux".
    pub name: String,
    /// Former names of the image ("cua-desktop-linux" for "linux"), kept
    /// for one release so lookups by the old name still match.
    pub aliases: Vec<String>,
    /// "linux", "windows" or "macos".
    pub os: String,
    /// "rootfs" or "containerdisk".
    pub variant: String,
    /// "supervisord", "systemd", "launchd" or "scm".
    pub init: String,
    /// "amd64" or "arm64".
    pub arch: String,
    /// Reference the image was published as, when known at build time.
    #[serde(rename = "ref")]
    pub reference: String,
    /// Source revision of the image definition.
    pub source_revision: String,
    /// Registry annotations/labels the image is published with.
    pub annotations: BTreeMap<String, String>,
    /// The baked cua-spacesd.
    pub spacesd: SpacesdClaim,
    /// Canonical feature names that must be supported.
    pub features_required: Vec<String>,
    /// Features that may be missing (a warning, not a failure). A trailing
    /// `*` matches a prefix ("teleport.*").
    pub features_optional: Vec<String>,
    /// Attribute values a feature must report, by feature name and then
    /// attribute (`{"presence.cursor_shape": {"hit_test": "atspi"}}`). The
    /// doctor fails when the feature is unsupported or reports another
    /// value, so an image pins the backends it was built for.
    pub feature_attributes: BTreeMap<String, BTreeMap<String, String>>,
    /// Encoder backends that must pass the encode/decode check.
    pub codecs_required: Vec<String>,
    /// Token delivery modes the image supports: "local", "pod-secret",
    /// "kubevirt-bridge".
    pub auth_modes: Vec<String>,
    /// Network services the image serves (this variant's).
    pub services: Vec<ServiceClaim>,
    /// Service units of this variant's init system.
    pub units: Vec<String>,
    /// Unit name to a substring of its process command line (used when the
    /// init system cannot be queried, for example supervisord's root-only
    /// socket from an unprivileged caller).
    pub unit_processes: BTreeMap<String, String>,
    /// The desktop session.
    pub display: DisplayClaim,
    /// Conformance fixtures.
    pub fixtures: Fixtures,
    /// Audio plumbing.
    pub audio: AudioClaim,
    /// Compatibility links kept for one release.
    pub compat_links: Vec<CompatLink>,
    /// The image tier ("slim", "full", "xcode"); empty for untiered images.
    pub tier: String,
    /// App name to the command printing its version. Required when claimed:
    /// `doctor --strict` fails when one is missing or its version does not
    /// match `expect`.
    pub apps: BTreeMap<String, SoftwareClaim>,
    /// Tool name to the command printing its version (the dev tiers'
    /// toolchains), enforced like `apps`.
    pub tools: BTreeMap<String, SoftwareClaim>,
    /// The exact set of available simulator runtimes (`xcrun simctl list
    /// runtimes`), e.g. "iOS 26.0". Empty: not checked.
    pub simulator_runtimes: Vec<String>,
    /// App `WindowsService.LaunchApp` must start, when claimed.
    pub launch_app: Option<LaunchApp>,
    /// Fidelity keys expected to differ between variants.
    pub parity: Parity,
    /// Resource floors.
    pub resources: Resources,
    /// Expected time zone.
    pub tz: String,
    /// Expected locale.
    pub locale: String,
    /// Host name the network check resolves (optional).
    pub dns_probe: String,
    /// A/V skew budgets in ms by runtime class ("container", "gvisor", "vm").
    pub av_skew_budget_ms: BTreeMap<String, u32>,
}

/// A claimed app or tool: its version command, as a bare argv
/// (`["python3", "--version"]`) or `{"argv": [...], "expect": "<regex>"}`,
/// where `expect` must match the first line of the output.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SoftwareClaim {
    /// Only the version command.
    Argv(Vec<String>),
    /// The version command and the version it must print.
    Spec {
        /// The version command.
        argv: Vec<String>,
        /// Regex searched in the first output line (empty: any version).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        expect: String,
    },
}

impl SoftwareClaim {
    /// The version command.
    pub fn argv(&self) -> &[String] {
        match self {
            Self::Argv(argv) | Self::Spec { argv, .. } => argv,
        }
    }

    /// The version regex ("" when any version will do).
    pub fn expect(&self) -> &str {
        match self {
            Self::Argv(_) => "",
            Self::Spec { expect, .. } => expect,
        }
    }
}

/// The baked cua-spacesd, from `cua-spacesd build-info` at image build time.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpacesdClaim {
    /// Whether the image ships cua-spacesd.
    pub present: bool,
    /// Build source ("local", "release", "none").
    pub source: String,
    /// Installed path.
    pub path: String,
    /// Version.
    pub version: String,
    /// Protocol revision.
    pub protocol_revision: u32,
    /// Source revision of the binary.
    pub git_sha: String,
    /// Linked cua-driver core version.
    pub cua_driver_version: String,
    /// Hash of the linked cua-driver tool schemas.
    pub tools_sha256: String,
    /// Number of tools.
    pub tools_count: u32,
    /// Encoder backends compiled in.
    pub codecs_compiled: Vec<String>,
}

/// A network service the image serves.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServiceClaim {
    /// Service name ("env", "viewer", "ssh").
    pub name: String,
    /// Port in the guest.
    pub port: u16,
    /// "grpc", "rfb", "http", "tcp" or "udp".
    pub protocol: String,
    /// Component serving it, when it is cua-spacesd.
    pub component: String,
    /// Readiness probe, for example "http:/viewer/" or "tcp".
    pub readiness: String,
}

/// The desktop session.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayClaim {
    /// X display, for example ":1".
    pub name: String,
    /// "<w>x<h>".
    pub resolution: String,
    /// Desktop user.
    pub user: String,
}

impl DisplayClaim {
    /// Parsed resolution.
    pub fn size(&self) -> Option<(u32, u32)> {
        let (w, h) = self.resolution.split_once('x')?;
        Some((w.parse().ok()?, h.parse().ok()?))
    }
}

/// Conformance fixtures shipped in the image.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Fixtures {
    /// Directory holding the fixture scripts.
    pub root: String,
    /// Fixture names ("grid", "form", "http", "tone", "avsync").
    pub apps: Vec<String>,
}

impl Fixtures {
    /// Whether fixture `name` ships.
    pub fn has(&self, name: &str) -> bool {
        !self.root.is_empty() && self.apps.iter().any(|a| a == name)
    }
}

/// Audio plumbing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioClaim {
    /// Server description, for example "pipewire".
    pub server: String,
    /// Default sink.
    pub default_sink: String,
    /// Default source.
    pub default_source: String,
    /// Sink that feeds the uplink source.
    pub uplink_input_sink: String,
}

/// A compatibility alias.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompatLink {
    /// Symlink path (file aliases).
    pub path: String,
    /// What it must resolve to (relative to the link's directory).
    pub target: String,
    /// systemd unit alias (unit aliases).
    pub unit_alias: String,
    /// The unit it aliases.
    pub unit: String,
}

/// The app `LaunchApp` must start.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LaunchApp {
    /// App name or bundle id passed to `LaunchApp`.
    pub app: String,
    /// Arguments.
    pub args: Vec<String>,
    /// Substring of the window title, or of the app name, that must appear.
    pub window_match: String,
    /// Seconds to wait.
    pub timeout_s: u32,
    /// Runtimes where `app` cannot run (a concrete OS or runtime limitation,
    /// published rather than hidden): the doctor launches `instead` there and
    /// names the limitation in the check detail, or skips when `instead` is
    /// unset.
    pub limits: Vec<LaunchAppLimit>,
}

/// A runtime and architecture where [`LaunchApp::app`] cannot run.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LaunchAppLimit {
    /// Runtime name as the doctor reports it ("gvisor", "container", ...).
    pub runtime: String,
    /// Architecture ("x86_64", "arm64"); empty matches every architecture.
    pub arch: String,
    /// Why the app cannot run there.
    pub reason: String,
    /// The app to launch there instead (its own `limits` are ignored).
    pub instead: Option<Box<LaunchApp>>,
}

impl LaunchApp {
    /// The limit that applies on `runtime`/`arch`, if any.
    pub fn limit_for(&self, runtime: &str, arch: &str) -> Option<&LaunchAppLimit> {
        self.limits
            .iter()
            .find(|l| l.runtime == runtime && (l.arch.is_empty() || l.arch == arch))
    }
}

/// Parity settings.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Parity {
    /// Fidelity keys allowed to differ between variants.
    pub expected_diff: Vec<String>,
}

/// Resource floors.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Resources {
    /// Free space on `/`, `/tmp` and the desktop user's home, GiB.
    pub min_disk_gib: f64,
    /// Size of `/dev/shm`, MiB.
    pub min_shm_mib: u64,
    /// Total memory, MiB.
    pub min_memory_mib: u64,
}

impl Default for Resources {
    fn default() -> Self {
        Self {
            min_disk_gib: 2.0,
            min_shm_mib: 256,
            min_memory_mib: 1024,
        }
    }
}

/// A loaded manifest with its provenance.
#[derive(Clone, Debug, Default)]
pub struct Loaded {
    /// The manifest (default when none was found).
    pub manifest: Manifest,
    /// Where it came from ("" when none).
    pub source: String,
    /// SHA-256 of its bytes.
    pub sha256: String,
    /// Why loading failed, when a manifest existed but did not parse.
    pub error: Option<String>,
}

impl Loaded {
    /// Whether a manifest was found and parsed.
    pub fn present(&self) -> bool {
        !self.source.is_empty() && self.error.is_none()
    }

    /// Parses `bytes` from `source`.
    pub fn parse(bytes: &[u8], source: &str) -> Self {
        let sha256 = hex::encode(Sha256::digest(bytes));
        match serde_json::from_slice::<Manifest>(bytes) {
            Ok(manifest) if manifest.schema_version == SCHEMA_VERSION => Loaded {
                manifest,
                source: source.to_owned(),
                sha256,
                error: None,
            },
            Ok(manifest) => Loaded {
                error: Some(format!(
                    "manifest schema_version {} is not {SCHEMA_VERSION}",
                    manifest.schema_version
                )),
                source: source.to_owned(),
                sha256,
                manifest: Manifest::default(),
            },
            Err(error) => Loaded {
                error: Some(format!("manifest does not parse: {error}")),
                source: source.to_owned(),
                sha256,
                manifest: Manifest::default(),
            },
        }
    }

    /// Reads `path`; absent means no manifest (not an error).
    pub fn read(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(bytes) => Self::parse(&bytes, &path.display().to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Loaded::default(),
            Err(error) => Loaded {
                source: path.display().to_string(),
                error: Some(format!("reading {}: {error}", path.display())),
                ..Loaded::default()
            },
        }
    }

    /// Severity of a check that matters because of `claims`:
    ///
    /// - `core`: always required (cua-spacesd's own contract);
    /// - `feature:<name>`: required when the image requires the feature,
    ///   recommended when optional, else info;
    /// - `manifest:<key>`: required when a manifest is present (the image
    ///   declares the thing the check verifies), else info;
    /// - anything else: info.
    ///
    /// The strongest claim wins.
    pub fn severity(&self, claims: &[&str]) -> Severity {
        let mut best = Severity::Info;
        for claim in claims {
            let severity = if *claim == "core" {
                Severity::Required
            } else if let Some(feature) = claim.strip_prefix("feature:") {
                if self.manifest.requires(feature) {
                    Severity::Required
                } else if self.manifest.allows(feature) {
                    Severity::Recommended
                } else {
                    Severity::Info
                }
            } else if claim.starts_with("manifest:") && self.present() {
                Severity::Required
            } else {
                Severity::Info
            };
            best = stronger(best, severity);
        }
        best
    }
}

fn stronger(a: Severity, b: Severity) -> Severity {
    let rank = |s: Severity| match s {
        Severity::Required => 2,
        Severity::Recommended => 1,
        Severity::Info => 0,
    };
    if rank(b) > rank(a) { b } else { a }
}

fn matches(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => pattern == name,
    }
}

impl Manifest {
    /// Whether the image is `name`, by its name or a former one.
    pub fn is_named(&self, name: &str) -> bool {
        self.name == name || self.aliases.iter().any(|a| a == name)
    }

    /// Whether `feature` is required.
    pub fn requires(&self, feature: &str) -> bool {
        self.features_required.iter().any(|p| matches(p, feature))
    }

    /// Whether `feature` is listed as optional.
    pub fn allows(&self, feature: &str) -> bool {
        self.features_optional.iter().any(|p| matches(p, feature))
    }

    /// Whether `feature` is listed at all.
    pub fn lists(&self, feature: &str) -> bool {
        self.requires(feature) || self.allows(feature)
    }

    /// Budget for the A/V skew under `runtime` (a runtime name from the
    /// report environment).
    pub fn av_skew_budget(&self, runtime: &str) -> u32 {
        let class = match runtime {
            "gvisor" => "gvisor",
            "container" => "container",
            _ => "vm",
        };
        self.av_skew_budget_ms
            .get(class)
            .copied()
            .unwrap_or(if class == "gvisor" { 80 } else { 40 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Loaded {
        Loaded::parse(
            br#"{"schema_version":1,"name":"x","features_required":["a11y","driver"],
                 "features_optional":["teleport.*","h264_hw"],"av_skew_budget_ms":{"gvisor":90},
                 "feature_attributes":{"presence.cursor_shape":{"hit_test":"atspi"}}}"#,
            "/etc/cua-image/manifest.json",
        )
    }

    #[test]
    fn severity_follows_claims() {
        let loaded = sample();
        assert!(loaded.present());
        assert_eq!(loaded.severity(&["feature:a11y"]), Severity::Required);
        assert_eq!(
            loaded.severity(&["feature:teleport.firefox"]),
            Severity::Recommended
        );
        assert_eq!(loaded.severity(&["feature:h264_hw"]), Severity::Recommended);
        assert_eq!(loaded.severity(&["feature:hotspot"]), Severity::Info);
        assert_eq!(loaded.severity(&["core"]), Severity::Required);
        assert_eq!(loaded.severity(&["manifest:units"]), Severity::Required);
        // The strongest claim wins.
        assert_eq!(
            loaded.severity(&["feature:hotspot", "feature:driver"]),
            Severity::Required
        );
        // No manifest: only core checks are required.
        let none = Loaded::default();
        assert!(!none.present());
        assert_eq!(none.severity(&["manifest:units"]), Severity::Info);
        assert_eq!(none.severity(&["feature:a11y"]), Severity::Info);
        assert_eq!(none.severity(&["core"]), Severity::Required);
    }

    #[test]
    fn bad_manifests_are_reported_not_trusted() {
        let wrong = Loaded::parse(br#"{"schema_version":2}"#, "m");
        assert!(!wrong.present());
        assert!(wrong.error.unwrap().contains("schema_version 2"));
        let junk = Loaded::parse(b"{", "m");
        assert!(junk.error.unwrap().contains("does not parse"));
        assert_eq!(
            Loaded::read(Path::new("/nonexistent/cua/manifest.json")).source,
            ""
        );
    }

    #[test]
    fn budgets_and_resolution() {
        let loaded = sample();
        assert_eq!(loaded.manifest.av_skew_budget("gvisor"), 90);
        assert_eq!(loaded.manifest.av_skew_budget("container"), 40);
        assert_eq!(loaded.manifest.av_skew_budget("qemu"), 40);
        assert_eq!(
            loaded.manifest.feature_attributes["presence.cursor_shape"]["hit_test"],
            "atspi"
        );
        let display = DisplayClaim {
            resolution: "1280x800".into(),
            ..DisplayClaim::default()
        };
        assert_eq!(display.size(), Some((1280, 800)));
    }

    /// The hand-written claims of the VM-only images parse too.
    #[test]
    fn vm_only_image_claims_parse() {
        // Windows bakes cua-spacesd and pins its cursor-shape backends; the
        // macOS copies predate it.
        for (text, os, spacesd) in [
            (
                include_str!("../../../../../scripts/images/manifests/windows-2022.json"),
                "windows",
                true,
            ),
            (
                include_str!("../../../../../scripts/images/manifests/macos-26.json"),
                "macos",
                false,
            ),
        ] {
            let loaded = Loaded::parse(text.as_bytes(), "fixture");
            assert!(loaded.present(), "{:?}", loaded.error);
            assert_eq!(loaded.manifest.os, os);
            assert_eq!(loaded.manifest.spacesd.present, spacesd);
            if spacesd {
                assert_eq!(
                    loaded.manifest.feature_attributes["presence.cursor_shape"]["hit_test"],
                    "uia"
                );
            }
        }
    }

    /// The image's generated manifest parses with this model.
    #[test]
    fn the_generator_fixture_parses() {
        let text =
            include_str!("../../../../images/common/tools/tests/fixtures/manifest.linux.json");
        let loaded = Loaded::parse(text.as_bytes(), "fixture");
        assert!(loaded.present(), "{:?}", loaded.error);
        assert!(loaded.manifest.requires("desktop_stream"));
        assert!(loaded.manifest.fixtures.has("grid"));
        assert_eq!(
            loaded.manifest.apps["python3"].argv(),
            ["python3", "--version"]
        );
        assert_eq!(loaded.manifest.name, "linux");
        // The pre-rename name still matches, for one release.
        assert!(loaded.manifest.is_named("linux"));
        assert!(loaded.manifest.is_named("cua-desktop-linux"));
        assert!(!loaded.manifest.is_named("omarchy"));
    }

    #[test]
    fn software_claims_take_both_forms() {
        let loaded = Loaded::parse(
            br#"{"schema_version":1,"tier":"full",
                 "apps":{"a":["a","--version"],"b":{"argv":["b","-v"],"expect":"^b 2"}},
                 "tools":{"node":{"argv":["node","--version"]}},
                 "simulator_runtimes":["iOS 26.0"]}"#,
            "m",
        );
        assert!(loaded.present(), "{:?}", loaded.error);
        let m = &loaded.manifest;
        assert_eq!(m.tier, "full");
        assert_eq!(m.apps["a"].expect(), "");
        assert_eq!(m.apps["b"].argv(), ["b", "-v"]);
        assert_eq!(m.apps["b"].expect(), "^b 2");
        assert_eq!(m.tools["node"].expect(), "");
        assert_eq!(m.simulator_runtimes, ["iOS 26.0"]);
    }
}
