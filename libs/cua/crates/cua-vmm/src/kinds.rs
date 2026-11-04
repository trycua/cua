//! Which software runs each kind of Space here: the per-kind runtime
//! settings (`runtime.container`, `runtime.vm`, `runtime.microvm`, each
//! `auto`, `builtin` or `system`, and `runtime.<kind>_path`), the runtimes
//! already installed on this machine for each kind, and what a given
//! binary is.
//!
//! * **One setting per kind.** `runtime.container` was `runtime.linux` and
//!   `runtime.vm` was `runtime.lume`: the old names are still read (the new
//!   ones win), in the config file and the environment.
//! * **A path overrides detection.** With a kind set to `system`, its
//!   `runtime.<kind>_path` names the binary to use instead of the one
//!   found on `PATH` ([`configured_bin`]); the daemon and `cua` both read
//!   it, so they use the same one.
//! * **Detection** ([`detect`]) lists, per kind, every runtime this machine
//!   has (name, path, version), in the order the one that is used is
//!   picked; [`probe`] tells what any binary is (it runs `--version`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::host::{self, HostOs};

/// A kind of Space with a runtime setting of its own (`--kind`'s words).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeKind {
    Container,
    Vm,
    Microvm,
}

impl RuntimeKind {
    /// Every kind, in Settings' order.
    pub const ALL: [RuntimeKind; 3] = [
        RuntimeKind::Container,
        RuntimeKind::Vm,
        RuntimeKind::Microvm,
    ];

    /// `container`, `vm`, `microvm`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Container => "container",
            Self::Vm => "vm",
            Self::Microvm => "microvm",
        }
    }

    /// The kind `word` names (`containers`, `virtual machines` too).
    pub fn parse(word: &str) -> Option<Self> {
        match word.trim().to_ascii_lowercase().as_str() {
            "container" | "containers" => Some(Self::Container),
            "vm" | "vms" | "virtual machine" | "virtual machines" => Some(Self::Vm),
            "microvm" | "microvms" => Some(Self::Microvm),
            _ => None,
        }
    }

    /// The per-OS setting this kind's replaced (`[runtime] <name>` and its
    /// variable), still read when the new one is not set.
    fn legacy(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Container => Some(("linux", "CUA_RUNTIME_LINUX")),
            Self::Vm => Some(("lume", "CUA_RUNTIME_LUME")),
            Self::Microvm => None,
        }
    }
}

/// What a kind's setting asks for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KindSource {
    /// The best of what is here (the kind's own rules).
    #[default]
    Auto,
    /// Cua's own (set up on first use).
    Builtin,
    /// Only what is already installed here (nothing is downloaded).
    System,
}

impl KindSource {
    /// `auto`, `builtin` (`built-in`) or `system`.
    pub fn parse(word: &str) -> Option<Self> {
        match word.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Some(Self::Auto),
            "builtin" | "built-in" | "cua" => Some(Self::Builtin),
            "system" | "installed" => Some(Self::System),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Builtin => "builtin",
            Self::System => "system",
        }
    }
}

/// `runtime.<kind><suffix>` now: its variable (`CUA_RUNTIME_<KIND><SUFFIX>`,
/// then the old per-OS one), else the config file (the new name, then the
/// old one). Read on every use, so a change in Settings applies at once.
fn setting(kind: RuntimeKind, suffix: &str, config: &Path) -> Option<String> {
    let name = format!("{}{suffix}", kind.as_str());
    let legacy = kind.legacy().filter(|_| suffix.is_empty());
    let env = std::iter::once(format!("CUA_RUNTIME_{}", name.to_ascii_uppercase()))
        .chain(legacy.map(|(_, e)| e.to_string()))
        .find_map(|e| std::env::var(e).ok().filter(|v| !v.trim().is_empty()));
    env.or_else(|| {
        let doc = std::fs::read_to_string(config)
            .ok()?
            .parse::<toml_edit::DocumentMut>()
            .ok()?;
        let table = doc.get("runtime")?;
        std::iter::once(name.as_str())
            .chain(legacy.map(|(n, _)| n))
            .find_map(|n| table.get(n)?.as_str().map(str::to_string))
    })
    .map(|v| v.trim().to_string())
    .filter(|v| !v.is_empty())
}

fn config_file() -> PathBuf {
    host::cua_home().join("config.toml")
}

/// `runtime.<kind>` now (auto when unset or unreadable).
pub fn source(kind: RuntimeKind) -> KindSource {
    source_in(kind, &config_file())
}

/// [`source`] with the config file at `config`.
pub fn source_in(kind: RuntimeKind, config: &Path) -> KindSource {
    setting(kind, "", config)
        .and_then(|v| KindSource::parse(&v))
        .unwrap_or_default()
}

/// `runtime.<kind>_path` now, whatever the source says.
pub fn custom_path(kind: RuntimeKind) -> Option<PathBuf> {
    custom_path_in(kind, &config_file())
}

/// [`custom_path`] with the config file at `config`.
pub fn custom_path_in(kind: RuntimeKind, config: &Path) -> Option<PathBuf> {
    setting(kind, "_path", config).map(PathBuf::from)
}

/// The binary the settings pin for `kind`: its custom path, when the kind
/// is set to `system` (Automatic and Cua's own pick their own). This
/// overrides detection everywhere the kind's binary is looked up.
pub fn configured_bin(kind: RuntimeKind) -> Option<PathBuf> {
    (source(kind) == KindSource::System)
        .then(|| custom_path(kind))
        .flatten()
}

/// With `kind` set to `system`, the binary to run: its custom path, else
/// the first of its tools installed here (never one cua set up itself).
/// `None` for Automatic and Cua's own, and when nothing is installed.
pub fn system_bin(kind: RuntimeKind) -> Option<PathBuf> {
    if source(kind) != KindSource::System {
        return None;
    }
    custom_path(kind).or_else(|| first_installed(kind))
}

/// The first of `kind`'s tools on `PATH` here (not cua's own copies).
pub fn first_installed(kind: RuntimeKind) -> Option<PathBuf> {
    tools_for(kind, HostOs::current())
        .into_iter()
        .flat_map(program_names)
        .filter_map(|n| host::which(&n))
        .find(|p| !p.starts_with(host::cua_home()))
}

/// A runtime program a kind can use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tool {
    /// `docker`, `podman`, `lume`, `qemu`, `container`, `firecracker`.
    pub id: &'static str,
    /// As people know it (`Docker`, `Apple container`).
    pub name: &'static str,
    pub kind: RuntimeKind,
    /// The systems it runs on.
    pub hosts: &'static [HostOs],
    /// The guest systems it runs, when only one ("macOS", "Linux").
    pub guests: Option<&'static str>,
    /// The oldest version cua runs on.
    pub min_version: Option<&'static str>,
}

/// `lume::builtin::MIN_VERSION` (a test keeps them equal; this module
/// builds without the `lume` feature too).
const LUME_MIN_VERSION: &str = "0.6.0";

const MAC: &[HostOs] = &[HostOs::Macos];
const LINUX: &[HostOs] = &[HostOs::Linux];
const MAC_LINUX: &[HostOs] = &[HostOs::Macos, HostOs::Linux];

/// Every tool, per kind in the order detection prefers them.
pub const TOOLS: &[Tool] = &[
    Tool {
        id: "docker",
        name: "Docker",
        kind: RuntimeKind::Container,
        hosts: MAC_LINUX,
        guests: None,
        min_version: None,
    },
    Tool {
        id: "podman",
        name: "Podman",
        kind: RuntimeKind::Container,
        hosts: MAC_LINUX,
        guests: None,
        min_version: None,
    },
    Tool {
        id: "lume",
        name: "Lume",
        kind: RuntimeKind::Vm,
        hosts: MAC,
        guests: Some("macOS"),
        min_version: Some(LUME_MIN_VERSION),
    },
    Tool {
        id: "qemu",
        name: "QEMU",
        kind: RuntimeKind::Vm,
        hosts: LINUX,
        guests: None,
        min_version: None,
    },
    Tool {
        id: "container",
        name: "Apple container",
        kind: RuntimeKind::Microvm,
        hosts: MAC,
        guests: Some("Linux"),
        min_version: None,
    },
    Tool {
        id: "firecracker",
        name: "Firecracker",
        kind: RuntimeKind::Microvm,
        hosts: LINUX,
        guests: None,
        min_version: None,
    },
];

/// The tools `kind` can use on `os`.
pub fn tools_for(kind: RuntimeKind, os: HostOs) -> Vec<&'static Tool> {
    TOOLS
        .iter()
        .filter(|t| t.kind == kind && t.hosts.contains(&os))
        .collect()
}

/// The program names a tool is looked up by on `PATH`.
fn program_names(tool: &Tool) -> Vec<String> {
    match tool.id {
        "lume" => vec![
            cua_home::channel::CuaChannel::current()
                .lume_bin_name()
                .to_string(),
        ],
        "qemu" => vec![format!("qemu-system-{}", crate::types::Arch::host().qemu())],
        id => vec![id.to_string()],
    }
}

/// A runtime found on this machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Detected {
    /// [`Tool::id`].
    pub tool: String,
    /// [`Tool::name`].
    pub name: String,
    pub path: PathBuf,
    /// As `--version` says it (`27.3.1`), when it says.
    pub version: Option<String>,
    /// [`Tool::guests`].
    pub guests: Option<String>,
}

/// What a binary is, for the path a person typed (or the one in use).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathProbe {
    pub path: PathBuf,
    /// A file is there.
    pub exists: bool,
    /// It can be run.
    pub executable: bool,
    /// The tool it is ([`Tool::id`]), when it is one of the kind's.
    pub tool: Option<String>,
    /// [`Tool::name`].
    pub name: Option<String>,
    pub version: Option<String>,
    /// The tool's [`Tool::min_version`].
    pub min_version: Option<String>,
    /// The names of the tools the kind takes here ("Docker", "Podman").
    pub expected: Vec<String>,
    /// [`Tool::guests`].
    pub guests: Option<String>,
}

/// The first `x.y` or `x.y.z` in `text` (`Docker version 27.3.1, build
/// ce12230` is `27.3.1`; `Firecracker v1.10.1` is `1.10.1`).
pub fn version_in(text: &str) -> Option<String> {
    text.split(|c: char| c.is_whitespace() || c == ',' || c == '(' || c == ')')
        .map(|w| w.trim_start_matches('v'))
        .find(|w| {
            let parts: Vec<&str> = w.split('.').collect();
            parts.len() >= 2
                && parts.len() <= 4
                && parts
                    .iter()
                    .take(2)
                    .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        })
        .map(|w| {
            w.trim_end_matches(|c: char| !c.is_ascii_digit())
                .to_string()
        })
}

/// Which of the tools `text` (a `--version` answer) comes from, judged by
/// what each one prints and, for Lume (it prints only its version), the
/// file's name.
pub fn identify(file_name: &str, text: &str) -> Option<&'static Tool> {
    let t = text.to_ascii_lowercase();
    let by = |id: &str| TOOLS.iter().find(|x| x.id == id);
    if t.contains("docker version") {
        by("docker")
    } else if t.contains("podman version") {
        by("podman")
    } else if t.contains("qemu emulator version") {
        by("qemu")
    } else if t.contains("container cli version")
        || t.contains("container version") && t.contains("apple")
    {
        by("container")
    } else if t.contains("firecracker") {
        by("firecracker")
    } else if file_name.to_ascii_lowercase().starts_with("lume") && version_in(text).is_some() {
        by("lume")
    } else {
        None
    }
}

/// How long `--version` may take.
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);

/// `<bin> --version`'s answer (stdout, else stderr), when it answers in time.
async fn version_output(bin: &Path) -> Option<String> {
    let mut cmd = version_command(bin);
    let out = tokio::time::timeout(
        VERSION_TIMEOUT,
        cmd.arg("--version")
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    let text = [&out.stdout, &out.stderr]
        .iter()
        .map(|b| String::from_utf8_lossy(b).trim().to_string())
        .find(|s| !s.is_empty())?;
    Some(text)
}

/// The command a binary's `--version` runs as: a Lume as its channel runs
/// it (Local's keeps its own config), anything else as it is.
fn version_command(bin: &Path) -> tokio::process::Command {
    #[cfg(feature = "lume")]
    if bin
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("lume"))
        && let Ok(l) = crate::lume::builtin::LumeCmd::new(
            cua_home::channel::CuaChannel::current(),
            &host::cua_home().join("vmm").join("lume"),
            bin.to_path_buf(),
        )
    {
        return l.tokio_command();
    }
    tokio::process::Command::new(bin)
}

fn executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        p.metadata()
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

/// What `path` is, for `kind` on this machine: there, runnable, which tool
/// (only the kind's own count) and its version. Runs `path --version`
/// (bounded); never anything else.
pub async fn probe(kind: RuntimeKind, path: &Path) -> PathProbe {
    probe_on(kind, path, HostOs::current()).await
}

async fn probe_on(kind: RuntimeKind, path: &Path, os: HostOs) -> PathProbe {
    let tools = tools_for(kind, os);
    let mut p = PathProbe {
        path: path.to_path_buf(),
        exists: path.exists(),
        expected: tools.iter().map(|t| t.name.to_string()).collect(),
        ..Default::default()
    };
    p.executable = p.exists && executable(path);
    if !p.executable {
        return p;
    }
    let Some(text) = version_output(path).await else {
        return p;
    };
    let file = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if let Some(tool) = identify(file, &text).filter(|t| tools.iter().any(|x| x.id == t.id)) {
        p.tool = Some(tool.id.into());
        p.name = Some(tool.name.into());
        p.min_version = tool.min_version.map(str::to_string);
        p.guests = tool.guests.map(str::to_string);
        p.version = version_in(&text);
    }
    p
}

/// The runtimes installed here for `kind`, in the order the one used is
/// picked (the first is what Automatic and "Already installed" use when no
/// path is set). Never anything cua set up itself (its own Lume, its own
/// `container`, its own Firecracker).
pub async fn detect(kind: RuntimeKind) -> Vec<Detected> {
    let mut out: Vec<Detected> = Vec::new();
    for tool in tools_for(kind, HostOs::current()) {
        for name in program_names(tool) {
            let Some(path) = host::which(&name) else {
                continue;
            };
            if path.starts_with(host::cua_home()) || out.iter().any(|d| d.path == path) {
                continue;
            }
            let version = version_output(&path).await.and_then(|t| version_in(&t));
            out.push(Detected {
                tool: tool.id.into(),
                name: tool.name.into(),
                path,
                version,
                guests: tool.guests.map(str::to_string),
            });
        }
    }
    out
}

/// One kind as Settings shows it: its setting, its custom path, what is
/// installed here and what the path in use is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KindReport {
    pub kind: RuntimeKind,
    pub source: KindSource,
    /// `runtime.<kind>_path`, when set.
    pub path: Option<PathBuf>,
    pub detected: Vec<Detected>,
    /// [`probe`] of the path in use (the custom one, else the first
    /// detected), when there is one.
    pub check: Option<PathProbe>,
    /// The names of the tools the kind takes here (`Docker`, `Podman`).
    pub expected: Vec<String>,
}

/// [`KindReport`] for every kind, read now.
pub async fn report() -> Vec<KindReport> {
    let mut out = Vec::new();
    for kind in RuntimeKind::ALL {
        let detected = detect(kind).await;
        let path = custom_path(kind);
        let in_use = path
            .clone()
            .or_else(|| detected.first().map(|d| d.path.clone()));
        let check = match in_use {
            Some(p) => Some(probe(kind, &p).await),
            None => None,
        };
        out.push(KindReport {
            kind,
            source: source(kind),
            path,
            detected,
            check,
            expected: tools_for(kind, HostOs::current())
                .iter()
                .map(|t| t.name.to_string())
                .collect(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_and_tools_are_read_from_version_answers() {
        assert_eq!(
            version_in("Docker version 27.3.1, build ce12230").as_deref(),
            Some("27.3.1")
        );
        assert_eq!(version_in("podman version 5.2.1").as_deref(), Some("5.2.1"));
        assert_eq!(
            version_in("Firecracker v1.10.1\n\nSupported snapshot data format versions: v1.0.0")
                .as_deref(),
            Some("1.10.1")
        );
        assert_eq!(
            version_in("QEMU emulator version 9.1.0").as_deref(),
            Some("9.1.0")
        );
        assert_eq!(
            version_in("container CLI version 0.6.0 (build: release, commit: 1a2b3c4)").as_deref(),
            Some("0.6.0")
        );
        assert_eq!(version_in("0.6.0").as_deref(), Some("0.6.0"));
        assert_eq!(version_in("no version here"), None);
        let id = |f: &str, t: &str| identify(f, t).map(|t| t.id);
        assert_eq!(
            id("docker", "Docker version 27.3.1, build ce12230"),
            Some("docker")
        );
        assert_eq!(id("podman", "podman version 5.2.1"), Some("podman"));
        assert_eq!(
            id("container", "container CLI version 0.6.0 (build: release)"),
            Some("container")
        );
        assert_eq!(
            id("firecracker", "Firecracker v1.10.1"),
            Some("firecracker")
        );
        assert_eq!(
            id("qemu-system-aarch64", "QEMU emulator version 9.1.0"),
            Some("qemu")
        );
        assert_eq!(id("lume", "0.6.0"), Some("lume"));
        // Only a Lume answers with a bare version.
        assert_eq!(id("sh", "0.6.0"), None);
        assert_eq!(id("ls", "ls: illegal option -- -"), None);
    }

    #[cfg(feature = "lume")]
    #[test]
    fn lume_s_oldest_version_is_the_one_cua_runs_on() {
        assert_eq!(LUME_MIN_VERSION, crate::lume::builtin::MIN_VERSION);
    }

    #[test]
    fn each_kind_takes_its_own_tools_per_system() {
        let ids = |k, os| tools_for(k, os).iter().map(|t| t.id).collect::<Vec<_>>();
        assert_eq!(
            ids(RuntimeKind::Container, HostOs::Macos),
            ["docker", "podman"]
        );
        assert_eq!(
            ids(RuntimeKind::Container, HostOs::Linux),
            ["docker", "podman"]
        );
        assert_eq!(ids(RuntimeKind::Vm, HostOs::Macos), ["lume"]);
        assert_eq!(ids(RuntimeKind::Vm, HostOs::Linux), ["qemu"]);
        assert_eq!(ids(RuntimeKind::Microvm, HostOs::Macos), ["container"]);
        assert_eq!(ids(RuntimeKind::Microvm, HostOs::Linux), ["firecracker"]);
        assert_eq!(
            RuntimeKind::parse("Virtual machines"),
            Some(RuntimeKind::Vm)
        );
        assert_eq!(KindSource::parse("Built-in"), Some(KindSource::Builtin));
    }

    /// The new name wins; the old per-OS name still counts; a path is read
    /// whatever the source.
    #[test]
    fn settings_read_new_then_old_names() {
        let d = tempfile::tempdir().unwrap();
        let config = d.path().join("config.toml");
        std::fs::write(
            &config,
            "[runtime]\nlinux = \"builtin\"\nlume = \"system\"\n",
        )
        .unwrap();
        assert_eq!(
            source_in(RuntimeKind::Container, &config),
            KindSource::Builtin
        );
        assert_eq!(source_in(RuntimeKind::Vm, &config), KindSource::System);
        assert_eq!(source_in(RuntimeKind::Microvm, &config), KindSource::Auto);
        std::fs::write(
            &config,
            "[runtime]\nlinux = \"builtin\"\ncontainer = \"system\"\ncontainer_path = \"/opt/homebrew/bin/podman\"\n",
        )
        .unwrap();
        assert_eq!(
            source_in(RuntimeKind::Container, &config),
            KindSource::System
        );
        assert_eq!(
            custom_path_in(RuntimeKind::Container, &config),
            Some(PathBuf::from("/opt/homebrew/bin/podman"))
        );
        assert_eq!(custom_path_in(RuntimeKind::Vm, &config), None);
    }

    /// A fake Docker: what it is and its version; a script that is not one;
    /// a file that cannot run; nothing there.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_path_is_probed_for_what_it_is() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let script = |name: &str, body: &str, mode: u32| {
            let p = d.path().join(name);
            std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
            p
        };
        let docker = script(
            "docker",
            "echo 'Docker version 27.3.1, build ce12230'",
            0o755,
        );
        let p = probe_on(RuntimeKind::Container, &docker, HostOs::Macos).await;
        assert!(p.exists && p.executable);
        assert_eq!(
            (p.tool.as_deref(), p.version.as_deref()),
            (Some("docker"), Some("27.3.1"))
        );
        assert_eq!(p.expected, ["Docker", "Podman"]);
        // A Docker is not a microVM runtime.
        let p = probe_on(RuntimeKind::Microvm, &docker, HostOs::Macos).await;
        assert_eq!(
            (p.tool, p.expected),
            (None, vec!["Apple container".to_string()])
        );
        let other = script("tool", "echo 'something else 1.0'", 0o755);
        assert_eq!(
            probe_on(RuntimeKind::Container, &other, HostOs::Macos)
                .await
                .tool,
            None
        );
        let plain = script("plain", "true", 0o644);
        let p = probe_on(RuntimeKind::Container, &plain, HostOs::Macos).await;
        assert!(p.exists && !p.executable);
        let p = probe_on(
            RuntimeKind::Container,
            &d.path().join("missing"),
            HostOs::Macos,
        )
        .await;
        assert!(!p.exists && !p.executable);
        let lume = script("lume", "echo 0.5.3", 0o755);
        let p = probe_on(RuntimeKind::Vm, &lume, HostOs::Macos).await;
        assert_eq!(
            (
                p.tool.as_deref(),
                p.version.as_deref(),
                p.min_version.as_deref(),
                p.guests.as_deref()
            ),
            (Some("lume"), Some("0.5.3"), Some("0.6.0"), Some("macOS"))
        );
    }
}
