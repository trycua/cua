//! Connect's per-part results (CUA-4067): setup's "Which agents do you use?"
//! draws one row per part per agent (Cua's tools, its skills, Cua Driver, the
//! sign-in saved to Keyvault) and retries one row by asking for that part
//! alone. Against a temporary HOME; nothing real is read or run.
#![cfg(unix)]

mod common;

use cua_agent_setup::{
    AgentSetup, HostEnv, ImportedSignIn, McpServer, SignInCopy, SignInVault,
    connection::{
        ConnectParts, ConnectResult, DRIVER_NEEDS_PERMISSION, DriverTarget, Part, PartResult,
        PartStatus,
    },
};
use std::{path::PathBuf, sync::Mutex, time::Duration};

#[derive(Default)]
struct Vault(Mutex<Vec<ImportedSignIn>>);

impl SignInVault for Vault {
    fn imported(&self) -> Result<Vec<ImportedSignIn>, String> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn save(&self, c: &SignInCopy) -> Result<ImportedSignIn, String> {
        let i = ImportedSignIn {
            source: c.source.clone(),
            env: c.env.clone(),
            label: c.label.clone(),
            expires_ms: c.expires_ms,
            added_ms: 1,
        };
        self.0.lock().unwrap().push(i.clone());
        Ok(i)
    }
    fn remove(&self, _: &str) -> Result<Vec<String>, String> {
        Ok(vec![])
    }
}

struct World {
    _dir: tempfile::TempDir,
    home: PathBuf,
    env: HostEnv,
    cua: PathBuf,
    /// The Cua Driver the Spaces app carries, as its bundle lays it out.
    bundled: PathBuf,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&home).unwrap();
        let cua = common::write_script(&bin, "cua", "exit 0");
        let bundled = dir
            .path()
            .join("Applications/Cua Spaces.app/Contents/Helpers/cua-driver");
        std::fs::create_dir_all(bundled.parent().unwrap()).unwrap();
        common::write_executable(&bundled, "#!/bin/sh\nexit 0\n");
        let mut env = HostEnv::isolated(&home);
        env.path = vec![bin];
        // Codex is on this computer (its folder is there).
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        World {
            _dir: dir,
            home,
            env,
            cua,
            bundled,
        }
    }

    fn setup(&self) -> AgentSetup {
        AgentSetup::new(self.env.clone())
    }

    fn server(&self) -> McpServer {
        McpServer::new(self.cua.display().to_string())
    }

    /// The bundled driver's entry, as Cua Spaces for macOS writes it.
    fn driver(&self, permissions: Option<bool>) -> DriverTarget {
        let socket = self.home.join(".cua/run/cua-driver.sock");
        DriverTarget {
            server: McpServer {
                args: vec![
                    "mcp".into(),
                    "--embedded".into(),
                    "--socket".into(),
                    socket.display().to_string(),
                    "--host-bundle-id".into(),
                    "com.trycua.spaces.macos".into(),
                ],
                ..McpServer::driver(self.bundled.display().to_string())
            },
            permissions_granted: permissions,
        }
    }

    fn connect(&self, parts: ConnectParts, driver: Option<&DriverTarget>) -> ConnectResult {
        self.connect_agent("codex", parts, driver, None)
    }

    fn connect_agent(
        &self,
        agent: &str,
        parts: ConnectParts,
        driver: Option<&DriverTarget>,
        vault: Option<&dyn SignInVault>,
    ) -> ConnectResult {
        let mut r = self
            .setup()
            .connect_full(
                &[agent.to_string()],
                &self.server(),
                driver,
                parts,
                vault,
                Duration::ZERO,
                None,
            )
            .unwrap();
        assert_eq!(r.len(), 1);
        r.remove(0)
    }

    fn codex_config(&self) -> String {
        std::fs::read_to_string(self.home.join(".codex/config.toml")).unwrap_or_default()
    }
}

const TOOLS_AND_DRIVER: ConnectParts = ConnectParts {
    skills: true,
    mcp: true,
    sign_in: false,
    driver: true,
};

fn part(r: &ConnectResult, p: Part) -> &PartResult {
    r.parts
        .iter()
        .find(|x| x.part == p)
        .unwrap_or_else(|| panic!("no {p:?} in {:?}", r.parts))
}

#[test]
fn every_part_asked_for_reports_what_it_did_and_where() {
    let w = World::new();
    let r = w.connect(TOOLS_AND_DRIVER, Some(&w.driver(Some(true))));
    let order: Vec<Part> = r.parts.iter().map(|p| p.part).collect();
    assert_eq!(order, [Part::Skills, Part::Mcp, Part::Driver]);
    assert!(
        r.parts.iter().all(|p| p.status == PartStatus::Done),
        "{:?}",
        r.parts
    );
    assert_eq!(
        part(&r, Part::Mcp).place.as_deref(),
        Some("~/.codex/config.toml")
    );
    assert_eq!(
        part(&r, Part::Driver).place.as_deref(),
        Some("~/.codex/config.toml")
    );
    assert_eq!(
        part(&r, Part::Skills).place.as_deref(),
        Some("~/.agents/skills")
    );
    assert!(r.tools_ok);
    // Serialized as the bridge sends it: `where`, snake_case words.
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["parts"][2]["part"], "driver");
    assert_eq!(v["parts"][2]["status"], "done");
    assert_eq!(v["parts"][2]["where"], "~/.codex/config.toml");
}

#[test]
fn the_driver_entry_runs_the_bundled_cua_driver_never_the_standalone_app() {
    let w = World::new();
    let r = w.connect(TOOLS_AND_DRIVER, Some(&w.driver(Some(true))));
    assert_eq!(part(&r, Part::Driver).status, PartStatus::Done);
    let cfg = w.codex_config();
    let parsed: toml_edit::DocumentMut = cfg.parse().unwrap();
    let entry = &parsed["mcp_servers"]["cua-driver"];
    assert_eq!(
        entry["command"].as_str(),
        Some(w.bundled.display().to_string().as_str())
    );
    assert!(
        entry["command"]
            .as_str()
            .unwrap()
            .ends_with("Contents/Helpers/cua-driver")
    );
    assert!(!cfg.contains("/Applications/CuaDriver.app"), "{cfg}");
    let args: Vec<&str> = entry["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(&args[..2], ["mcp", "--embedded"]);
    // Recorded like Cua's own entry, so it can be removed exactly.
    let state = std::fs::read_to_string(w.home.join(".cua/agent-setup.json")).unwrap();
    assert!(state.contains("cua-driver"), "{state}");
    // And Cua's skill for it is there.
    assert!(w.home.join(".agents/skills/cua-driver/SKILL.md").exists());
}

#[test]
fn permissions_off_writes_the_entry_and_says_what_to_allow() {
    let w = World::new();
    let r = w.connect(TOOLS_AND_DRIVER, Some(&w.driver(Some(false))));
    let d = part(&r, Part::Driver);
    assert_eq!(d.status, PartStatus::NeedsPermission);
    assert_eq!(d.line.as_deref(), Some(DRIVER_NEEDS_PERMISSION));
    // Written anyway: it works as soon as both are on, with nothing to redo.
    assert!(w.codex_config().contains("cua-driver"));
    // Unknown permissions (a host that can't tell) aren't a problem.
    let r = w.connect(TOOLS_AND_DRIVER, Some(&w.driver(None)));
    assert_eq!(part(&r, Part::Driver).status, PartStatus::Done);
}

#[test]
fn a_missing_bundled_driver_fails_its_row_only_and_writes_nothing_for_it() {
    let w = World::new();
    let mut d = w.driver(Some(true));
    d.server.command = w.home.join("gone/cua-driver").display().to_string();
    let r = w.connect(TOOLS_AND_DRIVER, Some(&d));
    let dr = part(&r, Part::Driver);
    assert_eq!(dr.status, PartStatus::Failed);
    assert!(
        dr.line
            .as_deref()
            .unwrap()
            .contains("Cua Driver isn't in this app"),
        "{dr:?}"
    );
    assert_eq!(part(&r, Part::Mcp).status, PartStatus::Done);
    assert_eq!(part(&r, Part::Skills).status, PartStatus::Done);
    assert!(!w.codex_config().contains("cua-driver"));

    // A relative command or a name that isn't Cua Driver's is refused too.
    let mut rel = w.driver(Some(true));
    rel.server.command = "cua-driver".into();
    let r = w.connect(TOOLS_AND_DRIVER, Some(&rel));
    assert_eq!(part(&r, Part::Driver).status, PartStatus::Failed);
    let mut named = w.driver(Some(true));
    named.server.name = "cua".into();
    let r = w.connect(TOOLS_AND_DRIVER, Some(&named));
    assert_eq!(part(&r, Part::Driver).status, PartStatus::Failed);
    assert!(!w.codex_config().contains("cua-driver"));
}

#[test]
fn a_config_cua_cant_read_fails_with_the_file_and_the_reason() {
    let w = World::new();
    std::fs::write(w.home.join(".codex/config.toml"), "this is = = not toml [").unwrap();
    let r = w.connect(TOOLS_AND_DRIVER, Some(&w.driver(Some(true))));
    let m = part(&r, Part::Mcp);
    assert_eq!(m.status, PartStatus::Failed);
    assert_eq!(m.place.as_deref(), Some("~/.codex/config.toml"));
    let line = m.line.as_deref().unwrap();
    assert!(
        line.starts_with("Couldn't write ~/.codex/config.toml: "),
        "{line}"
    );
    assert_eq!(part(&r, Part::Driver).status, PartStatus::Failed);
    // The skills don't depend on it.
    assert_eq!(part(&r, Part::Skills).status, PartStatus::Done);
    assert!(!r.tools_ok);
    // Left untouched.
    assert_eq!(w.codex_config(), "this is = = not toml [");
}

#[test]
fn one_row_is_retried_by_asking_for_that_part_alone() {
    let w = World::new();
    let only_driver = ConnectParts {
        skills: false,
        mcp: false,
        sign_in: false,
        driver: true,
    };
    let r = w.connect(only_driver, Some(&w.driver(Some(true))));
    let order: Vec<Part> = r.parts.iter().map(|p| p.part).collect();
    assert_eq!(order, [Part::Driver]);
    assert_eq!(part(&r, Part::Driver).status, PartStatus::Done);
    // Cua's own entry wasn't written.
    let cfg = w.codex_config();
    assert!(
        cfg.contains("cua-driver") && !cfg.contains("[mcp_servers.cua]"),
        "{cfg}"
    );
    // Asking for the driver part without a driver (a host that carries none)
    // reports nothing for it.
    let r = w.connect(only_driver, None);
    assert!(r.parts.is_empty(), "{:?}", r.parts);
}

#[test]
fn the_sign_in_row_says_where_it_went_or_why_not() {
    let w = World::new();
    let vault = Vault::default();
    let sign_in_only = ConnectParts::SIGN_IN_ONLY;
    // Claude Code isn't signed in here.
    let r = w.connect_agent("claude-code", sign_in_only, None, Some(&vault));
    let s = part(&r, Part::SignIn);
    assert_eq!(s.status, PartStatus::Failed);
    assert!(s.line.as_deref().is_some_and(|l| !l.is_empty()), "{s:?}");
    // Signed in: saved to Keyvault.
    std::fs::create_dir_all(w.home.join(".claude")).unwrap();
    let creds = serde_json::json!({"claudeAiOauth": {
        "accessToken": "sk-ant-oat01-FAKEaccess_0123456789abcdef_ABCDEFGHIJKLMNOPQRSTUVWXYZ_abcdefghijklmnopqrstuvwxyz_0123456789_QQQQ",
        "refreshToken": "sk-ant-ort01-FAKE", "expiresAt": 4_102_444_800_000u64,
        "scopes": ["user:inference"], "subscriptionType": "max"
    }});
    std::fs::write(w.home.join(".claude/.credentials.json"), creds.to_string()).unwrap();
    std::fs::write(
        w.home.join(".claude.json"),
        r#"{"oauthAccount":{"emailAddress":"ada@example.com"}}"#,
    )
    .unwrap();
    let r = w.connect_agent("claude-code", sign_in_only, None, Some(&vault));
    let s = part(&r, Part::SignIn);
    assert_eq!(s.status, PartStatus::Done, "{s:?}");
    assert_eq!(s.place.as_deref(), Some("Keyvault"));
    // No Keyvault in this build: a failed row that says so.
    let r = w.connect_agent("claude-code", sign_in_only, None, None);
    let s = part(&r, Part::SignIn);
    assert_eq!(s.status, PartStatus::Failed);
    assert!(s.line.as_deref().unwrap().contains("Keyvault"));
    // Codex signs in with a code made for Cua: the row says how.
    let r = w.connect_agent("codex", sign_in_only, None, Some(&vault));
    let s = part(&r, Part::SignIn);
    assert_eq!(s.status, PartStatus::Failed);
    assert!(
        s.line.as_deref().unwrap().contains("signs in with a code"),
        "{s:?}"
    );
    assert_eq!(
        r.sign_in.as_ref().unwrap().refused.as_deref(),
        Some("not_yet")
    );
    // An agent whose sign-in Cua doesn't save: skipped, nothing to show.
    let r = w.connect_agent("hermes", sign_in_only, None, Some(&vault));
    assert_eq!(part(&r, Part::SignIn).status, PartStatus::Skipped);
}
