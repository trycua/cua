//! The harness connection-state matrix (CUA-2268), against fake harness
//! homes: a temporary HOME, fake agent binaries (shell scripts that print
//! versions and answer `auth status`), a fake `cua` that answers the MCP
//! handshake, and an in-memory Keyvault. Nothing real is read or run.
//!
//! Every cell asserts the row's state and problem, and the invariant the
//! status exists for: never "connected" while something is broken, never
//! "not connected" while it works.
#![cfg(unix)]

use cua_agent_setup::{
    AgentSetup, ConnectionRow, ConnectionState, HostEnv, ImportState, ImportedSignIn, McpServer,
    McpState, Parts, ProblemAction, SignInCopy, SignInVault, SignedInState, StatusOptions,
    connection::ConnectParts,
};
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

#[derive(Default)]
struct FakeVault {
    items: Mutex<Vec<(ImportedSignIn, String)>>,
}

impl SignInVault for FakeVault {
    fn imported(&self) -> Result<Vec<ImportedSignIn>, String> {
        Ok(self
            .items
            .lock()
            .unwrap()
            .iter()
            .map(|(i, _)| i.clone())
            .collect())
    }
    fn save(&self, c: &SignInCopy) -> Result<ImportedSignIn, String> {
        let i = ImportedSignIn {
            source: c.source.clone(),
            env: c.env.clone(),
            label: c.label.clone(),
            expires_ms: c.expires_ms,
            added_ms: 1,
        };
        let mut g = self.items.lock().unwrap();
        g.retain(|(x, _)| x.source != c.source);
        g.push((i.clone(), c.value.clone()));
        Ok(i)
    }
    fn remove(&self, source: &str) -> Result<Vec<String>, String> {
        let mut g = self.items.lock().unwrap();
        let out = g
            .iter()
            .filter(|(x, _)| x.source == source)
            .map(|(x, _)| x.env.clone())
            .collect();
        g.retain(|(x, _)| x.source != source);
        Ok(out)
    }
}

impl FakeVault {
    fn value(&self, source: &str) -> Option<String> {
        self.items
            .lock()
            .unwrap()
            .iter()
            .find(|(x, _)| x.source == source)
            .map(|(_, v)| v.clone())
    }
    fn expire(&self, source: &str) {
        for (x, _) in self.items.lock().unwrap().iter_mut() {
            if x.source == source {
                x.expires_ms = Some(1);
            }
        }
    }
}

struct World {
    _dir: tempfile::TempDir,
    home: PathBuf,
    bin: PathBuf,
    env: HostEnv,
    vault: FakeVault,
}

const FAKE_CUA: &str = r#"#!/bin/sh
# A cua whose `mcp` answers initialize.
[ "$1" = "mcp" ] || exit 2
read -r line
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","serverInfo":{"name":"cua","version":"0"},"capabilities":{}}}'
"#;

const DEAD_CUA: &str = "#!/bin/sh\n# A cua that never answers.\nexit 0\n";

fn agent_script(version_line: &str, extra: &str) -> String {
    format!(
        "#!/bin/sh\ncase \"$1\" in\n  --version) echo \"{version_line}\" ;;\n{extra}  *) exit 0 ;;\nesac\n"
    )
}

const CLAUDE_EXTRA: &str = r#"  auth)
    if [ -f "$HOME/.fake-claude-signed-in" ]; then echo '{"authMethod":"claude.ai"}'; exit 0; fi
    echo '{"authMethod":"none"}'; exit 1 ;;
  setup-token)
    if [ -f "$HOME/.fake-claude-cancel" ]; then echo "Login cancelled"; exit 1; fi
    printf 'Your OAuth token:\r\n\r\n  sk-ant-oat01-FAKEtoken_0123456789abcdef\r\n' ; exit 0 ;;
"#;

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let mut env = HostEnv::isolated(&home);
        env.path = vec![bin.clone()];
        let w = World {
            _dir: dir,
            home,
            bin,
            env,
            vault: FakeVault::default(),
        };
        w.script("cua", FAKE_CUA);
        w.script("dead/cua", DEAD_CUA);
        w
    }

    fn script(&self, name: &str, body: &str) -> PathBuf {
        let p = self.bin.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn write(&self, rel: &str, text: &str) -> PathBuf {
        let p = self.home.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
        p
    }

    fn install(&self, id: &str, version: &str) {
        match id {
            "codex" => self.script("codex", &agent_script(&format!("codex-cli {version}"), "")),
            "claude-code" => self.script(
                "claude",
                &agent_script(&format!("{version} (Claude Code)"), CLAUDE_EXTRA),
            ),
            "hermes" => self.script(
                "hermes",
                &agent_script(&format!("Hermes Agent v{version}"), ""),
            ),
            "cursor" => self.script("cursor-agent", &agent_script(version, "")),
            "openclaw" => self.script(
                "openclaw",
                &agent_script(&format!("OpenClaw {version}"), ""),
            ),
            "t3code" => self.script("t3", &agent_script(version, "")),
            _ => unreachable!(),
        };
    }

    fn uninstall(&self, bin: &str) {
        std::fs::remove_file(self.bin.join(bin)).unwrap();
    }

    fn sign_in(&self, id: &str) {
        match id {
            "codex" => {
                self.write(
                    ".codex/auth.json",
                    r#"{"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{"id_token":"x","access_token":"y","refresh_token":"rt-1"},"last_refresh":"2026-10-24T00:00:00Z"}"#,
                );
            }
            "claude-code" => {
                self.write(".fake-claude-signed-in", "1");
            }
            "hermes" => {
                self.write(".hermes/.env", "OPENROUTER_API_KEY=sk-or-v1-fake\n");
                self.write(".hermes/config.yaml", "model:\n  provider: openrouter\n");
            }
            "cursor" => {
                self.write(".config/cursor/auth.json", r#"{"accessToken":"a"}"#);
            }
            _ => {}
        }
    }

    fn setup(&self) -> AgentSetup {
        AgentSetup::new(self.env.clone())
    }

    fn server(&self) -> McpServer {
        McpServer::new(self.bin.join("cua").display().to_string())
    }

    fn connect(&self, id: &str, sign_in: bool) {
        let mut env = self.env.clone();
        // Claude's setup-token is a CLI run; Claude's own config writer is
        // not (the fake has no `mcp add-json`).
        env.run_agent_clis = id == "claude-code" && sign_in;
        let s = AgentSetup::new(env);
        let parts = ConnectParts {
            skills: true,
            mcp: !(id == "claude-code" && sign_in),
            sign_in,
        };
        if !parts.mcp {
            self.setup()
                .connect(
                    &[id.into()],
                    &self.server(),
                    ConnectParts {
                        sign_in: false,
                        ..ConnectParts::ALL
                    },
                    None,
                    Duration::from_secs(5),
                )
                .unwrap();
        }
        let r = s
            .connect(
                &[id.into()],
                &self.server(),
                parts,
                Some(&self.vault),
                Duration::from_secs(10),
            )
            .unwrap();
        assert!(r[0].tools_ok, "{r:?}");
    }

    fn row(&self, id: &str) -> ConnectionRow {
        let opts = StatusOptions {
            expected: Some(self.server()),
            refresh: true,
            timeout: Duration::from_secs(5),
        };
        let r = self.setup().connection_status(&opts, Some(&self.vault));
        r.row(id)
            .unwrap_or_else(|| panic!("no row {id}: {r:?}"))
            .clone()
    }
}

/// The invariant behind every cell.
fn honest(r: &ConnectionRow) {
    let broken = !r.installed
        || matches!(r.mcp, McpState::Missing | McpState::Broken)
        || matches!(r.signed_in, SignedInState::No | SignedInState::Expired)
        || r.signin_import == ImportState::Expired;
    if r.state == ConnectionState::Connected {
        assert!(!broken, "connected while broken: {r:?}");
        assert!(
            r.mcp == McpState::Ok || r.mcp == McpState::Stale || r.mcp == McpState::Unsupported,
            "{r:?}"
        );
    }
    if r.state == ConnectionState::NotConnected {
        assert_eq!(
            r.mcp,
            McpState::Missing,
            "not connected while it works: {r:?}"
        );
        assert!(r.problem.is_none());
    }
    if r.state == ConnectionState::Attention {
        let p = r.problem.as_ref().expect("attention says what to do");
        assert!(!p.line.is_empty() && !p.line.contains('\n'), "{r:?}");
    }
}

fn check(
    w: &World,
    id: &str,
    want: ConnectionState,
    action: Option<ProblemAction>,
) -> ConnectionRow {
    let r = w.row(id);
    honest(&r);
    assert_eq!(r.state, want, "{id}: {r:#?}");
    assert_eq!(r.problem.as_ref().map(|p| p.action), action, "{id}: {r:#?}");
    r
}

const HARNESSES: [&str; 6] = [
    "hermes",
    "claude-code",
    "codex",
    "cursor",
    "openclaw",
    "t3code",
];

#[test]
fn rows_come_in_the_agreed_order_for_every_leading_harness() {
    let w = World::new();
    let opts = StatusOptions::default();
    let r = w.setup().connection_status(&opts, Some(&w.vault));
    let ids: Vec<&str> = r.agents.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(&ids[..6], &HARNESSES);
    for a in &r.agents {
        assert_eq!(a.state, ConnectionState::NotInstalled, "{a:?}");
        honest(a);
    }
    // JSON shape (CONTRACT §4).
    let v = serde_json::to_value(&r).unwrap();
    let row = &v["agents"][2];
    for k in [
        "id",
        "name",
        "installed",
        "version",
        "previous_version",
        "signed_in",
        "sign_in",
        "mcp",
        "mcp_config",
        "signin_imported",
        "signin_import",
        "state",
        "problem",
        "checked_at_ms",
    ] {
        assert!(row.get(k).is_some(), "missing {k}: {row}");
    }
    assert_eq!(row["name"], "Codex");
    assert_eq!(row["mcp_config"], "~/.codex/config.toml");
}

#[test]
fn installed_signed_in_or_not_connected_or_not() {
    for id in HARNESSES {
        let w = World::new();
        // Installed, nothing else.
        w.install(id, "1.2.3");
        let r = check(&w, id, ConnectionState::NotConnected, None);
        assert_eq!(r.version.as_deref(), Some("1.2.3"), "{id}");
        assert_eq!(r.mcp, McpState::Missing);
        // Signed in, still not connected.
        w.sign_in(id);
        check(&w, id, ConnectionState::NotConnected, None);
        // Connected (tools only).
        w.connect(id, false);
        let r = w.row(id);
        honest(&r);
        assert_eq!(r.mcp, McpState::Ok, "{id}: {r:?}");
        match id {
            "hermes" | "claude-code" | "codex" | "cursor" => {
                assert_eq!(r.signed_in, SignedInState::Yes, "{id}");
                assert_eq!(r.state, ConnectionState::Connected, "{id}: {r:?}");
            }
            _ => {
                // OpenClaw and T3 Code: Cua can't tell; it doesn't guess.
                assert_eq!(r.signed_in, SignedInState::Unknown, "{id}");
                assert_eq!(r.state, ConnectionState::Connected, "{id}: {r:?}");
            }
        }
        assert_eq!(r.previous_version.as_deref(), Some("1.2.3"));
        let expect_import = match id {
            "hermes" | "claude-code" | "codex" => ImportState::Missing,
            _ => ImportState::Unsupported,
        };
        assert_eq!(r.signin_import, expect_import, "{id}");
    }
}

#[test]
fn connected_but_not_signed_in_says_so_and_offers_sign_in_where_cua_can_start_it() {
    for (id, action) in [
        ("codex", ProblemAction::SignIn),
        ("claude-code", ProblemAction::None),
        ("hermes", ProblemAction::None),
    ] {
        let w = World::new();
        w.install(id, "1.0.0");
        w.connect(id, false);
        let r = check(&w, id, ConnectionState::Attention, Some(action));
        assert_eq!(r.signed_in, SignedInState::No);
        assert!(r.problem.unwrap().line.starts_with("Connected. No"), "{id}");
    }
    // Cursor without a sign-in file: unknown, not "no".
    let w = World::new();
    w.install("cursor", "1.0.0");
    w.connect("cursor", false);
    let r = check(&w, "cursor", ConnectionState::Connected, None);
    assert_eq!(r.signed_in, SignedInState::Unknown);
}

#[test]
fn sign_ins_are_copied_to_keyvault_and_their_expiry_is_seen() {
    // Codex: its auth.json, as is.
    let w = World::new();
    w.install("codex", "0.61.0");
    w.sign_in("codex");
    w.connect("codex", true);
    let r = check(&w, "codex", ConnectionState::Connected, None);
    assert!(r.signin_imported);
    assert_eq!(r.signin_import, ImportState::Ok);
    assert_eq!(r.sign_in.as_deref(), Some("ChatGPT sign-in"));
    let v: serde_json::Value = serde_json::from_str(&w.vault.value("codex").unwrap()).unwrap();
    assert_eq!(v["tokens"]["refresh_token"], "rt-1");
    let items = w.vault.imported().unwrap();
    assert_eq!(items[0].label, "Codex · ChatGPT sign-in");
    assert_eq!(items[0].env, "CODEX_AUTH_JSON");
    // The copy expired: Reconnect copies it again.
    w.vault.expire("codex");
    let r = check(
        &w,
        "codex",
        ConnectionState::Attention,
        Some(ProblemAction::Reconnect),
    );
    assert_eq!(r.signin_import, ImportState::Expired);
    w.connect("codex", true);
    check(&w, "codex", ConnectionState::Connected, None);

    // Claude Code: the token `claude setup-token` prints.
    let w = World::new();
    w.install("claude-code", "2.4.1");
    w.sign_in("claude-code");
    w.connect("claude-code", true);
    let r = check(&w, "claude-code", ConnectionState::Connected, None);
    assert_eq!(r.signin_import, ImportState::Ok);
    assert_eq!(
        w.vault.value("claude-code").as_deref(),
        Some("sk-ant-oat01-FAKEtoken_0123456789abcdef")
    );

    // Hermes: the provider key its config uses.
    let w = World::new();
    w.install("hermes", "0.22.5");
    w.sign_in("hermes");
    w.connect("hermes", true);
    let r = check(&w, "hermes", ConnectionState::Connected, None);
    assert_eq!(r.signin_import, ImportState::Ok);
    assert_eq!(r.sign_in.as_deref(), Some("OpenRouter key"));
    assert_eq!(w.vault.value("hermes").as_deref(), Some("sk-or-v1-fake"));
}

#[test]
fn a_refused_sign_in_copy_leaves_the_tools_connected_with_one_line() {
    let w = World::new();
    // Codex never signed in.
    w.install("codex", "0.61.0");
    let r = w
        .setup()
        .connect(
            &["codex".into()],
            &w.server(),
            ConnectParts::ALL,
            Some(&w.vault),
            Duration::from_secs(5),
        )
        .unwrap();
    assert!(r[0].tools_ok);
    let s = r[0].sign_in.as_ref().unwrap();
    assert_eq!(s.refused.as_deref(), Some("not_signed_in"));
    assert_eq!(
        s.line.as_deref(),
        Some("No ChatGPT sign-in on This Mac yet.")
    );
    // Claude's browser approval cancelled.
    w.install("claude-code", "2.4.1");
    w.sign_in("claude-code");
    w.write(".fake-claude-cancel", "1");
    let mut env = w.env.clone();
    env.run_agent_clis = true;
    let r = AgentSetup::new(env)
        .connect(
            &["claude-code".into()],
            &w.server(),
            ConnectParts {
                mcp: false,
                skills: false,
                sign_in: true,
            },
            Some(&w.vault),
            Duration::from_secs(10),
        )
        .unwrap();
    let s = r[0].sign_in.as_ref().unwrap();
    assert_eq!(s.refused.as_deref(), Some("cancelled"), "{s:?}");
    assert!(
        s.line
            .as_ref()
            .unwrap()
            .starts_with("Connected. Sign-in not copied")
    );
    // Hermes on an OAuth provider: not yet.
    w.install("hermes", "0.22.5");
    w.write(".hermes/config.yaml", "model:\n  provider: nous\n");
    w.write(
        ".hermes/auth.json",
        r#"{"version":1,"providers":{"nous":{"access_token":"x"}},"active_provider":"nous"}"#,
    );
    let r = w
        .setup()
        .connect(
            &["hermes".into()],
            &w.server(),
            ConnectParts::ALL,
            Some(&w.vault),
            Duration::from_secs(5),
        )
        .unwrap();
    assert_eq!(
        r[0].sign_in.as_ref().unwrap().refused.as_deref(),
        Some("not_yet")
    );
    assert!(w.vault.value("hermes").is_none());
    // A config Cua can't write: the row names the file.
    w.install("cursor", "1.0.0");
    let cfg = w.write(".cursor/mcp.json", "{ this is not json");
    let r = w
        .setup()
        .connect(
            &["cursor".into()],
            &w.server(),
            ConnectParts::ALL,
            Some(&w.vault),
            Duration::from_secs(5),
        )
        .unwrap();
    assert!(!r[0].tools_ok);
    assert_eq!(
        r[0].tools_line.as_deref(),
        Some("Couldn't add Cua to ~/.cursor/mcp.json")
    );
    assert_eq!(std::fs::read_to_string(cfg).unwrap(), "{ this is not json");
    let row = check(
        &w,
        "cursor",
        ConnectionState::Attention,
        Some(ProblemAction::Reconnect),
    );
    assert_eq!(row.mcp, McpState::Broken);
}

#[test]
fn expired_own_sign_in_is_never_connected() {
    let w = World::new();
    w.install("codex", "0.61.0");
    w.connect("codex", false);
    // An access token that expired and nothing to refresh it with.
    let exp_jwt = format!("h.{}.s", base64_url(r#"{"exp":1000}"#));
    w.write(
        ".codex/auth.json",
        &format!(r#"{{"tokens":{{"access_token":"{exp_jwt}","refresh_token":""}}}}"#),
    );
    let r = check(
        &w,
        "codex",
        ConnectionState::Attention,
        Some(ProblemAction::SignIn),
    );
    assert_eq!(r.signed_in, SignedInState::Expired);
    assert_eq!(
        r.problem.unwrap().line,
        "Codex's ChatGPT sign-in expired. Sign in again."
    );
}

fn base64_url(s: &str) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let b = s.as_bytes();
    let mut out = String::new();
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..(c.len() + 1) {
            out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

#[test]
fn config_edited_or_removed_outside_cua_is_caught() {
    for id in HARNESSES {
        let w = World::new();
        w.install(id, "1.0.0");
        w.sign_in(id);
        w.connect(id, false);
        check(&w, id, ConnectionState::Connected, None);
        let r = w.row(id);
        let file = w.home.join(r.mcp_config.unwrap().trim_start_matches("~/"));
        let text = std::fs::read_to_string(&file).unwrap();
        let cua = w.bin.join("cua").display().to_string();

        // Pointed at a cua that isn't there.
        std::fs::write(&file, text.replace(&cua, "/nowhere/cua")).unwrap();
        let r = check(
            &w,
            id,
            ConnectionState::Attention,
            Some(ProblemAction::Reconnect),
        );
        assert_eq!(r.mcp, McpState::Broken, "{id}");
        assert!(
            r.problem
                .unwrap()
                .line
                .contains("points at a cua that isn't there")
        );

        // Pointed at a cua that doesn't answer.
        std::fs::write(
            &file,
            text.replace(&cua, &w.bin.join("dead/cua").display().to_string()),
        )
        .unwrap();
        let r = check(
            &w,
            id,
            ConnectionState::Attention,
            Some(ProblemAction::Reconnect),
        );
        assert_eq!(r.mcp, McpState::Broken, "{id}");

        // Another copy of cua that works: connected, with a quiet line.
        let other = w.script("other/cua", FAKE_CUA);
        std::fs::write(&file, text.replace(&cua, &other.display().to_string())).unwrap();
        let r = check(
            &w,
            id,
            ConnectionState::Connected,
            Some(ProblemAction::Reconnect),
        );
        assert_eq!(r.mcp, McpState::Stale, "{id}");

        // Removed outside Cua.
        std::fs::write(&file, "").unwrap();
        let r = check(
            &w,
            id,
            ConnectionState::Attention,
            Some(ProblemAction::Reconnect),
        );
        assert_eq!(r.mcp, McpState::Missing, "{id}");
        assert!(
            r.problem.unwrap().line.starts_with("Cua was taken out of"),
            "{id}"
        );

        // Reconnect fixes it in one call.
        w.connect(id, false);
        check(&w, id, ConnectionState::Connected, None);
    }
}

#[test]
fn upgraded_harness_stays_connected_and_says_from_what() {
    let w = World::new();
    w.install("codex", "0.60.1");
    w.sign_in("codex");
    w.connect("codex", false);
    w.install("codex", "0.61.0");
    let r = check(&w, "codex", ConnectionState::Connected, None);
    assert_eq!(r.version.as_deref(), Some("0.61.0"));
    assert_eq!(r.previous_version.as_deref(), Some("0.60.1"));
    // An upgrade that rewrote the config without Cua: caught.
    std::fs::write(w.home.join(".codex/config.toml"), "model = \"gpt-5\"\n").unwrap();
    check(
        &w,
        "codex",
        ConnectionState::Attention,
        Some(ProblemAction::Reconnect),
    );
}

#[test]
fn uninstalled_after_connecting_offers_disconnect_which_undoes_both() {
    let w = World::new();
    w.install("codex", "0.61.0");
    w.sign_in("codex");
    w.write(
        ".codex/config.toml",
        "# mine\n[mcp_servers.cua]\ncommand = \"/opt/mine/wrapper\"\nargs = [\"serve\"]\n",
    );
    w.connect("codex", true);
    w.uninstall("codex");
    let r = check(
        &w,
        "codex",
        ConnectionState::Attention,
        Some(ProblemAction::Disconnect),
    );
    assert!(!r.installed);
    assert!(
        r.problem
            .unwrap()
            .line
            .contains("isn't on This Mac anymore")
    );
    // Disconnect: the entry that was there before comes back, the copy goes.
    let (_, removed) = w
        .setup()
        .disconnect(
            &["codex".into()],
            Parts::ALL,
            Default::default(),
            true,
            Some(&w.vault),
        )
        .unwrap();
    assert_eq!(removed, ["CODEX_AUTH_JSON"]);
    assert!(w.vault.imported().unwrap().is_empty());
    let toml = std::fs::read_to_string(w.home.join(".codex/config.toml")).unwrap();
    assert!(toml.contains("/opt/mine/wrapper"), "{toml}");
    // The old entry isn't Cua's own and its cua is gone: the row says so,
    // but it is no longer "Cua was taken out".
    let r = w.row("codex");
    honest(&r);
    assert_ne!(r.state, ConnectionState::Connected);
}

#[test]
fn a_second_agent_of_the_same_harness() {
    // Two Codex installs on PATH: the first one is the one that runs.
    let w = World::new();
    w.install("codex", "0.61.0");
    let second = w._dir.path().join("bin2");
    std::fs::create_dir_all(&second).unwrap();
    let p = second.join("codex");
    std::fs::write(&p, agent_script("codex-cli 0.55.0", "")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut env = w.env.clone();
    env.path.push(second.clone());
    let w2 = World { env, ..w };
    w2.sign_in("codex");
    w2.connect("codex", true);
    let r = check(&w2, "codex", ConnectionState::Connected, None);
    assert_eq!(r.version.as_deref(), Some("0.61.0"));
    // T3 Code drives that Codex and shares its config: connecting one shows
    // in the other, and disconnecting Codex is seen by T3 Code too.
    w2.install("t3code", "0.1.2");
    let t = w2.row("t3code");
    honest(&t);
    assert_eq!(t.mcp, McpState::Ok);
    w2.connect("t3code", false);
    w2.setup()
        .disconnect(
            &["codex".into()],
            Parts::ALL,
            Default::default(),
            false,
            Some(&w2.vault),
        )
        .unwrap();
    check(
        &w2,
        "t3code",
        ConnectionState::Attention,
        Some(ProblemAction::Reconnect),
    );
    check(&w2, "codex", ConnectionState::NotConnected, None);
    // A second Codex home (CODEX_HOME) is a separate agent: its own row.
    let mut env = w2.env.clone();
    env.vars.insert(
        "CODEX_HOME".into(),
        w2.home.join("work-codex").display().to_string(),
    );
    let s = AgentSetup::new(env);
    let r = s.connection_status(
        &StatusOptions {
            expected: Some(w2.server()),
            refresh: true,
            timeout: Duration::from_secs(5),
        },
        Some(&w2.vault),
    );
    let c = r.row("codex").unwrap();
    assert_eq!(c.mcp_config.as_deref(), Some("~/work-codex/config.toml"));
    assert_eq!(c.signed_in, SignedInState::No);
}

#[test]
fn status_is_cached_briefly_and_refresh_asks_again() {
    let w = World::new();
    w.install("codex", "0.61.0");
    let opts = StatusOptions {
        expected: Some(w.server()),
        refresh: true,
        timeout: Duration::from_secs(5),
    };
    let first = w.setup().connection_status(&opts, Some(&w.vault));
    w.uninstall("codex");
    let cached = w.setup().connection_status(
        &StatusOptions {
            refresh: false,
            ..opts.clone()
        },
        Some(&w.vault),
    );
    assert_eq!(cached, first);
    let fresh = w.setup().connection_status(&opts, Some(&w.vault));
    assert!(!fresh.row("codex").unwrap().installed);
}

#[allow(dead_code)]
fn _paths(_: &Path) {}
