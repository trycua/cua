//! Spaces this host provides to the owner's other devices (`cua host setup
//! --provide-spaces`), and how a client picks the host by name.
//!
//! The host's cua daemon creates each Space with its own runtimes (Lume for
//! macOS VMs, Docker for Linux containers) when an enrolled device of the
//! owner asks through the relay, attaches it to the relay as its own
//! machine, and keeps:
//!
//! - `<cua home>/host/provided-spaces.json` (0600): the Spaces it provides,
//!   with each one's relay machine token (so it can remove the machine when
//!   the Space is deleted);
//! - `<cua home>/host/spaces-audit.jsonl` (0600): every remote create,
//!   delete and refusal, and every settings change, one hash-chained JSON
//!   line each, so an edited or truncated log is reported as such.
//!
//! [`match_machine`] resolves the name a person or an agent gave ("spare
//! mac mini") to one of the account's relay machines, or returns the
//! choices when more than one fits.

use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::relay::Machine;
use crate::{Error, Result};

/// Apple's macOS license allows at most two additional macOS instances (VMs)
/// per Mac. Apple Virtualization, which Lume runs on, refuses a third one.
pub const MACOS_VM_LICENSE_LIMIT: u32 = 2;

/// Provided Spaces at once unless the owner sets another limit.
pub const DEFAULT_MAX_SPACES: u32 = 4;

/// Why macOS VMs are limited, for people.
pub const MACOS_LIMIT_REASON: &str = "Apple's macOS license allows two macOS VMs per Mac (Apple Virtualization, which Lume runs on, enforces it)";

/// The host's two independent settings and its limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSpacesSettings {
    /// This machine's own desktop is a Space (screen, input, shell, files).
    pub share_desktop: bool,
    /// Accept Space create, list and delete requests from the owner's
    /// enrolled devices and the accounts the machine is shared with.
    pub provide_spaces: bool,
    /// At most this many provided Spaces at once (0: no limit).
    pub max_spaces: u32,
    /// At most this many macOS VMs on this Mac (never above
    /// [`MACOS_VM_LICENSE_LIMIT`]).
    pub max_macos_vms: u32,
}

impl Default for HostSpacesSettings {
    fn default() -> Self {
        Self {
            share_desktop: true,
            provide_spaces: false,
            max_spaces: DEFAULT_MAX_SPACES,
            max_macos_vms: MACOS_VM_LICENSE_LIMIT,
        }
    }
}

/// What a machine set up for access is for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HostProfile {
    /// Your own desktop, reached from your other devices (the default):
    /// share the desktop, do not provide Spaces.
    #[default]
    Desktop,
    /// A spare machine that runs Spaces for you: do not share its desktop,
    /// provide Spaces.
    Spare,
}

impl HostProfile {
    /// Parses `desktop` / `spare`.
    pub fn parse(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "desktop" | "" => Ok(Self::Desktop),
            "spare" | "spaces" => Ok(Self::Spare),
            other => Err(Error::InvalidArgument(format!(
                "profile {other:?}: use desktop (share this desktop) or spare (only provide Spaces)"
            ))),
        }
    }

    /// `(share_desktop, provide_spaces)` of the profile.
    pub fn settings(self) -> (bool, bool) {
        match self {
            Self::Desktop => (true, false),
            Self::Spare => (false, true),
        }
    }
}

/// A Space this host provides.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvidedSpace {
    /// The relay machine it is reached as (`relay:<machine>`).
    pub relay_machine: String,
    /// The Space on this host (`local:<name>`).
    pub local_space: String,
    /// Display name.
    pub name: String,
    /// The image, as requested.
    pub image: String,
    /// `linux`, `macos`, `windows` (empty when unknown).
    #[serde(default)]
    pub os: String,
    /// `container` or `vm` (empty when unknown).
    #[serde(default)]
    pub kind: String,
    /// `lume`, `runc`, ... (empty when unknown).
    #[serde(default)]
    pub runtime: String,
    /// Who created it, for people.
    pub created_by: String,
    /// The account id that created it (the relay-asserted `acct`, or
    /// `local`).
    pub created_by_account: String,
    /// Unix milliseconds.
    pub created_at_ms: u64,
    /// The relay machine token (removes the machine on delete). Never
    /// serialized to status or clients.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub machine_token: String,
    /// The relay the machine is on.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub relay_url: String,
    /// A Space provided without the relay (a host in direct mode): the
    /// port on the host's direct address forwarded to its cua-spacesd. 0
    /// for a Space on the relay (`relay_machine` is then set).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub direct_port: u16,
}

fn is_zero(n: &u16) -> bool {
    *n == 0
}

impl ProvidedSpace {
    /// Whether this is a macOS VM (counts against the license limit).
    pub fn is_macos(&self) -> bool {
        self.os == "macos" || is_macos_image(&self.image)
    }

    /// Whether it is provided without the relay (reached at the host's
    /// direct address on [`ProvidedSpace::direct_port`]).
    pub fn is_direct(&self) -> bool {
        self.direct_port != 0
    }

    /// The id people and the audit know it by: `relay:<machine>`, or its
    /// `local:<name>` on the host for a direct Space.
    pub fn label(&self) -> String {
        if self.relay_machine.is_empty() {
            self.local_space.clone()
        } else {
            format!("relay:{}", self.relay_machine)
        }
    }

    /// The record without its secrets (for status and clients).
    pub fn public(&self) -> Self {
        Self {
            machine_token: String::new(),
            ..self.clone()
        }
    }
}

/// Whether `image` names a macOS guest: the `macos` alias (`macos`,
/// `macos:26`) or a reference whose repository is a macOS image.
pub fn is_macos_image(image: &str) -> bool {
    let i = image.trim().to_ascii_lowercase();
    let i = i.strip_prefix("lume:").unwrap_or(&i);
    if i == "macos" || i.starts_with("macos:") || i.starts_with("macos-") {
        return true;
    }
    // `ghcr.io/trycua/macos:26`, `ghcr.io/trycua/macos-sequoia-cua:latest`.
    let repo = i.split(['@', ':']).next().unwrap_or("");
    let last = i
        .rsplit('/')
        .next()
        .unwrap_or("")
        .split(['@', ':'])
        .next()
        .unwrap_or("");
    i.contains('/') && (last == "macos" || last.starts_with("macos-") || repo.ends_with("/macos"))
}

/// `<host dir>/provided-spaces.json`.
pub fn provided_path(host_dir: &Path) -> PathBuf {
    host_dir.join("provided-spaces.json")
}

/// `<host dir>/spaces-audit.jsonl`.
pub fn audit_path(host_dir: &Path) -> PathBuf {
    host_dir.join("spaces-audit.jsonl")
}

/// The Spaces this host provides (with their secrets), oldest first.
pub fn load_provided(host_dir: &Path) -> Result<Vec<ProvidedSpace>> {
    match std::fs::read(provided_path(host_dir)) {
        Ok(b) => serde_json::from_slice(&b)
            .map_err(|e| Error::Internal(format!("{}: {e}", provided_path(host_dir).display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(e.into()),
    }
}

/// Replaces the provided Spaces (0600).
pub fn save_provided(host_dir: &Path, all: &[ProvidedSpace]) -> Result<()> {
    let text =
        serde_json::to_string_pretty(all).map_err(|e| Error::Internal(e.to_string()))? + "\n";
    crate::host::write_secret(&provided_path(host_dir), &text)
}

// ------------------------------------------------------------------ audit

/// The hashed part of an audit line (field order is the canonical form).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct AuditBody {
    seq: u64,
    ts_ms: u64,
    action: String,
    who: String,
    space: String,
    detail: String,
    prev: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AuditLine {
    #[serde(flatten)]
    body: AuditBody,
    hash: String,
}

const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";
/// Lines read at most.
const MAX_AUDIT_LINES: usize = 200_000;

fn audit_hash(body: &AuditBody) -> String {
    let json = serde_json::to_string(body).expect("serializable");
    let mut h = Sha256::new();
    h.update(body.prev.as_bytes());
    h.update(b"\n");
    h.update(json.as_bytes());
    hex::encode(h.finalize())
}

/// One line of the host's Spaces audit, for display.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpacesAuditRecord {
    /// Unix milliseconds.
    pub at_ms: u64,
    /// `create`, `delete`, `refused` or `config`.
    pub action: String,
    /// Who: an account, or `local`.
    pub who: String,
    /// The relay machine or Space.
    pub space: String,
    /// Detail.
    pub detail: String,
}

/// The audit's newest entries and whether its chain verified.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpacesAuditReport {
    /// Newest first.
    pub recent: Vec<SpacesAuditRecord>,
    /// Set when the chain does not verify.
    pub error: Option<String>,
}

fn read_chain(path: &Path) -> std::result::Result<(Vec<AuditLine>, Option<String>), Error> {
    let f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((vec![], None)),
        Err(e) => return Err(e.into()),
    };
    let mut out = Vec::new();
    let mut prev = GENESIS.to_string();
    for (i, line) in BufReader::new(f).lines().enumerate() {
        if i >= MAX_AUDIT_LINES {
            return Ok((out, Some("audit log too long to verify".into())));
        }
        let Ok(line) = line else {
            return Ok((out, Some(format!("line {}: unreadable", i + 1))));
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(e) = serde_json::from_str::<AuditLine>(&line) else {
            return Ok((out, Some(format!("line {}: malformed", i + 1))));
        };
        if e.body.prev != prev {
            return Ok((out, Some(format!("line {}: chain broken", i + 1))));
        }
        if audit_hash(&e.body) != e.hash {
            return Ok((out, Some(format!("line {}: altered", i + 1))));
        }
        prev = e.hash.clone();
        out.push(e);
    }
    Ok((out, None))
}

/// Appends one event to the host's Spaces audit (hash-chained after the
/// last verified line; a log that no longer verifies is refused rather
/// than extended, so tampering stays visible).
pub fn audit(host_dir: &Path, action: &str, who: &str, space: &str, detail: &str) -> Result<()> {
    std::fs::create_dir_all(host_dir)?;
    let path = audit_path(host_dir);
    let (lines, error) = read_chain(&path)?;
    if let Some(e) = error {
        return Err(Error::Internal(format!(
            "{}: {e}; not appending to an altered audit log",
            path.display()
        )));
    }
    let (seq, prev) = lines
        .last()
        .map(|l| (l.body.seq + 1, l.hash.clone()))
        .unwrap_or((0, GENESIS.to_string()));
    let body = AuditBody {
        seq,
        ts_ms: crate::host::now_ms(),
        action: action.into(),
        who: who.into(),
        space: space.into(),
        detail: detail.chars().take(500).collect(),
        prev,
    };
    let hash = audit_hash(&body);
    let line = serde_json::to_string(&AuditLine { body, hash })
        .map_err(|e| Error::Internal(e.to_string()))?;
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut f = opts.open(&path)?;
    writeln!(f, "{line}")?;
    f.sync_all()?;
    Ok(())
}

/// The newest `limit` audit events, newest first, and whether the chain
/// verified.
pub fn read_audit(host_dir: &Path, limit: usize) -> SpacesAuditReport {
    match read_chain(&audit_path(host_dir)) {
        Ok((lines, error)) => SpacesAuditReport {
            recent: lines
                .into_iter()
                .rev()
                .take(limit)
                .map(|l| SpacesAuditRecord {
                    at_ms: l.body.ts_ms,
                    action: l.body.action,
                    who: l.body.who,
                    space: l.body.space,
                    detail: l.body.detail,
                })
                .collect(),
            error,
        },
        Err(e) => SpacesAuditReport {
            recent: vec![],
            error: Some(e.to_string()),
        },
    }
}

// --------------------------------------------------------------- matching

/// How a machine name resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MachineMatch {
    /// Exactly one machine fits best.
    One(Box<Machine>),
    /// Several fit equally well: ask which.
    Ambiguous(Vec<Machine>),
    /// None fits.
    None,
}

/// Words that say nothing about which machine is meant.
const FILLER: &[&str] = &[
    "my", "the", "a", "an", "on", "of", "our", "s", "machine", "computer", "host", "box",
];

fn tokens(s: &str) -> Vec<String> {
    s.to_ascii_lowercase()
        .replace('\'', "")
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

fn significant(s: &str) -> Vec<String> {
    tokens(s)
        .into_iter()
        .filter(|t| !FILLER.contains(&t.as_str()))
        .collect()
}

/// How many query words `m` (its name or id) contains: a word counts when
/// it equals a word of the name, starts one (at least three letters), or
/// appears in the name with its separators removed ("macmini").
fn score(query: &[String], m: &Machine) -> usize {
    let mut words = tokens(&m.name);
    words.extend(tokens(&m.id));
    let squashed: String = format!("{}{}", m.name, m.id)
        .to_ascii_lowercase()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    query
        .iter()
        .filter(|q| {
            words
                .iter()
                .any(|w| w == *q || (q.len() >= 3 && w.starts_with(q.as_str())))
                || (q.len() >= 3 && squashed.contains(q.as_str()))
        })
        .count()
}

/// Resolves `query` (a machine id, its exact name, or words from its name
/// such as "spare mac mini") among `machines`:
///
/// 1. an exact id or name (case-insensitive) wins;
/// 2. otherwise the machines containing the most query words, as long as
///    they contain at least half of them: one is [`MachineMatch::One`],
///    several are [`MachineMatch::Ambiguous`] (the caller asks which).
///
/// So "spare mac mini" picks "Mac mini (spare)" over "dillons-mac-mini",
/// and picks "dillons-mac-mini" when it is the only Mac mini.
pub fn match_machine(query: &str, machines: &[Machine]) -> MachineMatch {
    let q = query.trim();
    let q = q
        .strip_prefix("host:")
        .or_else(|| q.strip_prefix("relay:"))
        .unwrap_or(q)
        .trim();
    if q.is_empty() {
        return MachineMatch::None;
    }
    let exact: Vec<&Machine> = machines
        .iter()
        .filter(|m| m.id == q || m.name.eq_ignore_ascii_case(q))
        .collect();
    match exact.as_slice() {
        [one] => return MachineMatch::One(Box::new((*one).clone())),
        [] => {}
        many => return MachineMatch::Ambiguous(many.iter().map(|m| (*m).clone()).collect()),
    }
    let words = significant(q);
    if words.is_empty() {
        return MachineMatch::None;
    }
    let scored: Vec<(usize, &Machine)> = machines.iter().map(|m| (score(&words, m), m)).collect();
    let best = scored.iter().map(|(s, _)| *s).max().unwrap_or(0);
    if best == 0 || best * 2 < words.len() {
        return MachineMatch::None;
    }
    let top: Vec<Machine> = scored
        .into_iter()
        .filter(|(s, _)| *s == best)
        .map(|(_, m)| m.clone())
        .collect();
    match top.len() {
        1 => MachineMatch::One(Box::new(top.into_iter().next().expect("one"))),
        _ => MachineMatch::Ambiguous(top),
    }
}

/// A person-readable list of machines (`"Mac mini (spare)" (0123abcd...)`).
pub fn describe_choices(machines: &[Machine]) -> String {
    machines
        .iter()
        .map(|m| {
            format!(
                "{:?} (host:{}{})",
                if m.name.is_empty() { &m.id } else { &m.name },
                m.id,
                if m.online { "" } else { ", offline" }
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(id: &str, name: &str) -> Machine {
        Machine {
            id: id.into(),
            name: name.into(),
            online: true,
            ..Default::default()
        }
    }

    fn one(q: &str, ms: &[Machine]) -> String {
        match match_machine(q, ms) {
            MachineMatch::One(m) => m.id.clone(),
            other => panic!("{q}: {other:?}"),
        }
    }

    #[test]
    fn spare_mac_mini_resolves_by_words() {
        let spare = m("aaaa1111", "Mac mini (spare)");
        let dillons = m("bbbb2222", "dillons-mac-mini");
        let studio = m("cccc3333", "Studio");
        let all = [spare.clone(), dillons.clone(), studio.clone()];
        assert_eq!(one("spare mac mini", &all), "aaaa1111");
        assert_eq!(one("my spare Mac mini", &all), "aaaa1111");
        // The only Mac mini matches without "spare" in its name.
        assert_eq!(
            one("spare mac mini", &[dillons.clone(), studio.clone()]),
            "bbbb2222"
        );
        assert_eq!(
            one("macmini", &[dillons.clone(), studio.clone()]),
            "bbbb2222"
        );
        // Exact id and name win, with or without the `host:` prefix.
        assert_eq!(one("host:cccc3333", &all), "cccc3333");
        assert_eq!(one("studio", &all), "cccc3333");
        assert_eq!(one("dillons-mac-mini", &all), "bbbb2222");
    }

    #[test]
    fn ties_are_ambiguous_and_strangers_are_none() {
        let a = m("aaaa1111", "Mac mini (office)");
        let b = m("bbbb2222", "Mac mini (spare)");
        match match_machine("mac mini", &[a.clone(), b.clone()]) {
            MachineMatch::Ambiguous(c) => {
                assert_eq!(c.len(), 2);
                let text = describe_choices(&c);
                assert!(text.contains("host:aaaa1111") && text.contains("host:bbbb2222"));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            match_machine("windows laptop", std::slice::from_ref(&a)),
            MachineMatch::None
        );
        assert_eq!(
            match_machine("the machine", std::slice::from_ref(&a)),
            MachineMatch::None
        );
        assert_eq!(match_machine("", &[a]), MachineMatch::None);
    }

    #[test]
    fn macos_images_are_recognized() {
        for i in [
            "macos",
            "macos:26",
            "MACOS:15",
            "lume:macos",
            "ghcr.io/trycua/macos:26",
            "ghcr.io/trycua/macos-sequoia-cua:latest",
        ] {
            assert!(is_macos_image(i), "{i}");
        }
        for i in [
            "linux",
            "ghcr.io/trycua/linux:24.04",
            "ubuntu:24.04",
            "",
            "notmacos",
        ] {
            assert!(!is_macos_image(i), "{i}");
        }
    }

    #[test]
    fn profiles() {
        assert_eq!(
            HostProfile::parse("spare").unwrap().settings(),
            (false, true)
        );
        assert_eq!(
            HostProfile::parse("desktop").unwrap().settings(),
            (true, false)
        );
        assert!(HostProfile::parse("laptop").is_err());
    }

    #[test]
    fn audit_chains_and_detects_edits() {
        let dir = tempfile::tempdir().unwrap();
        audit(dir.path(), "create", "ada", "space-1", "linux").unwrap();
        audit(dir.path(), "delete", "ada", "space-1", "").unwrap();
        let r = read_audit(dir.path(), 10);
        assert_eq!(r.error, None);
        assert_eq!(
            r.recent
                .iter()
                .map(|e| e.action.as_str())
                .collect::<Vec<_>>(),
            ["delete", "create"]
        );
        let p = audit_path(dir.path());
        let text = std::fs::read_to_string(&p)
            .unwrap()
            .replace("\"ada\"", "\"eve\"");
        std::fs::write(&p, text).unwrap();
        assert!(
            read_audit(dir.path(), 10)
                .error
                .unwrap()
                .contains("altered")
        );
        assert!(audit(dir.path(), "create", "x", "y", "").is_err());
    }

    #[test]
    fn provided_spaces_round_trip_without_leaking_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let s = ProvidedSpace {
            relay_machine: "space-1".into(),
            machine_token: "cmt_secret".into(),
            image: "macos:26".into(),
            ..Default::default()
        };
        save_provided(dir.path(), std::slice::from_ref(&s)).unwrap();
        assert_eq!(load_provided(dir.path()).unwrap(), vec![s.clone()]);
        assert!(s.is_macos());
        assert!(
            !serde_json::to_string(&s.public())
                .unwrap()
                .contains("cmt_secret")
        );
    }
}
