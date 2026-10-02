//! The approval gate of the agent surface.
//!
//! [`classify`] says which [`Cap`] a tool call exercises (or that it is not
//! for agents at all); [`Guard`] asks the user ([`Approver`], Touch ID)
//! when the [`Policy`] in the cua home says that capability needs it. The
//! policy is read on every call, so a change in Settings applies at once.

use crate::approvals::{Approver, Cap, Policy};
use serde_json::Value;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

/// What a call needs before it runs.
#[derive(Debug, PartialEq, Eq)]
pub enum Need {
    /// The user's approval for `Cap`, shown as `String`.
    Cap(Cap, String),
    /// The user's approval for `Cap` when this Space is one of the user's
    /// own machines (`relay:` / `direct:`).
    OnMachine {
        /// The Space argument.
        space: String,
        /// What the call does, for the prompt.
        what: String,
    },
    /// Never available to an agent.
    Forbidden(String),
}

fn str_of<'a>(a: &'a Value, k: &str) -> &'a str {
    a.get(k).and_then(Value::as_str).unwrap_or("")
}

fn non_empty(a: &Value, k: &str) -> bool {
    match a.get(k) {
        Some(Value::Array(v)) => !v.is_empty(),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Null) | None => false,
        Some(_) => true,
    }
}

/// Where a `create_space` / `sandbox_create` call puts the new Space.
fn placement(a: &Value) -> Option<Cap> {
    if non_empty(a, "pool") {
        return Some(Cap::Cloud);
    }
    let on = str_of(a, "on").trim();
    match on {
        "" | "local" => None,
        "cloud" | "aws" | "gcp" | "modal" => Some(Cap::Cloud),
        o if o.starts_with("cloud:") => Some(Cap::Cloud),
        // `host:<machine>`, a machine's name, or `direct:`: the user's own.
        _ => Some(Cap::RemoteExec),
    }
}

/// The Space argument of a tool.
fn space_of(tool: &str, a: &Value) -> String {
    let key = if tool.starts_with("computer_") && !tool.starts_with("computer_access_") {
        "sandbox"
    } else if tool.starts_with("sandbox_") || tool == "teleport_browser_session" {
        "name"
    } else {
        "space"
    };
    str_of(a, key).to_string()
}

/// What `tool(args)` needs. Empty: it is free.
pub fn classify(tool: &str, a: &Value) -> Vec<Need> {
    let mut out = vec![];
    let on_machine = |out: &mut Vec<Need>, what: &str| {
        out.push(Need::OnMachine {
            space: space_of(tool, a),
            what: what.to_string(),
        })
    };
    match tool {
        t if crate::mcp::surface::USER_ONLY.contains(&t) => out.push(Need::Forbidden(format!(
            "{t} is for you, not agents: approve and grant access in the Cua app."
        ))),
        "cloud_connect" => out.push(Need::Cap(Cap::Cloud, "connect a cloud account".into())),
        "cloud_sweep" => out.push(Need::Cap(
            Cap::Cloud,
            "delete Cua resources in your cloud".into(),
        )),
        "create_space" | "sandbox_create" => {
            if let Some(cap) = placement(a) {
                let what = if cap == Cap::Cloud {
                    "create a cloud Space (it costs money)"
                } else {
                    "create a Space on one of your machines"
                };
                out.push(Need::Cap(cap, what.into()));
            }
        }
        "add_space" => out.push(Need::Cap(Cap::Machines, "add a machine as a Space".into())),
        "relay_register_space" => out.push(Need::Cap(
            Cap::Machines,
            "attach a Space to your account".into(),
        )),
        "upload" | "send_file" => {
            if sensitive_path(str_of(a, "path"), &homes()) {
                out.push(Need::Cap(
                    Cap::HostFiles,
                    format!("send {} from this Mac into a Space", str_of(a, "path")),
                ));
            }
            on_machine(&mut out, "write files on one of your machines");
        }
        "download" => {
            let dest = str_of(a, "dest");
            if !dest.is_empty() && protected_dest(dest, &homes()) {
                out.push(Need::Forbidden(format!(
                    "download into {dest} is not allowed: it holds settings, keys or startup items."
                )));
            }
            on_machine(&mut out, "read files on one of your machines");
        }
        "space_bash" => on_machine(&mut out, "run a command on one of your machines"),
        "space_write" => on_machine(&mut out, "write a file on one of your machines"),
        "call_tool" => on_machine(&mut out, "use one of your machines"),
        "agent_start" => {
            if non_empty(a, "env_from_host") {
                out.push(Need::Cap(
                    Cap::ApiKeys,
                    "give an agent your API keys".into(),
                ));
            }
            on_machine(&mut out, "run an agent on one of your machines");
        }
        "persistent_agent_create" => {
            if non_empty(a, "env_from_host") {
                out.push(Need::Cap(
                    Cap::ApiKeys,
                    "give an agent your API keys".into(),
                ));
            }
            on_machine(&mut out, "run an agent on one of your machines");
        }
        "agent_message" => on_machine(&mut out, "steer an agent on one of your machines"),
        "sandbox_open_browser" => on_machine(&mut out, "open a browser on one of your machines"),
        t if t.starts_with("computer_") && !t.starts_with("computer_access_") => {
            on_machine(&mut out, "control one of your machines")
        }
        "routine_add" => out.push(Need::Cap(
            Cap::Routines,
            "schedule a routine that runs on its own".into(),
        )),
        "routine_set_enabled" if a.get("enabled").and_then(Value::as_bool) == Some(true) => {
            out.push(Need::Cap(Cap::Routines, "turn a routine on".into()))
        }
        "volume_storage_set" => out.push(Need::Cap(
            Cap::Storage,
            "change where your Volume is stored".into(),
        )),
        "volume_mount" | "volume_unmount" => out.push(Need::Cap(
            Cap::Storage,
            "mount or unmount your Volume".into(),
        )),
        "hotspot_start" => out.push(Need::Cap(
            Cap::Network,
            "share this Mac's network with a Space".into(),
        )),
        "open_space_viewer" | "show_space_pip" | "stream_space_window" => out.push(Need::Cap(
            Cap::Display,
            "show a Space on your screen".into(),
        )),
        _ => {}
    }
    out
}

/// `home` and the cua home, as the path checks need them.
pub struct Homes {
    /// The user's home directory.
    pub home: PathBuf,
    /// The cua home.
    pub cua: PathBuf,
}

fn homes() -> Homes {
    Homes {
        home: std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default(),
        cua: cua_home::cua_home(),
    }
}

/// `path` as an absolute, lexically normalised path, with symlinks resolved
/// as far as the path exists.
fn resolve(path: &str, h: &Homes) -> PathBuf {
    let p = if path == "~" {
        h.home.clone()
    } else if let Some(rest) = path.strip_prefix("~/") {
        h.home.join(rest)
    } else {
        PathBuf::from(path)
    };
    let p = if p.is_absolute() {
        p
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    };
    let mut norm = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                norm.pop();
            }
            Component::CurDir => {}
            other => norm.push(other),
        }
    }
    // Resolve symlinks of the longest existing prefix.
    let mut existing = norm.clone();
    let mut rest = vec![];
    while !existing.exists() {
        match existing.file_name() {
            Some(n) => rest.push(n.to_os_string()),
            None => break,
        }
        if !existing.pop() {
            break;
        }
    }
    let mut real = existing.canonicalize().unwrap_or(existing);
    for r in rest.into_iter().rev() {
        real.push(r);
    }
    real
}

const SECRET_DIRS: &[&str] = &[
    ".ssh",
    ".aws",
    ".gnupg",
    ".azure",
    ".kube",
    ".docker",
    ".netrc",
    ".git-credentials",
    ".npmrc",
    ".pypirc",
    ".cua",
    ".claude",
    ".codex",
    ".mozilla",
    ".config/gcloud",
    ".config/google-chrome",
    ".config/chromium",
    "Library/Keychains",
    "Library/Cookies",
    "Library/Safari",
    "Library/Application Support/Google",
    "Library/Application Support/Firefox",
    "Library/Application Support/Chromium",
    "Library/Application Support/BraveSoftware",
    "Library/Application Support/Microsoft Edge",
    "Library/Application Support/Arc",
];

fn related(p: &Path, root: &Path) -> bool {
    p.starts_with(root) || root.starts_with(p)
}

/// Whether sending `path` into a Space could carry a secret: the path is a
/// credentials folder or file, is inside one, or is a folder that holds one
/// (the home directory itself, say).
pub fn sensitive_path(path: &str, h: &Homes) -> bool {
    if path.is_empty() {
        return false;
    }
    let p = resolve(path, h);
    if related(&p, &resolve(&h.cua.to_string_lossy(), h)) {
        return true;
    }
    SECRET_DIRS
        .iter()
        .any(|d| related(&p, &resolve(&h.home.join(d).to_string_lossy(), h)))
}

/// Whether a download into `dest` could replace settings, keys or startup
/// items.
pub fn protected_dest(dest: &str, h: &Homes) -> bool {
    let p = resolve(dest, h);
    if p == resolve(&h.home.to_string_lossy(), h) {
        return true;
    }
    let below = |root: PathBuf| p.starts_with(resolve(&root.to_string_lossy(), h));
    if below(h.cua.clone()) {
        return true;
    }
    const USER: &[&str] = &[
        ".cua",
        ".ssh",
        ".aws",
        ".gnupg",
        ".config",
        ".kube",
        ".docker",
        ".claude",
        ".codex",
        "Library/LaunchAgents",
        "Library/Keychains",
        "Library/Application Support",
        "Library/Preferences",
    ];
    if USER.iter().any(|d| below(h.home.join(d))) {
        return true;
    }
    [
        "/etc",
        "/usr",
        "/bin",
        "/sbin",
        "/System",
        "/Library",
        "/private/etc",
    ]
    .iter()
    .any(|d| p.starts_with(d))
}

/// Asks the user when the policy in `home` says `cap` needs it.
#[derive(Clone)]
pub struct Guard {
    home: PathBuf,
    approver: Arc<dyn Approver>,
}

impl std::fmt::Debug for Guard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Guard").field("home", &self.home).finish()
    }
}

impl Guard {
    /// A guard reading `<home>/approvals.json` and asking `approver`.
    pub fn new(home: PathBuf, approver: Arc<dyn Approver>) -> Self {
        Guard { home, approver }
    }

    /// The cua home the policy lives in.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// The current policy.
    pub fn policy(&self) -> Policy {
        Policy::load(&self.home)
    }

    /// Confirms `cap` for `what`, or says why not.
    pub async fn require(&self, who: &str, cap: Cap, what: &str) -> Result<(), String> {
        if !self.policy().requires(cap) {
            return Ok(());
        }
        let approver = self.approver.clone();
        let reason = format!("{who} wants to {what}");
        tokio::task::spawn_blocking(move || approver.confirm(&reason))
            .await
            .map_err(|e| format!("approval task: {e}"))?
            .map_err(|e| format!("not approved: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn homes() -> (tempfile::TempDir, Homes) {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().canonicalize().unwrap();
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::create_dir_all(home.join("proj")).unwrap();
        let h = Homes {
            cua: home.join(".cua"),
            home,
        };
        (d, h)
    }

    #[test]
    fn secret_paths_are_sensitive_and_projects_are_not() {
        let (_d, h) = homes();
        let p = |s: &str| h.home.join(s).to_string_lossy().to_string();
        assert!(sensitive_path(&p(".ssh/id_ed25519"), &h));
        assert!(sensitive_path(&p(".ssh"), &h));
        assert!(sensitive_path(&p(".cua/spaces-credentials.json"), &h));
        assert!(
            sensitive_path(&h.home.to_string_lossy(), &h),
            "the home holds them"
        );
        assert!(sensitive_path(&p("proj/../.ssh/id"), &h), "dot-dot");
        assert!(!sensitive_path(&p("proj/main.py"), &h));
        assert!(!sensitive_path(&p("proj"), &h));
        assert!(!sensitive_path("", &h));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_into_secrets_is_sensitive() {
        let (_d, h) = homes();
        let link = h.home.join("proj/innocent");
        std::os::unix::fs::symlink(h.home.join(".ssh"), &link).unwrap();
        assert!(sensitive_path(&link.to_string_lossy(), &h));
    }

    #[test]
    fn downloads_cannot_land_in_settings() {
        let (_d, h) = homes();
        let p = |s: &str| h.home.join(s).to_string_lossy().to_string();
        assert!(protected_dest(&p(".cua"), &h));
        assert!(protected_dest(&p(".cua/volume"), &h));
        assert!(protected_dest(&p(".ssh"), &h));
        assert!(protected_dest(&p("Library/LaunchAgents"), &h));
        assert!(protected_dest(&h.home.to_string_lossy(), &h));
        assert!(protected_dest("/etc", &h));
        assert!(!protected_dest(&p("proj/out"), &h));
        assert!(!protected_dest(&p("Downloads"), &h));
    }

    fn caps(tool: &str, a: Value) -> Vec<Cap> {
        classify(tool, &a)
            .into_iter()
            .filter_map(|n| match n {
                Need::Cap(c, _) => Some(c),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn placement_decides_cloud_and_machines() {
        assert!(caps("create_space", json!({})).is_empty());
        assert!(caps("create_space", json!({"on": "local"})).is_empty());
        assert_eq!(caps("create_space", json!({"on": "cloud"})), [Cap::Cloud]);
        assert_eq!(caps("create_space", json!({"on": "aws"})), [Cap::Cloud]);
        assert_eq!(caps("sandbox_create", json!({"pool": "p"})), [Cap::Cloud]);
        assert_eq!(
            caps("create_space", json!({"on": "host:mac-mini"})),
            [Cap::RemoteExec]
        );
        assert_eq!(caps("cloud_sweep", json!({})), [Cap::Cloud]);
    }

    #[test]
    fn capabilities_map_to_their_tools() {
        assert_eq!(caps("add_space", json!({"url": "x"})), [Cap::Machines]);
        assert_eq!(caps("relay_register_space", json!({})), [Cap::Machines]);
        assert_eq!(caps("routine_add", json!({})), [Cap::Routines]);
        assert!(caps("routine_set_enabled", json!({"enabled": false})).is_empty());
        assert_eq!(
            caps("routine_set_enabled", json!({"enabled": true})),
            [Cap::Routines]
        );
        assert_eq!(caps("volume_storage_set", json!({})), [Cap::Storage]);
        assert_eq!(caps("hotspot_start", json!({})), [Cap::Network]);
        assert_eq!(caps("show_space_pip", json!({})), [Cap::Display]);
        assert_eq!(
            caps(
                "agent_start",
                json!({"env_from_host": ["ANTHROPIC_API_KEY"]})
            ),
            [Cap::ApiKeys]
        );
        assert!(caps("agent_start", json!({"env_from_host": []})).is_empty());
        assert!(caps("list_spaces", json!({})).is_empty());
        assert!(caps("space_bash", json!({"space": "local:x"})).is_empty());
    }

    #[test]
    fn machine_scoped_tools_name_their_space() {
        let n = classify("computer_click", &json!({"sandbox": "relay:mini"}));
        assert_eq!(
            n,
            [Need::OnMachine {
                space: "relay:mini".into(),
                what: "control one of your machines".into()
            }]
        );
        let n = classify("space_bash", &json!({"space": "mini"}));
        assert!(matches!(&n[0], Need::OnMachine { space, .. } if space == "mini"));
        assert!(classify("computer_access_grant", &json!({})).is_empty());
    }

    #[test]
    fn approving_your_own_request_is_not_for_agents() {
        for t in [
            "volume_approve",
            "volume_deny",
            "volume_grant",
            "volume_revoke",
            "volume_grants",
            "volume_requests",
        ] {
            assert!(
                matches!(classify(t, &json!({}))[0], Need::Forbidden(_)),
                "{t}"
            );
        }
    }

    struct Deny;
    impl Approver for Deny {
        fn confirm(&self, _: &str) -> Result<(), String> {
            Err("declined".into())
        }
    }

    #[tokio::test]
    async fn the_policy_decides_whether_the_user_is_asked() {
        let d = tempfile::tempdir().unwrap();
        let g = Guard::new(d.path().to_path_buf(), Arc::new(Deny));
        assert!(
            g.require("agent", Cap::Cloud, "x").await.is_err(),
            "default: asks"
        );
        std::fs::write(
            crate::approvals::path_in(d.path()),
            r#"{"require":{"cloud":false}}"#,
        )
        .unwrap();
        assert!(
            g.require("agent", Cap::Cloud, "x").await.is_ok(),
            "turned off: free"
        );
        assert!(g.require("agent", Cap::Machines, "x").await.is_err());
    }
}
