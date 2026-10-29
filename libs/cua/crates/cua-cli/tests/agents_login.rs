//! `cua agents ...` and the `cua auth login` flows (browser PKCE with a
//! loopback redirect, device code, the automatic fallback) with the agent
//! onboarding, against a fake OIDC issuer in a temporary HOME. PATH is
//! reduced to system dirs and the fallback install dirs (`/opt/homebrew/bin`,
//! `/usr/local/bin`, ...) are dropped (`CUA_AGENT_BIN_DIRS`), so no real
//! agent CLI is ever found or run, and app bundles are hidden
//! (`CUA_AGENT_APP_DIRS`). The result does not depend on what is installed
//! on the machine running the tests.

mod common;
use common::*;
use serde_json::json;
use std::{
    io::{Read, Write},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::io::AsyncBufReadExt;

/// A home with no ambient agents.
fn home() -> Home {
    let mut h = Home::new();
    h.set("PATH", "/usr/bin:/bin")
        .set("CUA_AGENT_BIN_DIRS", "")
        .set("CUA_AGENT_APP_DIRS", "");
    h
}

fn mkdirs(h: &Home, rels: &[&str]) {
    for r in rels {
        std::fs::create_dir_all(h.dir.path().join(r)).unwrap();
    }
}

#[derive(Default)]
struct IssuerLog {
    challenge: Option<String>,
    redirect: Option<String>,
}

/// A fake issuer: `/auth` answers 200 (loopback redirect accepted) or 400
/// (`pkce_ok = false`, like Keycloak today), authorization_code and device
/// grants both issue tokens.
async fn issuer(pkce_ok: bool) -> (FakeHttp, Arc<Mutex<IssuerLog>>) {
    let log = Arc::new(Mutex::new(IssuerLog::default()));
    let l = log.clone();
    let fake = FakeHttp::start(move |r| {
        let host = format!("http://{}", r.headers.get("host").cloned().unwrap_or_default());
        let token = || {
            (
                200,
                json!({"access_token": jwt(json!({"preferred_username": "tester", "email": "tester@example.com", "sub": "u-1"})),
                    "refresh_token": "rt-1", "expires_in": 3600, "token_type": "Bearer"}),
            )
        };
        let path = r.path.split('?').next().unwrap_or_default();
        match (r.method.as_str(), path) {
            ("GET", "/.well-known/openid-configuration") => (
                200,
                json!({
                    "authorization_endpoint": format!("{host}/auth"),
                    "token_endpoint": format!("{host}/token"),
                    "device_authorization_endpoint": format!("{host}/device"),
                }),
            ),
            ("GET", "/auth") => {
                let u = url::Url::parse(&format!("http://x{}", r.path)).unwrap();
                let q: std::collections::HashMap<String, String> = u.query_pairs().into_owned().collect();
                assert_eq!(q["client_id"], "cua-cli");
                assert_eq!(q["response_type"], "code");
                assert_eq!(q["code_challenge_method"], "S256");
                assert!(q["redirect_uri"].starts_with("http://127.0.0.1:"));
                let mut g = l.lock().unwrap();
                g.challenge = Some(q["code_challenge"].clone());
                g.redirect = Some(q["redirect_uri"].clone());
                if pkce_ok {
                    (200, json!({"page": "login"}))
                } else {
                    (400, json!({"error": "Invalid parameter: redirect_uri"}))
                }
            }
            ("POST", "/device") => (
                200,
                json!({"device_code": "dc-1", "user_code": "WXYZ-1234",
                    "verification_uri": "https://login.test/device",
                    "expires_in": 60, "interval": 1}),
            ),
            ("POST", "/token") => match r.form("grant_type").as_deref() {
                Some("authorization_code") => {
                    let g = l.lock().unwrap();
                    let verifier = r.form("code_verifier").unwrap_or_default();
                    if r.form("code").as_deref() != Some("code-1")
                        || r.form("redirect_uri") != g.redirect
                        || Some(pkce(&verifier)) != g.challenge
                    {
                        return (400, json!({"error": "invalid_grant"}));
                    }
                    token()
                }
                Some("urn:ietf:params:oauth:grant-type:device_code") => token(),
                other => (400, json!({"error": format!("unsupported {other:?}")})),
            },
            _ => (404, json!({"error": "no route"})),
        }
    })
    .await;
    (fake, log)
}

fn pkce(verifier: &str) -> String {
    use base64::Engine;
    use sha2::Digest;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(sha2::Sha256::digest(verifier.as_bytes()))
}

/// Reads the child's stdout until a line contains `needle` (bounded).
async fn read_until(
    lines: &mut tokio::io::Lines<tokio::io::BufReader<tokio::process::ChildStdout>>,
    seen: &mut String,
    needle: &str,
) -> String {
    for _ in 0..200 {
        let l = tokio::time::timeout(Duration::from_secs(20), lines.next_line())
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {needle:?}; saw:\n{seen}"))
            .unwrap()
            .unwrap_or_else(|| panic!("stdout closed before {needle:?}; saw:\n{seen}"));
        seen.push_str(&l);
        seen.push('\n');
        if l.contains(needle) {
            return l;
        }
    }
    panic!("{needle:?} not found in:\n{seen}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browser_pkce_login_then_onboards_the_named_agents() {
    let (iss, _log) = issuer(true).await;
    let mut h = home();
    h.set("CUA_OIDC_ISSUER", &iss.url);
    mkdirs(&h, &[".kiro", ".codex"]);
    let mut child = h.spawn(&["auth", "login", "--agents", "kiro", "--yes"]);
    let mut lines = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
    let mut seen = String::new();
    let auth_line = read_until(&mut lines, &mut seen, "/auth?").await;
    let url = url::Url::parse(auth_line.trim()).unwrap();
    let q: std::collections::HashMap<String, String> = url.query_pairs().into_owned().collect();

    // A stray request and a wrong state are ignored.
    let redirect = q["redirect_uri"].clone();
    let base = redirect.trim_end_matches("/callback");
    let c = reqwest::Client::new();
    let r = c.get(format!("{base}/favicon.ico")).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 404);
    let r = c
        .get(format!("{redirect}?code=bad&state=wrong"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 400);
    // The browser comes back with the code.
    let r = c
        .get(format!("{redirect}?code=code-1&state={}", q["state"]))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let page = r.text().await.unwrap();
    assert!(page.contains("You're signed in") && page.contains("return to your terminal"));
    assert!(
        !page.contains("code-1") && !page.contains(&q["state"]),
        "{page}"
    );

    let status = tokio::time::timeout(Duration::from_secs(60), child.wait())
        .await
        .unwrap()
        .unwrap();
    while let Ok(Some(l)) = lines.next_line().await {
        seen.push_str(&l);
        seen.push('\n');
    }
    assert!(status.success(), "{seen}");
    assert!(
        seen.contains("Logged in to Cua as tester (tester@example.com)."),
        "{seen}"
    );
    assert!(
        !seen.contains("WXYZ"),
        "no device code in the browser flow: {seen}"
    );
    assert!(seen.contains("[5/5]"), "{seen}");
    assert!(
        seen.contains("Kiro: ~/.kiro/settings/mcp.json (added)"),
        "{seen}"
    );
    // Only the named agent: codex was not touched.
    assert!(!h.dir.path().join(".codex/config.toml").exists());
    let kiro = read_json(&h.dir.path().join(".kiro/settings/mcp.json"));
    assert_eq!(kiro["mcpServers"]["cua"]["args"], json!(["mcp"]));
    assert!(
        h.dir
            .path()
            .join(".kiro/skills/cua-spaces/SKILL.md")
            .is_file()
    );
    let creds = read_json(&h.cua_home().join("credentials.json"));
    assert_eq!(creds["refresh_token"], "rt-1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_falls_back_to_device_when_loopback_is_not_allowed_and_remote_skips_the_browser() {
    let (iss, log) = issuer(false).await;
    let mut h = home();
    h.set("CUA_OIDC_ISSUER", &iss.url)
        .set("CUA_OIDC_POLL_UNIT_MS", "5");
    let o = h.run(&["auth", "login"]).await;
    o.ok();
    assert!(o.stdout.contains("using a device code instead"), "{o:?}");
    assert!(o.stdout.contains("WXYZ-1234"), "{o:?}");
    assert!(o.stdout.contains("Logged in to Cua as tester"), "{o:?}");
    // Non-interactive with no --agents: no onboarding.
    assert!(!o.stdout.contains("AI coding agents"), "{o:?}");
    assert!(log.lock().unwrap().challenge.is_some(), "the preflight ran");

    // --remote never touches the authorization endpoint.
    let (iss, log) = issuer(true).await;
    let mut h = home();
    h.set("CUA_OIDC_ISSUER", &iss.url)
        .set("CUA_OIDC_POLL_UNIT_MS", "5");
    let o = h
        .run(&["auth", "login", "--remote", "--no-onboarding"])
        .await;
    o.ok();
    assert!(o.stdout.contains("WXYZ-1234"), "{o:?}");
    assert!(log.lock().unwrap().challenge.is_none());

    // Forcing PKCE against a provider that rejects it is a clear error.
    let (iss, _) = issuer(false).await;
    let mut h = home();
    h.set("CUA_OIDC_ISSUER", &iss.url);
    let o = h.run(&["auth", "login", "--flow", "pkce"]).await;
    assert_eq!(o.code, 4, "{o:?}");
    assert!(o.stderr.contains("browser sign-in is unavailable"), "{o:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agents_commands_round_trip_and_protect_malformed_files() {
    let h = home();
    mkdirs(&h, &[".codex", ".cursor", ".gemini"]);
    std::fs::write(
        h.dir.path().join(".gemini/settings.json"),
        "{\"mcpServers\": {",
    )
    .unwrap();
    std::fs::write(
        h.dir.path().join(".codex/config.toml"),
        "# mine\n[mcp_servers.other]\ncommand = \"o\"\n",
    )
    .unwrap();

    let o = h.run(&["--json", "agents", "detect"]).await;
    o.ok();
    let v = o.json();
    assert!(
        v["skills"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["name"] == "cua-spaces"),
        "{v}"
    );
    let installed: Vec<&str> = v["agents"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["installed"] == true)
        .map(|a| a["id"].as_str().unwrap())
        .collect();
    assert_eq!(installed, ["codex", "cursor", "gemini-cli"]);

    let o = h
        .run(&[
            "agents",
            "setup",
            "--yes",
            "--mcp-command",
            "/opt/cua/bin/cua",
        ])
        .await;
    assert_eq!(o.code, 1, "gemini's broken file fails the run: {o:?}");
    assert!(o.stdout.contains("✗ Gemini CLI"), "{o:?}");
    assert!(
        o.stdout
            .contains("✓ OpenAI Codex: ~/.codex/config.toml (added"),
        "{o:?}"
    );
    assert_eq!(
        std::fs::read_to_string(h.dir.path().join(".gemini/settings.json")).unwrap(),
        "{\"mcpServers\": {"
    );
    let codex = std::fs::read_to_string(h.dir.path().join(".codex/config.toml")).unwrap();
    assert!(codex.starts_with("# mine\n[mcp_servers.other]"), "{codex}");
    assert!(
        codex.contains("[mcp_servers.cua]\ncommand = \"/opt/cua/bin/cua\""),
        "{codex}"
    );

    // Idempotent re-run on the healthy agents.
    let o = h
        .run(&[
            "--json",
            "agents",
            "setup",
            "--agents",
            "codex,cursor",
            "--mcp-command",
            "/opt/cua/bin/cua",
        ])
        .await;
    o.ok();
    let v = o.json();
    let outcomes = v["outcomes"].as_array().unwrap();
    assert_eq!(
        outcomes.iter().filter(|m| m["target"] == "mcp").count(),
        2,
        "{v}"
    );
    assert!(outcomes.iter().all(|m| m["change"] == "unchanged"), "{v}");

    // The GUI installer's contract: --no-skills/--no-mcp, and a failure
    // exits non-zero with the reason on the last stderr line.
    let o = h
        .run(&[
            "--json",
            "agents",
            "setup",
            "--agents",
            "gemini",
            "--no-skills",
            "--yes",
        ])
        .await;
    assert_eq!(o.code, 1, "{o:?}");
    let v = o.json();
    assert_eq!(v["outcomes"][0]["change"], "failed");
    assert!(
        o.stderr
            .trim()
            .lines()
            .last()
            .unwrap()
            .contains("left unchanged"),
        "{o:?}"
    );

    let o = h.run(&["--json", "agents", "status"]).await;
    o.ok();
    let v = o.json();
    let cursor = v["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == "cursor")
        .unwrap();
    assert_eq!(cursor["cua_configured"], true);
    assert_eq!(cursor["cua_managed"], true);

    // remove without --yes in a non-interactive shell refuses.
    let o = h.run(&["agents", "remove"]).await;
    assert_eq!(o.code, 1);
    assert!(o.stdout.contains("--yes"), "{o:?}");
    let o = h.run(&["agents", "remove", "--yes"]).await;
    o.ok();
    let codex = std::fs::read_to_string(h.dir.path().join(".codex/config.toml")).unwrap();
    assert_eq!(codex, "# mine\n[mcp_servers.other]\ncommand = \"o\"\n");
    // The default skills include Cua Driver's: it is not Cua Spaces', so it
    // stays; the Spaces skills go.
    assert!(!h.dir.path().join(".agents/skills/cua-spaces").exists());
    assert!(h.dir.path().join(".agents/skills/cua-driver").is_dir());

    // Unknown agents are a usage error.
    let o = h
        .run(&["agents", "setup", "--agents", "nope", "--yes"])
        .await;
    assert_eq!(o.code, 2, "{o:?}");
}

/// The Local CLI (`CUA_CHANNEL=local`, its own home) registers `cua-local`
/// beside the Stable `cua` in the same agent config, installs `-local` skill
/// copies beside Stable's, and removes only its own (CUA-1776).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_local_cli_registers_cua_local_beside_stables_cua() {
    let mut h = home();
    mkdirs(&h, &[".cursor"]);
    let cursor = h.dir.path().join(".cursor/mcp.json");
    let stable_skill = h.dir.path().join(".agents/skills/cua-spaces");
    let local_skill = h.dir.path().join(".agents/skills/cua-spaces-local");

    // Stable first: `cua`.
    let o = h
        .run(&[
            "agents",
            "setup",
            "--agents",
            "cursor",
            "--yes",
            "--mcp-command",
            "/opt/cua/bin/cua",
        ])
        .await;
    o.ok();
    let before = std::fs::read_to_string(&cursor).unwrap();
    assert!(
        before.contains("\"cua\"") && !before.contains("cua-local"),
        "{before}"
    );
    assert!(stable_skill.join("SKILL.md").is_file());

    // Then the Local CLI, in its own home.
    let local_home = h.dir.path().join(".cua-local");
    h.set("CUA_CHANNEL", "local")
        .set("CUA_HOME", local_home.display().to_string());
    let o = h
        .run(&[
            "--json",
            "agents",
            "setup",
            "--agents",
            "cursor",
            "--yes",
            "--mcp-command",
            "/opt/cua-local/bin/cua-local",
        ])
        .await;
    o.ok();
    let v = o.json();
    let mcp: Vec<&str> = v["outcomes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["target"] == "mcp")
        .map(|m| m["item"].as_str().unwrap())
        .collect();
    assert_eq!(mcp, ["cua-local"], "{v}");
    let both: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cursor).unwrap()).unwrap();
    assert_eq!(both["mcpServers"]["cua"]["command"], "/opt/cua/bin/cua");
    assert_eq!(
        both["mcpServers"]["cua-local"]["command"],
        "/opt/cua-local/bin/cua-local"
    );
    assert!(local_skill.join("SKILL.md").is_file());
    assert!(
        std::fs::read_to_string(local_skill.join("SKILL.md"))
            .unwrap()
            .contains("name: cua-spaces-local")
    );
    assert!(local_home.join("agent-setup.json").is_file());

    let o = h.run(&["--json", "agents", "status"]).await;
    o.ok();
    let v = o.json();
    let c = v["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == "cursor")
        .unwrap();
    assert_eq!(
        (c["cua_configured"].clone(), c["cua_managed"].clone()),
        (true.into(), true.into()),
        "{c}"
    );
    assert!(
        v["skills"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["name"].as_str().unwrap().ends_with("-local")),
        "{v}"
    );

    // Removing Local's setup leaves Stable's entry and skills.
    h.run(&["agents", "remove", "--yes"]).await.ok();
    let left: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cursor).unwrap()).unwrap();
    assert_eq!(left["mcpServers"]["cua"]["command"], "/opt/cua/bin/cua");
    assert!(left["mcpServers"]["cua-local"].is_null(), "{left}");
    assert!(stable_skill.join("SKILL.md").is_file() && !local_skill.exists());
}

/// A stand-in cua-driver whose `mcp` answers one MCP `initialize`.
#[cfg(unix)]
fn fake_driver(h: &Home) -> String {
    use std::os::unix::fs::PermissionsExt;
    let p = h.dir.path().join("opt/drv/cua-driver");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(
        &p,
        "#!/bin/sh\nread line\nprintf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"serverInfo\":{\"name\":\"cua-driver\",\"version\":\"9.9.9\"}}}'\n",
    )
    .unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p.display().to_string()
}

/// What `uninstall-local.sh` runs (`cua-local agents remove --yes`) removes
/// Local's own MCP server and skills and nothing of the released app's: its
/// `cua` and `cua-driver` entries and skills stay, also when Local was asked to
/// set up `cua-driver` itself after Stable had (CUA-1776).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_local_cli_removes_only_its_own_entries_and_leaves_stables_cua_and_cua_driver() {
    let mut h = home();
    let drv = fake_driver(&h);
    mkdirs(&h, &[".cursor"]);
    let cursor = h.dir.path().join(".cursor/mcp.json");
    let root = h.dir.path().to_path_buf();
    let skill = move |name: &str| root.join(".agents/skills").join(name);
    let servers = |p: &std::path::Path| -> Vec<String> {
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
        let mut names: Vec<String> = v["mcpServers"]
            .as_object()
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default();
        names.sort();
        names
    };

    // The released app: `cua`, and Cua Driver's `cua-driver`.
    h.run(&[
        "agents",
        "setup",
        "--agents",
        "cursor",
        "--yes",
        "--mcp-command",
        "/opt/cua/bin/cua",
    ])
    .await
    .ok();
    h.run(&[
        "agents",
        "setup",
        "--cua-driver",
        "--agents",
        "cursor",
        "--yes",
        "--mcp-command",
        &drv,
    ])
    .await
    .ok();
    assert_eq!(servers(&cursor), ["cua", "cua-driver"]);
    let stable_config = std::fs::read_to_string(&cursor).unwrap();
    assert!(skill("cua-spaces").join("SKILL.md").is_file());
    assert!(skill("cua-driver").join("SKILL.md").is_file());

    // Local, in its own home: `cua-local`, and (asked to) `cua-driver` too.
    let local_home = h.dir.path().join(".cua-local");
    h.set("CUA_CHANNEL", "local")
        .set("CUA_HOME", local_home.display().to_string());
    h.run(&[
        "agents",
        "setup",
        "--agents",
        "cursor",
        "--yes",
        "--mcp-command",
        "/opt/cua-local/bin/cua-local",
    ])
    .await
    .ok();
    h.run(&[
        "agents",
        "setup",
        "--cua-driver",
        "--agents",
        "cursor",
        "--yes",
        "--mcp-command",
        &drv,
    ])
    .await
    .ok();
    assert_eq!(servers(&cursor), ["cua", "cua-driver", "cua-local"]);
    assert!(skill("cua-spaces-local").join("SKILL.md").is_file());

    // The uninstaller's call.
    h.run(&["agents", "remove", "--yes"]).await.ok();
    assert_eq!(
        servers(&cursor),
        ["cua", "cua-driver"],
        "only Local's server went"
    );
    let left: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cursor).unwrap()).unwrap();
    let before: serde_json::Value = serde_json::from_str(&stable_config).unwrap();
    assert_eq!(left, before, "Stable's entries are exactly as they were");
    assert!(!skill("cua-spaces-local").exists(), "Local's skills went");
    assert!(
        skill("cua-spaces").join("SKILL.md").is_file()
            && skill("cua-driver").join("SKILL.md").is_file(),
        "Stable's skills, cua-driver's included, stay"
    );
}

/// The other order: Local registered `cua-driver` first and the released app
/// later set up the same command. Local's record still says it wrote the entry,
/// but the name is Cua Driver's, so removing Local's setup leaves it, and the
/// released app's `cua-driver` skill, alone.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_local_cli_leaves_cua_driver_alone_even_when_it_registered_it_first() {
    let mut h = home();
    let drv = fake_driver(&h);
    mkdirs(&h, &[".cursor"]);
    let cursor = h.dir.path().join(".cursor/mcp.json");
    let root = h.dir.path().to_path_buf();
    let skill = move |name: &str| root.join(".agents/skills").join(name);
    let driver = [
        "agents",
        "setup",
        "--cua-driver",
        "--agents",
        "cursor",
        "--yes",
        "--mcp-command",
        &drv,
    ];

    let local_home = h.dir.path().join(".cua-local");
    h.set("CUA_CHANNEL", "local")
        .set("CUA_HOME", local_home.display().to_string());
    h.run(&driver).await.ok();
    h.run(&[
        "agents",
        "setup",
        "--agents",
        "cursor",
        "--yes",
        "--mcp-command",
        "/opt/cua-local/bin/cua-local",
    ])
    .await
    .ok();
    // Then the released app, its own home, the same command.
    let stable_home = h.dir.path().join(".cua").display().to_string();
    h.set("CUA_CHANNEL", "stable").set("CUA_HOME", stable_home);
    h.run(&driver).await.ok();
    assert!(skill("cua-driver").join("SKILL.md").is_file());

    h.set("CUA_CHANNEL", "local")
        .set("CUA_HOME", local_home.display().to_string());
    h.run(&["agents", "remove", "--yes"]).await.ok();
    let left: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cursor).unwrap()).unwrap();
    assert!(left["mcpServers"]["cua-local"].is_null(), "{left}");
    assert_eq!(
        left["mcpServers"]["cua-driver"]["command"],
        drv.as_str(),
        "Cua Driver's server stays: {left}"
    );
    assert!(
        skill("cua-driver").join("SKILL.md").is_file(),
        "the released app's cua-driver skill stays"
    );
    // `cua-driver-local` is Cua Driver's own sibling skill
    // (`is_cua_driver_name`), so it stays like `cua-driver`; only Cua Spaces'
    // skill goes.
    assert!(
        skill("cua-driver-local").exists(),
        "Cua Driver's Local skill stays"
    );
    assert!(
        !skill("cua-spaces-local").exists(),
        "Local's own skills went"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agents_setup_cua_driver_adds_the_driver_skill_and_server() {
    let h = home();
    let drv = fake_driver(&h);
    mkdirs(&h, &[".codex", ".claude"]);
    std::fs::write(
        h.dir.path().join(".codex/config.toml"),
        "[mcp_servers.other]\ncommand = \"o\"\n",
    )
    .unwrap();

    // What the installers run: every detected agent, no prompts.
    let o = h
        .run(&[
            "agents",
            "setup",
            "--cua-driver",
            "--agents",
            "all",
            "--yes",
            "--mcp-command",
            &drv,
        ])
        .await;
    o.ok();
    assert!(
        o.stdout
            .contains("Background computer-use (cua-driver) for:"),
        "{o:?}"
    );
    assert!(
        o.stdout
            .contains("MCP server cua-driver: ~/.codex/config.toml added"),
        "{o:?}"
    );
    assert!(!o.stdout.contains("not installed yet"), "{o:?}");
    let codex = std::fs::read_to_string(h.dir.path().join(".codex/config.toml")).unwrap();
    assert!(
        codex.contains(&format!(
            "[mcp_servers.cua-driver]\ncommand = \"{drv}\"\nargs = [\"mcp\"]"
        )),
        "{codex}"
    );
    assert!(
        !codex.contains("[mcp_servers.cua]"),
        "only the driver: {codex}"
    );
    // Only the cua-driver skill.
    assert!(
        h.dir
            .path()
            .join(".claude/skills/cua-driver/SKILL.md")
            .is_file()
    );
    assert!(!h.dir.path().join(".claude/skills/cua-spaces").exists());

    // JSON (the app and scripts), idempotent; status reports it.
    let o = h
        .run(&[
            "--json",
            "agents",
            "setup",
            "--cua-driver",
            "--agents",
            "codex",
            "--mcp-command",
            &drv,
        ])
        .await;
    o.ok();
    let v = o.json();
    assert_eq!(v["server"]["name"], "cua-driver");
    assert!(
        v["outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|x| x["change"] == "unchanged"),
        "{v}"
    );
    let o = h.run(&["--json", "agents", "status"]).await;
    let v = o.json();
    let codex = v["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == "codex")
        .unwrap();
    assert_eq!(codex["cua_driver_configured"], true, "{codex}");
    assert_eq!(codex["cua_configured"], false, "{codex}");

    // Plain `cua agents remove` leaves it: it removes Cua Spaces' entries
    // only. `--include-cua-driver` undoes it.
    let o = h.run(&["agents", "remove", "--yes"]).await;
    o.ok();
    let codex = std::fs::read_to_string(h.dir.path().join(".codex/config.toml")).unwrap();
    assert!(codex.contains("[mcp_servers.cua-driver]"), "{codex}");
    assert!(h.dir.path().join(".claude/skills/cua-driver").is_dir());
    let o = h
        .run(&["agents", "remove", "--include-cua-driver", "--yes"])
        .await;
    o.ok();
    let codex = std::fs::read_to_string(h.dir.path().join(".codex/config.toml")).unwrap();
    assert_eq!(codex, "[mcp_servers.other]\ncommand = \"o\"\n");
    assert!(!h.dir.path().join(".claude/skills/cua-driver").exists());
}

/// Without an installed cua-driver nothing is written: the row fails with
/// the reason and the CLI says how to install it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agents_setup_cua_driver_without_cua_driver_writes_nothing() {
    let h = home();
    mkdirs(&h, &[".claude"]);
    let o = h
        .run(&[
            "agents",
            "setup",
            "--cua-driver",
            "--agents",
            "claude",
            "--mcp-only",
            "--yes",
        ])
        .await;
    assert_eq!(o.code, 1, "{o:?}");
    assert!(o.stdout.contains("isn't installed"), "{o:?}");
    // The install one-liner is PowerShell's `-Only` on Windows, sh's `--only`
    // elsewhere.
    let hint = if cfg!(windows) {
        "-Only cua-driver"
    } else {
        "--only cua-driver"
    };
    assert!(o.stdout.contains(hint), "{o:?}");
    assert!(!h.dir.path().join(".claude.json").exists());
}

/// `cua agents remove` (what the uninstaller runs) takes Cua Spaces' `cua`
/// server and skills and leaves Cua Driver's `cua-driver` server and skill,
/// in the configs of Claude Code, Codex and Cursor; `--dry-run` lists exactly
/// that and changes nothing; `--include-cua-driver` takes Cua Driver's too.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agents_remove_keeps_cua_driver_and_dry_run_lists_what_it_removes() {
    let h = home();
    let drv = fake_driver(&h);
    mkdirs(&h, &[".claude", ".codex", ".cursor"]);
    let at = |rel: &str| h.dir.path().join(rel);
    std::fs::write(
        at(".claude.json"),
        "{\"mcpServers\": {\"other\": {\"command\": \"o\"}}}",
    )
    .unwrap();
    std::fs::write(
        at(".codex/config.toml"),
        "[mcp_servers.other]\ncommand = \"o\"\n",
    )
    .unwrap();
    std::fs::write(
        at(".cursor/mcp.json"),
        "{\"mcpServers\": {\"other\": {\"command\": \"o\"}}}",
    )
    .unwrap();
    let agents = "claude,codex,cursor";
    h.run(&[
        "agents",
        "setup",
        "--agents",
        agents,
        "--yes",
        "--mcp-command",
        "/opt/cua/bin/cua",
    ])
    .await
    .ok();
    h.run(&[
        "agents",
        "setup",
        "--cua-driver",
        "--agents",
        agents,
        "--yes",
        "--mcp-command",
        &drv,
    ])
    .await
    .ok();
    let configs = [".claude.json", ".codex/config.toml", ".cursor/mcp.json"];
    let read = |rel: &str| std::fs::read_to_string(at(rel)).unwrap();
    for c in configs {
        let text = read(c);
        assert!(text.contains("/opt/cua/bin/cua"), "{c}: {text}");
        assert!(text.contains(&drv), "{c}: {text}");
    }
    let state = at(".cua/agent-setup.json");
    let snapshot = || -> Vec<String> {
        configs
            .iter()
            .map(|c| read(c))
            .chain([std::fs::read_to_string(&state).unwrap()])
            .chain(["cua-spaces", "cua-volume", "cua-driver"].map(|s| {
                std::fs::read_to_string(at(&format!(".claude/skills/{s}/SKILL.md"))).unwrap()
            }))
            .collect()
    };
    let before = snapshot();

    // The preview needs no --yes, lists each entry and skill it would
    // remove, names what it keeps, and writes nothing.
    let o = h.run(&["agents", "remove", "--dry-run"]).await;
    o.ok();
    assert_eq!(snapshot(), before, "{o:?}");
    let out = &o.stdout;
    assert!(out.contains("Dry run: nothing is changed."), "{o:?}");
    for (cfg, shown) in [
        (".claude.json", "~/.claude.json"),
        (".codex/config.toml", "~/.codex/config.toml"),
        (".cursor/mcp.json", "~/.cursor/mcp.json"),
    ] {
        let _ = cfg;
        assert!(
            out.contains(&format!("would remove MCP server cua: {shown}")),
            "{o:?}"
        );
        assert!(
            out.contains(&format!("MCP server cua-driver: {shown} kept")),
            "{o:?}"
        );
    }
    for skill in [
        "cua-spaces",
        "cua-volume",
        "cua-sandboxes",
        "gui-automation",
    ] {
        assert!(
            out.contains(&format!(
                "would remove skill {skill}: ~/.claude/skills/{skill}"
            )),
            "{o:?}"
        );
    }
    assert!(
        out.contains("skill cua-driver: ~/.claude/skills/cua-driver kept"),
        "{o:?}"
    );
    assert!(!out.contains("would remove MCP server cua-driver"), "{o:?}");
    assert!(!out.contains("would remove skill cua-driver"), "{o:?}");

    // The JSON preview carries the same rows.
    let o = h.run(&["--json", "agents", "remove", "--dry-run"]).await;
    o.ok();
    let v = o.json();
    assert_eq!(v["dry_run"], true, "{v}");
    let removed: Vec<&str> = v["outcomes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| x["change"] == "removed")
        .map(|x| x["item"].as_str().unwrap())
        .collect();
    assert!(removed.contains(&"cua"), "{v}");
    assert!(!removed.contains(&"cua-driver"), "{v}");
    assert_eq!(snapshot(), before);

    // The real run removes what the preview listed, and only that.
    let o = h.run(&["agents", "remove", "--yes"]).await;
    o.ok();
    assert!(
        !o.stdout.contains("cua-driver: ~/.claude.json removed"),
        "{o:?}"
    );
    for c in configs {
        let text = read(c);
        assert!(!text.contains("/opt/cua/bin/cua"), "{c}: {text}");
        assert!(text.contains(&drv), "{c}: {text}");
        assert!(text.contains("other"), "{c}: {text}");
    }
    assert!(!at(".claude/skills/cua-spaces").exists());
    assert!(at(".claude/skills/cua-driver/SKILL.md").is_file());
    let o = h.run(&["agents", "remove", "--dry-run"]).await;
    o.ok();
    assert!(!o.stdout.contains("would remove"), "{o:?}");

    // Asked for, Cua Driver's go too; the previews say so first.
    let o = h
        .run(&["agents", "remove", "--include-cua-driver", "--dry-run"])
        .await;
    o.ok();
    assert!(
        o.stdout
            .contains("would remove MCP server cua-driver: ~/.codex/config.toml"),
        "{o:?}"
    );
    assert!(
        o.stdout
            .contains("would remove skill cua-driver: ~/.claude/skills/cua-driver"),
        "{o:?}"
    );
    h.run(&["agents", "remove", "--include-cua-driver", "--yes"])
        .await
        .ok();
    for c in configs {
        let text = read(c);
        assert!(!text.contains("cua-driver"), "{c}: {text}");
        assert!(text.contains("other"), "{c}: {text}");
    }
    assert!(!at(".claude/skills/cua-driver").exists());
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agents_setup_cua_driver_says_where_the_skill_went_and_no_skills_skips_it() {
    let h = home();
    let drv = fake_driver(&h);
    mkdirs(&h, &[".codex", ".claude"]);
    let skill = |rel: &str| h.dir.path().join(rel).join("cua-driver/SKILL.md");

    // Default: the skill lands beside the MCP entry, with one line saying so.
    let o = h
        .run(&[
            "agents",
            "setup",
            "--cua-driver",
            "--agents",
            "claude",
            "--yes",
            "--mcp-command",
            &drv,
        ])
        .await;
    o.ok();
    let said: Vec<&str> = o
        .stdout
        .lines()
        .filter(|l| l.contains("skill cua-driver"))
        .collect();
    assert_eq!(said.len(), 1, "{o:?}");
    assert!(said[0].contains(".claude/skills/cua-driver"), "{o:?}");
    assert!(skill(".claude/skills").is_file());

    // --no-skills (the installers' flag): the server only, nothing printed
    // about a skill, and Codex's skills folder is never created.
    let o = h
        .run(&[
            "agents",
            "setup",
            "--cua-driver",
            "--agents",
            "codex",
            "--no-skills",
            "--yes",
            "--mcp-command",
            &drv,
        ])
        .await;
    o.ok();
    assert!(
        o.stdout
            .contains("MCP server cua-driver: ~/.codex/config.toml added"),
        "{o:?}"
    );
    assert!(!o.stdout.contains("skill cua-driver"), "{o:?}");
    assert!(!skill(".agents/skills").exists());
    assert!(!h.dir.path().join(".agents/skills").exists());
}

// ------------------------------------------------------------ PTY (expect)

/// A `cua` process on a pseudo-terminal, driven like `expect`.
struct Pty {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    buf: Arc<Mutex<String>>,
}

impl Pty {
    fn spawn(h: &Home, args: &[&str]) -> Self {
        use portable_pty::{CommandBuilder, PtySize, native_pty_system};
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 50,
                cols: 200,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_cua"));
        cmd.args(args);
        cmd.env_clear();
        // Windows needs its system variables back after env_clear: without
        // SystemRoot, Winsock can't load its providers and every HTTP request
        // fails ("error sending request").
        #[cfg(windows)]
        for k in [
            "SystemRoot",
            "SystemDrive",
            "windir",
            "TEMP",
            "TMP",
            "ComSpec",
            "PATHEXT",
        ] {
            if let Some(v) = std::env::var_os(k) {
                cmd.env(k, v);
            }
        }
        for (k, v) in &h.env {
            cmd.env(k, v);
        }
        // Never a real browser, never the keychain (both also set by Home).
        cmd.env("CUA_NO_BROWSER", "1");
        cmd.env("CUA_CREDENTIAL_STORE", "file");
        cmd.env("TERM", "dumb");
        cmd.cwd(h.dir.path());
        let child = pair.slave.spawn_command(cmd).unwrap();
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().unwrap();
        let writer = pair.master.take_writer().unwrap();
        let buf = Arc::new(Mutex::new(String::new()));
        let b = buf.clone();
        std::thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            // Bounded: at most 4 MiB of output.
            while b.lock().unwrap().len() < 4 << 20 {
                match reader.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => b
                        .lock()
                        .unwrap()
                        .push_str(&String::from_utf8_lossy(&chunk[..n])),
                }
            }
        });
        // Keep the master alive for the process lifetime.
        std::mem::forget(pair.master);
        #[allow(unused_mut)]
        let mut pty = Pty { child, writer, buf };
        // Windows ConPTY opens with a cursor-position query (ESC[6n) and
        // holds the child's output until the terminal answers; a real
        // terminal does that, so the harness answers too.
        #[cfg(windows)]
        {
            pty.expect("\x1b[6n", 0);
            pty.send("\x1b[1;1R");
        }
        pty
    }

    /// Waits until the output contains `needle` after byte `from`; returns
    /// the end offset of the match.
    fn expect(&self, needle: &str, from: usize) -> usize {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            {
                let b = self.buf.lock().unwrap();
                if let Some(i) = b.get(from..).and_then(|s| s.find(needle)) {
                    return from + i + needle.len();
                }
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {needle:?}; output:\n{}",
                self.buf.lock().unwrap()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Everything printed so far. Only the Unix tests read it, so Windows clippy
    /// (`-D warnings`) would flag it as dead code.
    #[cfg(unix)]
    fn output(&self) -> String {
        self.buf.lock().unwrap().clone()
    }

    fn send(&mut self, s: &str) {
        self.writer.write_all(s.as_bytes()).unwrap();
        self.writer.flush().unwrap();
    }

    fn wait(&mut self) -> (u32, String) {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Some(s) = self.child.try_wait().unwrap() {
                std::thread::sleep(Duration::from_millis(100));
                return (s.exit_code(), self.buf.lock().unwrap().clone());
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                panic!("cua did not exit; output:\n{}", self.buf.lock().unwrap());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interactive_login_onboarding_prompts() {
    let (iss, _) = issuer(true).await;
    let mut h = home();
    h.set("CUA_OIDC_ISSUER", &iss.url)
        .set("CUA_OIDC_POLL_UNIT_MS", "5")
        .set("CUA_AUTH_FLOW", "device");
    mkdirs(&h, &[".claude", ".codex"]);
    let h = h;
    tokio::task::spawn_blocking(move || {
        // Yes to onboarding, default (yes) to skills, no to MCP.
        let mut p = Pty::spawn(&h, &["auth", "login"]);
        let at = p.expect("Logged in to Cua as tester", 0);
        let at = p.expect(
            "Let your AI coding agents (like Claude Code and Codex) use cua? [y/n/never]",
            at,
        );
        p.send("y\r");
        let at = p.expect("✓ Claude Code", at);
        let at = p.expect("✗ Cursor", at);
        let at = p.expect("Install 5 default cua skills? [Y/n]", at);
        p.send("\r");
        let at = p.expect("[1/5] ✓ cua-driver (installed, 2 locations)", at);
        let at = p.expect("[5/5]", at);
        p.expect("Configure cua MCP server connection? [Y/n]", at);
        p.send("n\r");
        let (code, out) = p.wait();
        assert_eq!(code, 0, "{out}");
        assert!(
            out.contains("Done: 5 skills in 2 locations; MCP configured for 0 agents."),
            "{out}"
        );
        assert!(
            h.dir
                .path()
                .join(".claude/skills/cua-driver/SKILL.md")
                .is_file()
        );
        assert!(
            h.dir
                .path()
                .join(".agents/skills/cua-driver/SKILL.md")
                .is_file()
        );
        assert!(
            !h.dir.path().join(".codex/config.toml").exists(),
            "MCP declined"
        );

        // "never" is remembered in ~/.cua/config.
        let mut p = Pty::spawn(&h, &["auth", "login"]);
        p.expect("[y/n/never]", 0);
        p.send("never\r");
        let (code, out) = p.wait();
        assert_eq!(code, 0, "{out}");
        let cfg = std::fs::read_to_string(h.cua_home().join("config")).unwrap();
        assert!(cfg.contains("agents = \"never\""), "{cfg}");
        let mut p = Pty::spawn(&h, &["auth", "login"]);
        let (code, out) = p.wait();
        assert_eq!(code, 0, "{out}");
        assert!(!out.contains("[y/n/never]"), "not asked again: {out}");

        // `cua agents setup` still works on demand, and asks per step.
        let mut p = Pty::spawn(
            &h,
            &[
                "agents",
                "setup",
                "--mcp-only",
                "--mcp-command",
                "/opt/cua/bin/cua",
            ],
        );
        let at = p.expect("Configure cua MCP server connection? [Y/n]", 0);
        p.send("maybe\r");
        let at = p.expect("Please answer y or n.", at);
        p.send("y\r");
        p.expect("MCP configured for 2 agents", at);
        let (code, out) = p.wait();
        assert_eq!(code, 0, "{out}");
        assert!(!out.contains("default cua skills"), "--mcp-only: {out}");
        let claude = read_json(&h.dir.path().join(".claude.json"));
        assert_eq!(claude["mcpServers"]["cua"]["command"], "/opt/cua/bin/cua");
    })
    .await
    .unwrap();
    drop(iss);
}

/// The sign-in's agent question is answered by a person, not by input that
/// was already waiting (an extra Enter at an earlier prompt, a key pressed
/// during the browser step) and not by the input ending.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn onboarding_question_is_not_answered_by_typeahead() {
    let (iss, _) = issuer(true).await;
    let mut h = home();
    h.set("CUA_OIDC_ISSUER", &iss.url)
        .set("CUA_OIDC_POLL_UNIT_MS", "5")
        .set("CUA_AUTH_FLOW", "device");
    mkdirs(&h, &[".claude"]);
    let h = h;
    tokio::task::spawn_blocking(move || {
        let mut p = Pty::spawn(&h, &["auth", "login"]);
        // Two Enters typed before the question exists.
        p.send("\r\r");
        let at = p.expect("[y/n/never]", 0);
        // Had the stray Enter been taken as "no", this would already be there.
        std::thread::sleep(Duration::from_millis(500));
        assert!(
            !p.output().contains("Skipped."),
            "typeahead answered the question: {}",
            p.output()
        );
        p.send("y\r");
        let at = p.expect("✓ Claude Code", at);
        let at = p.expect("default cua skills? [Y/n]", at);
        p.send("\r");
        p.expect("Configure cua MCP server connection? [Y/n]", at);
        p.send("n\r");
        let (code, out) = p.wait();
        assert_eq!(code, 0, "{out}");
        assert!(
            h.dir
                .path()
                .join(".claude/skills/cua-driver/SKILL.md")
                .is_file(),
            "the answer after the wait was taken: {out}"
        );
    })
    .await
    .unwrap();
    drop(iss);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn onboarding_question_with_no_answer_changes_nothing() {
    let (iss, _) = issuer(true).await;
    let mut h = home();
    h.set("CUA_OIDC_ISSUER", &iss.url)
        .set("CUA_OIDC_POLL_UNIT_MS", "5")
        .set("CUA_AUTH_FLOW", "device");
    mkdirs(&h, &[".claude"]);
    let h = h;
    tokio::task::spawn_blocking(move || {
        let mut p = Pty::spawn(&h, &["auth", "login"]);
        p.expect("[y/n/never]", 0);
        // Ctrl-D at an empty line: the input ends.
        p.send("\x04");
        let (code, out) = p.wait();
        assert_eq!(code, 0, "{out}");
        assert!(
            out.contains("No answer (the input ended), so your agents are unchanged."),
            "{out}"
        );
        assert!(!out.contains("Skipped."), "no input is not a 'no': {out}");
        assert!(
            !h.dir.path().join(".claude/skills").exists(),
            "nothing was installed"
        );
        // Not recorded as "never": the next sign-in asks again.
        let cfg = std::fs::read_to_string(h.cua_home().join("config")).unwrap_or_default();
        assert!(!cfg.contains("never"), "{cfg}");
        let mut p = Pty::spawn(&h, &["auth", "login"]);
        p.expect("[y/n/never]", 0);
        p.send("n\r");
        let (code, out) = p.wait();
        assert_eq!(code, 0, "{out}");
    })
    .await
    .unwrap();
    drop(iss);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_spaces_app_session_migrates_into_the_shared_store() {
    let h = home();
    std::fs::create_dir_all(h.cua_home()).unwrap();
    let at = jwt(json!({"preferred_username": "app-user", "sub": "u-9"}));
    std::fs::write(
        h.cua_home().join("spaces-session.json"),
        json!({"access_token": at, "refresh_token": "rt-app", "expires_at": 4102444800u64})
            .to_string(),
    )
    .unwrap();
    let o = h.run(&["--json", "auth", "status"]).await;
    o.ok();
    let v = o.json();
    assert_eq!(v["logged_in"], true, "{v}");
    assert_eq!(v["identity"]["username"], "app-user");
    assert!(!h.cua_home().join("spaces-session.json").exists());
    let creds = read_json(&h.cua_home().join("credentials.json"));
    assert_eq!(creds["refresh_token"], "rt-app");
}
