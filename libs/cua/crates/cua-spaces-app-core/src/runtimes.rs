// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Settings, Advanced, "Which software runs Spaces": one row per kind of
//! Space (Containers, Virtual machines, MicroVMs), each Automatic, Cua's
//! own or Already installed (`runtime.<kind>`).
//!
//! * Automatic and Cua's own show nothing more.
//! * Already installed shows the runtimes the host found for the kind
//!   (name, path, version) and the path that will be used, which a person
//!   can change (`runtime.<kind>_path`): Detected while it is one of the
//!   found ones, Custom otherwise (with "Reset to detected"), and one line
//!   about what that path is ([`path_verdict`]).
//! * A kind this machine cannot run at all says "Not available" and why,
//!   from the runtime doctor; before the doctor answered, "Checking…".
//!
//! Every row is always there, on every host, whatever the host knows yet.

use serde::{Deserialize, Serialize};

use crate::model::SpaceOs;
use crate::settings::{SettingsOption, SettingsRow, SettingsRowKind, SettingsSection, row};

/// A runtime the host found for one kind (the SDK's `RuntimeDetected`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DetectedRuntime {
    /// `docker`, `podman`, `lume`, `qemu`, `container`, `firecracker`.
    pub tool: String,
    /// As people know it (`Docker`, `Apple container`).
    pub name: String,
    pub path: String,
    /// As its `--version` says it (`27.3.1`).
    pub version: Option<String>,
    /// The one guest system it runs, when only one (`macOS`, `Linux`).
    pub guests: Option<String>,
}

/// What a binary is for one kind (the SDK's `RuntimePathProbe`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RuntimePathCheck {
    pub path: String,
    /// A file is there.
    pub exists: bool,
    /// It can be run.
    pub executable: bool,
    /// The tool it is, when it is one the kind takes.
    pub tool: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
    /// The oldest version Cua runs on.
    pub min_version: Option<String>,
    /// The tools the kind takes here (`Docker`, `Podman`).
    pub expected: Vec<String>,
    pub guests: Option<String>,
}

/// One kind as the host read it (the SDK's `RuntimeKindState`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RuntimeKindInput {
    /// `container`, `vm` or `microvm`.
    pub kind: String,
    /// `runtime.<kind>`: `auto`, `builtin`, `system` (none: auto).
    pub source: Option<String>,
    /// `runtime.<kind>_path`, when set.
    pub path: Option<String>,
    /// What is installed for the kind, the one used first (none: not
    /// looked for yet).
    pub detected: Option<Vec<DetectedRuntime>>,
    /// The path in use, probed.
    pub check: Option<RuntimePathCheck>,
    /// The tools the kind takes on this machine (`Docker`, `Podman`).
    pub expected: Vec<String>,
}

/// What Settings knows about this machine's runtimes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RuntimesInput {
    /// This machine's system (none: not said; every option is offered).
    pub os: Option<SpaceOs>,
    /// `arm64`, `amd64`.
    pub host_arch: Option<String>,
    /// The runtime doctor's ready backends (none: not read yet).
    pub local_backends: Option<Vec<String>>,
    /// Why each other backend is not ready.
    pub local_details: Option<std::collections::HashMap<String, String>>,
    /// Each kind's setting and what is installed for it (none: not read).
    pub kinds: Option<Vec<RuntimeKindInput>>,
}

/// A path's one line: whether it can be used, and what it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimePathVerdict {
    pub ok: bool,
    pub line: String,
}

/// The kinds, in Settings' order: id and label.
pub const KINDS: [(&str, &str); 3] = [
    ("container", "Containers"),
    ("vm", "Virtual machines"),
    ("microvm", "MicroVMs"),
];

/// The options' words.
pub const AUTOMATIC: &str = "Automatic";
pub const CUAS_OWN: &str = "Cua\u{2019}s own";
pub const INSTALLED: &str = "Already installed";
pub const NOT_AVAILABLE: &str = "Not available";
pub const CHECKING: &str = "Checking\u{2026}";
pub const DETECTED: &str = "Detected";
pub const CUSTOM: &str = "Custom";
pub const RESET: &str = "Reset to detected";

/// Whether a kind runs here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    /// The doctor has not answered.
    Checking,
    Ready,
    /// Why not, as a sentence about the kind ("MicroVMs need macOS 26").
    Unavailable(String),
}

fn is_mac(os: Option<SpaceOs>) -> bool {
    matches!(os, Some(SpaceOs::Macos) | None)
}

/// Whether `kind` runs on this machine, from its runtime doctor.
pub fn availability(kind: &str, input: &RuntimesInput) -> Availability {
    let Some(backends) = input.local_backends.as_deref() else {
        return Availability::Checking;
    };
    let has = |names: &[&str]| backends.iter().any(|b| names.contains(&b.as_str()));
    let intel = crate::wizard::arch_word(input.host_arch.as_deref()) == Some("amd64");
    match kind {
        // A Mac always has Cua's own (a VM with Docker and gVisor).
        "container" if is_mac(input.os) => Availability::Ready,
        "container" => {
            // The names New Space reads a container engine by.
            let engines = crate::wizard::LocalEngine::Container.backends();
            if backends.iter().any(|b| engines.contains(b)) {
                Availability::Ready
            } else {
                Availability::Unavailable("Containers need Docker or Podman".into())
            }
        }
        // Lume (Cua's own, or this Mac's) runs on Apple silicon only.
        "vm" if is_mac(input.os) => {
            if intel && !has(&["qemu"]) {
                Availability::Unavailable("Virtual machines need a Mac with Apple silicon".into())
            } else {
                Availability::Ready
            }
        }
        "vm" if has(&["qemu", "lume"]) => Availability::Ready,
        "vm" => Availability::Unavailable("Virtual machines need QEMU".into()),
        "microvm" => {
            use crate::wizard::{LocalProbe, MicrovmStatus, microvm_status_of};
            let probe = LocalProbe {
                backends: input.local_backends.as_deref(),
                details: input.local_details.as_ref(),
                host_arch: input.host_arch.as_deref(),
            };
            match microvm_status_of(&probe, input.os.unwrap_or(SpaceOs::Macos)) {
                MicrovmStatus::Checking => Availability::Checking,
                MicrovmStatus::Ready(_) => Availability::Ready,
                MicrovmStatus::Blocked(why) => {
                    // "Needs macOS 26." as "MicroVMs need macOS 26".
                    let mut c = why.trim().chars();
                    let why = c
                        .next()
                        .map(|f| f.to_lowercase().chain(c).collect::<String>())
                        .unwrap_or_default();
                    let why = why
                        .strip_prefix("needs ")
                        .map(|w| format!("need {w}"))
                        .unwrap_or(why);
                    Availability::Unavailable(format!("MicroVMs {}", why.trim_end_matches('.')))
                }
            }
        }
        _ => Availability::Ready,
    }
}

/// Whether Cua has its own runtime for `kind` on `os`.
fn has_own(kind: &str, os: Option<SpaceOs>) -> bool {
    match kind {
        // A VM with Docker and gVisor; a built-in Lume: Mac only.
        "container" | "vm" => is_mac(os),
        // Cua's monitor on a Mac, its own Firecracker on Linux.
        "microvm" => matches!(os, Some(SpaceOs::Macos) | Some(SpaceOs::Linux) | None),
        _ => false,
    }
}

/// `auto`, `builtin` or `system` (anything else: auto).
fn source_of(word: Option<&str>) -> &'static str {
    match word.map(|w| w.trim().to_ascii_lowercase()).as_deref() {
        Some("builtin" | "built-in") => "builtin",
        Some("system") => "system",
        _ => "auto",
    }
}

/// `major.minor` of a version (`27.3.1` is `27.3`).
pub fn short_version(v: &str) -> String {
    let parts: Vec<&str> = v.trim().split('.').collect();
    if parts.len() >= 2 {
        format!("{}.{}", parts[0], parts[1])
    } else {
        v.trim().to_string()
    }
}

/// A detected runtime as its option reads: `Docker — /usr/local/bin/docker
/// (27.3)`, with the one guest system it runs when only one.
pub fn detected_label(d: &DetectedRuntime) -> String {
    let mut s = format!("{} \u{2014} {}", d.name, d.path);
    if let Some(v) = d.version.as_deref().filter(|v| !v.is_empty()) {
        s.push_str(&format!(" ({})", short_version(v)));
    }
    if let Some(g) = d.guests.as_deref().filter(|g| !g.is_empty()) {
        s.push_str(&format!(" \u{b7} {g} only"));
    }
    s
}

fn version_tuple(v: &str) -> Vec<u64> {
    v.trim()
        .trim_start_matches('v')
        .split('.')
        .map(|p| {
            p.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
        })
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

/// Whether `v` is older than `min`.
fn older(v: &str, min: &str) -> bool {
    let (mut a, mut b) = (version_tuple(v), version_tuple(min));
    let n = a.len().max(b.len());
    a.resize(n, 0);
    b.resize(n, 0);
    a < b
}

/// "A", "A or B", "A, B or C".
fn join_or(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

/// What a path is, in one line: "Docker 27.3 found", "Not a Docker or
/// Podman binary", "Lume 0.5.3 is too old (needs 0.6.0)".
pub fn path_verdict(check: &RuntimePathCheck) -> RuntimePathVerdict {
    let bad = |line: String| RuntimePathVerdict { ok: false, line };
    if check.path.trim().is_empty() {
        return bad("No path".into());
    }
    if !check.exists {
        return bad("Nothing at this path".into());
    }
    if !check.executable {
        return bad("Not a program that can run".into());
    }
    let Some(tool) = check.tool.as_deref() else {
        return bad(if check.expected.is_empty() {
            "Not a runtime Cua knows".into()
        } else {
            format!("Not a {} binary", join_or(&check.expected))
        });
    };
    let name = check.name.clone().unwrap_or_else(|| tool.to_string());
    if let (Some(v), Some(min)) = (check.version.as_deref(), check.min_version.as_deref())
        && older(v, min)
    {
        return bad(format!("{name} {v} is too old (needs {min})"));
    }
    let mut line = match check.version.as_deref() {
        Some(v) => format!("{name} {} found", short_version(v)),
        None => format!("{name} found"),
    };
    if let Some(g) = check.guests.as_deref() {
        line.push_str(&format!(" \u{b7} runs {g} only"));
    }
    RuntimePathVerdict { ok: true, line }
}

/// The path a kind uses: its custom path, else the first runtime found.
pub fn path_in_use(k: &RuntimeKindInput) -> Option<String> {
    k.path
        .clone()
        .filter(|p| !p.trim().is_empty())
        .or_else(|| k.detected.as_ref()?.first().map(|d| d.path.clone()))
}

/// The custom path is none of the detected ones.
pub fn is_custom(k: &RuntimeKindInput) -> bool {
    match k.path.as_deref().filter(|p| !p.trim().is_empty()) {
        None => false,
        Some(p) => !k.detected.iter().flatten().any(|d| d.path == p),
    }
}

fn kind_rows(kind: &str, label: &str, input: &RuntimesInput) -> Vec<SettingsRow> {
    use SettingsRowKind::*;
    let k = input
        .kinds
        .iter()
        .flatten()
        .find(|k| k.kind == kind)
        .cloned()
        .unwrap_or_else(|| RuntimeKindInput {
            kind: kind.into(),
            ..Default::default()
        });
    let id = format!("runtime:{kind}");
    let availability = availability(kind, input);
    if let Availability::Unavailable(why) = &availability {
        let mut r = row(&id, Text, label);
        r.value = Some(NOT_AVAILABLE.into());
        r.help = Some(why.clone());
        r.enabled = false;
        return vec![r];
    }
    let source = source_of(k.source.as_deref());
    let mut main = row(&id, Choice, label);
    main.options = [
        ("auto", AUTOMATIC),
        ("builtin", CUAS_OWN),
        ("system", INSTALLED),
    ]
    .into_iter()
    .filter(|(o, _)| *o != "builtin" || has_own(kind, input.os) || source == "builtin")
    .map(|(o, l)| SettingsOption {
        id: o.into(),
        label: l.into(),
        active: o == source,
    })
    .collect();
    if availability == Availability::Checking {
        main.help = Some(CHECKING.into());
    }
    let mut rows = vec![main];
    if source != "system" {
        return rows;
    }
    let in_use = path_in_use(&k);
    let custom = is_custom(&k);
    let detected_id = format!("runtime-detected:{kind}");
    rows.push(match k.detected.as_deref() {
        None => row(&detected_id, Note, "Looking for what is installed\u{2026}"),
        Some([]) => {
            let label = if k.expected.is_empty() {
                "Nothing found on this machine".to_string()
            } else {
                format!("No {} found on this machine", join_or(&k.expected))
            };
            row(&detected_id, Note, &label)
        }
        Some(found) => {
            let mut r = row(&detected_id, Choice, "Installed");
            r.options = found
                .iter()
                .map(|d| SettingsOption {
                    id: d.path.clone(),
                    label: detected_label(d),
                    active: !custom && in_use.as_deref() == Some(d.path.as_str()),
                })
                .collect();
            r
        }
    });
    let mut path = row(
        &format!("runtime-path:{kind}"),
        Field,
        if custom { CUSTOM } else { DETECTED },
    );
    path.value = in_use.clone();
    path.placeholder = Some("Full path of the program".into());
    if custom {
        path.button = Some(RESET.into());
    }
    rows.push(path);
    let check_id = format!("runtime-check:{kind}");
    match (&k.check, &in_use) {
        (Some(c), Some(p)) if &c.path == p => {
            let v = path_verdict(c);
            rows.push(row(&check_id, if v.ok { Note } else { Error }, &v.line));
        }
        (_, Some(_)) => rows.push(row(&check_id, Note, CHECKING)),
        _ => {}
    }
    rows
}

/// Settings' Runtimes section: every kind's rows, whatever is known.
pub fn section(input: Option<&RuntimesInput>) -> SettingsSection {
    let empty = RuntimesInput::default();
    let input = input.unwrap_or(&empty);
    SettingsSection {
        id: "runtimes".into(),
        title: "Runtimes".into(),
        button: None,
        button_enabled: false,
        button_help: None,
        rows: KINDS
            .iter()
            .flat_map(|(kind, label)| kind_rows(kind, label, input))
            .collect(),
    }
}

/// What a row in the section changes: the setting and its new value
/// (`None`: unset it, back to the default or the detected path).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSettingChange {
    /// `runtime.container`, `runtime.vm_path`.
    pub key: String,
    pub value: Option<String>,
}

/// The setting a choice in the section changes, for the host to write
/// (none: not a runtime row). Picking the first detected runtime unsets
/// the path (detection then decides); another detected one, or a typed
/// path, sets it; an empty path ("Reset to detected") unsets it.
pub fn choose(
    input: Option<&RuntimesInput>,
    row_id: &str,
    option: &str,
) -> Option<RuntimeSettingChange> {
    let (prefix, kind) = row_id.split_once(':')?;
    if !KINDS.iter().any(|(k, _)| *k == kind) {
        return None;
    }
    let change = |suffix: &str, value: Option<String>| RuntimeSettingChange {
        key: format!("runtime.{kind}{suffix}"),
        value,
    };
    match prefix {
        "runtime" => Some(change("", Some(source_of(Some(option)).into()))),
        "runtime-path" => Some(change(
            "_path",
            Some(option.trim().to_string()).filter(|p| !p.is_empty()),
        )),
        "runtime-detected" => {
            let first = input
                .and_then(|i| i.kinds.as_ref())
                .and_then(|ks| ks.iter().find(|k| k.kind == kind))
                .and_then(|k| k.detected.as_ref()?.first().map(|d| d.path.clone()));
            Some(change(
                "_path",
                (first.as_deref() != Some(option)).then(|| option.to_string()),
            ))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn docker() -> DetectedRuntime {
        DetectedRuntime {
            tool: "docker".into(),
            name: "Docker".into(),
            path: "/usr/local/bin/docker".into(),
            version: Some("27.3.1".into()),
            guests: None,
        }
    }

    fn podman() -> DetectedRuntime {
        DetectedRuntime {
            tool: "podman".into(),
            name: "Podman".into(),
            path: "/opt/homebrew/bin/podman".into(),
            version: Some("5.2.1".into()),
            guests: None,
        }
    }

    fn ok_check(path: &str, name: &str, version: &str) -> RuntimePathCheck {
        RuntimePathCheck {
            path: path.into(),
            exists: true,
            executable: true,
            tool: Some(name.to_ascii_lowercase()),
            name: Some(name.into()),
            version: Some(version.into()),
            expected: vec!["Docker".into(), "Podman".into()],
            ..Default::default()
        }
    }

    fn mac(kinds: Vec<RuntimeKindInput>) -> RuntimesInput {
        RuntimesInput {
            os: Some(SpaceOs::Macos),
            host_arch: Some("arm64".into()),
            local_backends: Some(vec!["lume".into(), "managed".into(), "vz".into()]),
            local_details: None,
            kinds: Some(kinds),
        }
    }

    fn ids(s: &SettingsSection) -> Vec<&str> {
        s.rows.iter().map(|r| r.id.as_str()).collect()
    }

    fn get<'a>(s: &'a SettingsSection, id: &str) -> &'a SettingsRow {
        s.rows
            .iter()
            .find(|r| r.id == id)
            .unwrap_or_else(|| panic!("no {id} in {:?}", ids(s)))
    }

    fn active(r: &SettingsRow) -> Option<&str> {
        r.options.iter().find(|o| o.active).map(|o| o.id.as_str())
    }

    /// Before the host knew anything, every kind's row is there, checking.
    #[test]
    fn every_kind_has_a_row_before_anything_is_known() {
        let s = section(None);
        assert_eq!(
            ids(&s),
            ["runtime:container", "runtime:vm", "runtime:microvm"]
        );
        for r in &s.rows {
            assert_eq!(r.kind, SettingsRowKind::Choice);
            assert_eq!(r.help.as_deref(), Some(CHECKING));
            assert_eq!(active(r), Some("auto"));
            let labels: Vec<_> = r.options.iter().map(|o| o.label.as_str()).collect();
            assert_eq!(labels, [AUTOMATIC, CUAS_OWN, INSTALLED]);
        }
        let labels: Vec<_> = s.rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["Containers", "Virtual machines", "MicroVMs"]);
    }

    /// Automatic and Cua's own show nothing more.
    #[test]
    fn automatic_and_cuas_own_show_nothing_more() {
        let s = section(Some(&mac(vec![
            RuntimeKindInput {
                kind: "container".into(),
                source: Some("auto".into()),
                detected: Some(vec![docker()]),
                ..Default::default()
            },
            RuntimeKindInput {
                kind: "vm".into(),
                source: Some("builtin".into()),
                ..Default::default()
            },
        ])));
        assert_eq!(
            ids(&s),
            ["runtime:container", "runtime:vm", "runtime:microvm"]
        );
        assert_eq!(active(get(&s, "runtime:vm")), Some("builtin"));
        assert!(s.rows.iter().all(|r| r.help.is_none()));
    }

    /// Already installed: the runtimes found, the path in use autofilled
    /// from the first, Detected, and what it is.
    #[test]
    fn already_installed_lists_what_is_found_and_the_path_used() {
        let s = section(Some(&mac(vec![RuntimeKindInput {
            kind: "container".into(),
            source: Some("system".into()),
            detected: Some(vec![docker(), podman()]),
            check: Some(ok_check("/usr/local/bin/docker", "Docker", "27.3.1")),
            ..Default::default()
        }])));
        assert_eq!(
            ids(&s),
            [
                "runtime:container",
                "runtime-detected:container",
                "runtime-path:container",
                "runtime-check:container",
                "runtime:vm",
                "runtime:microvm"
            ]
        );
        let d = get(&s, "runtime-detected:container");
        let labels: Vec<_> = d.options.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Docker \u{2014} /usr/local/bin/docker (27.3)",
                "Podman \u{2014} /opt/homebrew/bin/podman (5.2)"
            ]
        );
        assert_eq!(active(d), Some("/usr/local/bin/docker"));
        let p = get(&s, "runtime-path:container");
        assert_eq!(
            (
                p.kind,
                p.label.as_str(),
                p.value.as_deref(),
                p.button.as_deref()
            ),
            (
                SettingsRowKind::Field,
                DETECTED,
                Some("/usr/local/bin/docker"),
                None
            )
        );
        let c = get(&s, "runtime-check:container");
        assert_eq!(
            (c.kind, c.label.as_str()),
            (SettingsRowKind::Note, "Docker 27.3 found")
        );
    }

    /// A custom path wins over detection: Custom, Reset to detected, and the
    /// drop-down names none of the found ones.
    #[test]
    fn a_custom_path_overrides_detection() {
        let k = RuntimeKindInput {
            kind: "container".into(),
            source: Some("system".into()),
            path: Some("/Users/me/bin/docker".into()),
            detected: Some(vec![docker()]),
            check: Some(ok_check("/Users/me/bin/docker", "Docker", "28.0.0")),
            ..Default::default()
        };
        assert_eq!(path_in_use(&k).as_deref(), Some("/Users/me/bin/docker"));
        let s = section(Some(&mac(vec![k.clone()])));
        let p = get(&s, "runtime-path:container");
        assert_eq!(
            (p.label.as_str(), p.value.as_deref(), p.button.as_deref()),
            (CUSTOM, Some("/Users/me/bin/docker"), Some(RESET))
        );
        assert_eq!(active(get(&s, "runtime-detected:container")), None);
        assert_eq!(
            get(&s, "runtime-check:container").label,
            "Docker 28.0 found"
        );
        // The second found one, picked: still Detected.
        let s = section(Some(&mac(vec![RuntimeKindInput {
            path: Some("/opt/homebrew/bin/podman".into()),
            detected: Some(vec![docker(), podman()]),
            check: Some(ok_check("/opt/homebrew/bin/podman", "Podman", "5.2.1")),
            ..k
        }])));
        assert_eq!(get(&s, "runtime-path:container").label, DETECTED);
        assert_eq!(
            active(get(&s, "runtime-detected:container")),
            Some("/opt/homebrew/bin/podman")
        );
    }

    #[test]
    fn a_bad_path_says_what_is_wrong() {
        let k = RuntimeKindInput {
            kind: "container".into(),
            source: Some("system".into()),
            path: Some("/bin/ls".into()),
            detected: Some(vec![]),
            expected: vec!["Docker".into(), "Podman".into()],
            check: Some(RuntimePathCheck {
                path: "/bin/ls".into(),
                exists: true,
                executable: true,
                expected: vec!["Docker".into(), "Podman".into()],
                ..Default::default()
            }),
        };
        let s = section(Some(&mac(vec![k])));
        let c = get(&s, "runtime-check:container");
        assert_eq!(
            (c.kind, c.label.as_str()),
            (SettingsRowKind::Error, "Not a Docker or Podman binary")
        );
        assert_eq!(
            get(&s, "runtime-detected:container").label,
            "No Docker or Podman found on this machine"
        );
    }

    #[test]
    fn verdicts() {
        let v = |c: RuntimePathCheck| path_verdict(&c);
        let base = RuntimePathCheck {
            path: "/x/docker".into(),
            expected: vec!["Docker".into(), "Podman".into()],
            ..Default::default()
        };
        assert_eq!(
            v(base.clone()),
            RuntimePathVerdict {
                ok: false,
                line: "Nothing at this path".into()
            }
        );
        assert_eq!(
            v(RuntimePathCheck {
                exists: true,
                ..base.clone()
            })
            .line,
            "Not a program that can run"
        );
        assert_eq!(
            v(RuntimePathCheck {
                exists: true,
                executable: true,
                ..base.clone()
            })
            .line,
            "Not a Docker or Podman binary"
        );
        assert_eq!(
            v(ok_check("/x/docker", "Docker", "27.3.1")),
            RuntimePathVerdict {
                ok: true,
                line: "Docker 27.3 found".into()
            }
        );
        let lume = |version: &str| RuntimePathCheck {
            path: "/opt/homebrew/bin/lume".into(),
            exists: true,
            executable: true,
            tool: Some("lume".into()),
            name: Some("Lume".into()),
            version: Some(version.into()),
            min_version: Some("0.6.0".into()),
            expected: vec!["Lume".into()],
            guests: Some("macOS".into()),
        };
        assert_eq!(
            v(lume("0.5.3")),
            RuntimePathVerdict {
                ok: false,
                line: "Lume 0.5.3 is too old (needs 0.6.0)".into()
            }
        );
        assert_eq!(
            v(lume("0.6.1")),
            RuntimePathVerdict {
                ok: true,
                line: "Lume 0.6 found \u{b7} runs macOS only".into()
            }
        );
        assert!(v(lume("0.10.0")).ok);
    }

    /// A kind that can't run here says so, and why, instead of its choices.
    #[test]
    fn a_kind_that_cannot_run_says_why() {
        let intel = RuntimesInput {
            os: Some(SpaceOs::Macos),
            host_arch: Some("amd64".into()),
            local_backends: Some(vec!["container".into()]),
            ..Default::default()
        };
        let s = section(Some(&intel));
        let vm = get(&s, "runtime:vm");
        assert_eq!(
            (vm.kind, vm.value.as_deref(), vm.help.as_deref()),
            (
                SettingsRowKind::Text,
                Some(NOT_AVAILABLE),
                Some("Virtual machines need a Mac with Apple silicon")
            )
        );
        let micro = get(&s, "runtime:microvm");
        assert_eq!(micro.value.as_deref(), Some(NOT_AVAILABLE));
        assert_eq!(
            micro.help.as_deref(),
            Some("MicroVMs need a Mac with Apple silicon")
        );
        assert_eq!(get(&s, "runtime:container").kind, SettingsRowKind::Choice);
        let old_mac = RuntimesInput {
            host_arch: Some("arm64".into()),
            local_backends: Some(vec!["lume".into(), "managed".into()]),
            ..intel.clone()
        };
        assert_eq!(
            get(&section(Some(&old_mac)), "runtime:microvm")
                .help
                .as_deref(),
            Some("MicroVMs need macOS 26")
        );
        let linux = RuntimesInput {
            os: Some(SpaceOs::Linux),
            host_arch: Some("amd64".into()),
            local_backends: Some(vec![]),
            ..Default::default()
        };
        let s = section(Some(&linux));
        assert_eq!(
            get(&s, "runtime:container").help.as_deref(),
            Some("Containers need Docker or Podman")
        );
        assert_eq!(
            get(&s, "runtime:vm").help.as_deref(),
            Some("Virtual machines need QEMU")
        );
    }

    /// Cua's own is offered only where Cua has one.
    #[test]
    fn cuas_own_is_offered_where_cua_has_one() {
        let linux = RuntimesInput {
            os: Some(SpaceOs::Linux),
            local_backends: Some(vec![
                "container".into(),
                "qemu".into(),
                "firecracker".into(),
            ]),
            ..Default::default()
        };
        let s = section(Some(&linux));
        let opts = |id: &str| {
            get(&s, id)
                .options
                .iter()
                .map(|o| o.id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(opts("runtime:container"), ["auto", "system"]);
        assert_eq!(opts("runtime:vm"), ["auto", "system"]);
        assert_eq!(opts("runtime:microvm"), ["auto", "builtin", "system"]);
    }

    /// What each choice writes, and the precedence between the first
    /// detected runtime (no path) and any other path.
    #[test]
    fn choices_write_the_per_kind_settings() {
        let input = mac(vec![RuntimeKindInput {
            kind: "container".into(),
            source: Some("system".into()),
            detected: Some(vec![docker(), podman()]),
            ..Default::default()
        }]);
        let c = |row: &str, o: &str| choose(Some(&input), row, o);
        let ch = |key: &str, value: Option<&str>| {
            Some(RuntimeSettingChange {
                key: key.into(),
                value: value.map(str::to_string),
            })
        };
        assert_eq!(
            c("runtime:microvm", "system"),
            ch("runtime.microvm", Some("system"))
        );
        assert_eq!(
            c("runtime:vm", "builtin"),
            ch("runtime.vm", Some("builtin"))
        );
        assert_eq!(
            c("runtime-detected:container", "/usr/local/bin/docker"),
            ch("runtime.container_path", None)
        );
        assert_eq!(
            c("runtime-detected:container", "/opt/homebrew/bin/podman"),
            ch("runtime.container_path", Some("/opt/homebrew/bin/podman"))
        );
        assert_eq!(
            c("runtime-path:container", " /Users/me/bin/docker "),
            ch("runtime.container_path", Some("/Users/me/bin/docker"))
        );
        assert_eq!(
            c("runtime-path:container", ""),
            ch("runtime.container_path", None)
        );
        assert_eq!(c("notch", "show"), None);
        assert_eq!(c("runtime:nope", "auto"), None);
    }
}
