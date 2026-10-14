//! End-to-end behaviour of cua-agent-setup against fixture configs for every
//! agent format, always in a temporary HOME (never the real one).

use cua_agent_setup::{
    AGENTS, AgentSetup, Change, Format, HostEnv, McpServer, Outcome, Parts, Target, registry,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

struct T {
    _dir: tempfile::TempDir,
    home: PathBuf,
    env: HostEnv,
}

impl T {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let env = HostEnv::isolated(&home);
        T {
            _dir: dir,
            home,
            env,
        }
    }
    fn setup(&self) -> AgentSetup {
        AgentSetup::new(self.env.clone())
    }
    fn write(&self, rel: &str, text: &str) -> PathBuf {
        let p = self.home.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
        p
    }
    fn read(&self, p: &Path) -> String {
        std::fs::read_to_string(p).unwrap()
    }
    fn backups_of(&self, p: &Path) -> Vec<PathBuf> {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        let mut v: Vec<PathBuf> = std::fs::read_dir(p.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|x| {
                x.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(&format!("{name}.cua-backup-"))
            })
            .collect();
        v.sort();
        v
    }
}

fn server() -> McpServer {
    McpServer::new("/opt/cua/bin/cua")
}

fn ids(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn one(out: Vec<Outcome>) -> Outcome {
    assert_eq!(out.len(), 1, "{out:#?}");
    out.into_iter().next().unwrap()
}

/// A user config for `agent` holding one unrelated server (and comments
/// where the format has them), plus the parsed path to the servers object.
fn fixture(agent: &str) -> (&'static str, String) {
    match agent {
        "claude-code" => (
            ".claude.json",
            "{\n  \"numStartups\": 12,\n  \"mcpServers\": {\n    \"other\": {\"type\": \"stdio\", \"command\": \"other-mcp\", \"args\": []}\n  },\n  \"projects\": {}\n}\n".into(),
        ),
        "codex" => (
            ".codex/config.toml",
            "# my codex config\nmodel = \"gpt-5\" # pinned\n\n[mcp_servers.other]\ncommand = \"other-mcp\" # keep\nargs = []\n".into(),
        ),
        "cursor" => (".cursor/mcp.json", jsonc("mcpServers", "\"type\": \"stdio\", \"command\": \"other-mcp\"")),
        "gemini-cli" => (".gemini/settings.json", jsonc("mcpServers", "\"command\": \"other-mcp\"")),
        "cline" => (".cline/data/settings/cline_mcp_settings.json", jsonc("mcpServers", "\"command\": \"other-mcp\", \"disabled\": false")),
        "kiro" => (".kiro/settings/mcp.json", jsonc("mcpServers", "\"command\": \"other-mcp\"")),
        "openclaw" => (
            ".openclaw/openclaw.json",
            "{\n  // JSON5 config\n  agents: { defaults: { model: 'x' } },\n  mcp: {\n    servers: {\n      other: { command: 'other-mcp', args: [] },\n    },\n  },\n}\n".into(),
        ),
        "opencode" => (
            ".config/opencode/opencode.json",
            "{\n  \"$schema\": \"https://opencode.ai/config.json\",\n  // mine\n  \"mcp\": {\n    \"other\": {\"type\": \"local\", \"command\": [\"other-mcp\"], \"enabled\": true}\n  }\n}\n".into(),
        ),
        "windsurf" => (".config/devin/mcp_config.json", jsonc("mcpServers", "\"command\": \"other-mcp\"")),
        "copilot-cli" => (".copilot/mcp-config.json", jsonc("mcpServers", "\"type\": \"local\", \"command\": \"other-mcp\", \"tools\": [\"*\"]")),
        "amp" => (
            ".config/amp/settings.json",
            "{\n  // amp\n  \"amp.notifications.enabled\": true,\n  \"amp.mcpServers\": {\n    \"other\": {\"command\": \"other-mcp\"}\n  }\n}\n".into(),
        ),
        "goose" => (
            ".config/goose/config.yaml",
            "# goose config\nGOOSE_PROVIDER: anthropic\nextensions:\n  other:\n    type: stdio\n    name: other\n    cmd: other-mcp\n    args: []\n    enabled: true\n".into(),
        ),
        "hermes" => (hermes_rel("config.yaml"), HERMES_CONFIG.into()),
        "zed" => (
            ".config/zed/settings.json",
            "// Zed settings\n{\n  \"theme\": \"One Dark\",\n  \"context_servers\": {\n    \"other\": {\"command\": \"other-mcp\", \"args\": []}\n  }\n}\n".into(),
        ),
        "vscode" => (vscode_rel(), jsonc("servers", "\"type\": \"stdio\", \"command\": \"other-mcp\"")),
        "antigravity" => (".gemini/config/mcp_config.json", jsonc("mcpServers", "\"command\": \"other-mcp\"")),
        other => panic!("no fixture for {other}"),
    }
}

/// A Hermes config.yaml as `hermes setup` leaves it: the seeded template's
/// comments (including a commented-out `mcp_servers` example), live
/// settings, and one MCP server of the user's.
const HERMES_CONFIG: &str = "# Hermes Agent configuration\nmodel:\n  default: anthropic/claude-opus-4.6  # pick one\n  provider: openrouter\n\n# mcp_servers:\n#   time:\n#     command: uvx\n#     args: [\"mcp-server-time\"]\nmcp_servers:\n  other:\n    command: other-mcp  # mine\n    args: []\n\napprovals:\n  mode: smart\n";

/// `rel` under the default Hermes home of this OS.
fn hermes_rel(rel: &str) -> &'static str {
    let base = if cfg!(windows) {
        "AppData/Local/hermes"
    } else {
        ".hermes"
    };
    Box::leak(format!("{base}/{rel}").into_boxed_str())
}

fn vscode_rel() -> &'static str {
    if cfg!(target_os = "macos") {
        "Library/Application Support/Code/User/mcp.json"
    } else if cfg!(windows) {
        "AppData/Roaming/Code/User/mcp.json"
    } else {
        ".config/Code/User/mcp.json"
    }
}

fn jsonc(key: &str, other_fields: &str) -> String {
    format!(
        "{{\n  // user comment\n  \"{key}\": {{\n    /* other server */\n    \"other\": {{{other_fields}}},\n  }},\n  \"unrelated\": [1, 2, 3],\n}}\n"
    )
}

fn has_comment(format: Format, agent: &str) -> Option<&'static str> {
    match (format, agent) {
        (Format::Yaml, "goose") => Some("# goose config"),
        (Format::Yaml, _) => Some("# mcp_servers:\n#   time:"),
        (_, "claude-code") => None,
        (Format::Toml, _) => Some("# my codex config"),
        (_, "openclaw") => Some("// JSON5 config"),
        (_, "opencode") => Some("// mine"),
        (_, "amp") => Some("// amp"),
        (_, "zed") => Some("// Zed settings"),
        _ => Some("/* other server */"),
    }
}

#[test]
fn every_mcp_agent_merges_idempotently_backs_up_and_removes_only_its_entry() {
    let with_mcp: Vec<&registry::AgentSpec> = AGENTS.iter().filter(|a| a.mcp.is_some()).collect();
    assert!(with_mcp.len() >= 14, "registry lost agents");
    for a in with_mcp {
        let t = T::new();
        let (rel, text) = fixture(a.id);
        let file = t.write(rel, &text);
        let m = a.mcp.unwrap();
        let s = t.setup();

        // Configure.
        let o = one(s.configure_mcp(&ids(&[a.id]), &server()).unwrap());
        assert_eq!(o.change, Change::Created, "{}: {o:?}", a.id);
        assert_eq!(o.path, file, "{}", a.id);
        let after = t.read(&file);
        let entry = cua_agent_setup_get(m.format, &file, &after, m.key_path, "cua");
        assert_eq!(
            entry,
            Some(cua_agent_setup::edit_value(&m.shape.fields(&server()))),
            "{}:\n{after}",
            a.id
        );
        assert!(
            cua_agent_setup_get(m.format, &file, &after, m.key_path, "other").is_some(),
            "{} lost the other server:\n{after}",
            a.id
        );
        if let Some(c) = has_comment(m.format, a.id) {
            assert!(after.contains(c), "{} lost comment {c:?}:\n{after}", a.id);
        }
        // Backup of the original, once.
        let backups = t.backups_of(&file);
        assert_eq!(backups.len(), 1, "{}", a.id);
        assert_eq!(t.read(&backups[0]), text, "{}", a.id);
        assert_eq!(o.backup.as_deref(), Some(backups[0].as_path()));

        // Idempotent: unchanged, no new backup, same bytes.
        let o = one(s.configure_mcp(&ids(&[a.id]), &server()).unwrap());
        assert_eq!(o.change, Change::Unchanged, "{}", a.id);
        assert_eq!(t.read(&file), after, "{}", a.id);
        assert_eq!(t.backups_of(&file).len(), 1);

        // Status.
        let st = s.detect().into_iter().find(|x| x.id == a.id).unwrap();
        assert!(st.cua_configured && st.cua_managed, "{}: {st:?}", a.id);
        assert_eq!(st.mcp_config.as_deref(), Some(file.as_path()));
        assert!(st.installed, "{}: the config marks it installed", a.id);

        // A new command updates the entry (still one backup).
        let o = one(s
            .configure_mcp(&ids(&[a.id]), &McpServer::new("/new/cua"))
            .unwrap());
        assert_eq!(o.change, Change::Updated, "{}", a.id);
        assert_eq!(t.backups_of(&file).len(), 1);

        // Remove: only ours goes, the other server and comments stay.
        let o = one(s
            .remove(
                &ids(&[a.id]),
                Parts {
                    skills: false,
                    mcp: true,
                },
            )
            .unwrap());
        assert_eq!(o.change, Change::Removed, "{}: {o:?}", a.id);
        let removed = t.read(&file);
        assert!(
            cua_agent_setup_get(m.format, &file, &removed, m.key_path, "cua").is_none(),
            "{}",
            a.id
        );
        assert!(
            cua_agent_setup_get(m.format, &file, &removed, m.key_path, "other").is_some(),
            "{}",
            a.id
        );
        if let Some(c) = has_comment(m.format, a.id) {
            assert!(removed.contains(c), "{}", a.id);
        }
        if matches!(m.format, Format::Toml | Format::Yaml) {
            assert_eq!(removed, text, "{}: round-trips byte for byte", a.id);
        }
        // Removing again is a no-op.
        let o = one(s.remove(&ids(&[a.id]), Parts::ALL).unwrap());
        assert_eq!(o.change, Change::Skipped, "{}", a.id);
        assert_eq!(t.read(&file), removed);
    }
}

// Thin wrappers over the crate's public test helpers.
fn cua_agent_setup_get(
    f: Format,
    file: &Path,
    text: &str,
    key: &[&str],
    name: &str,
) -> Option<Value> {
    cua_agent_setup::edit_get(f, file, text, key, name).unwrap()
}

#[test]
fn missing_configs_are_created_with_private_permissions() {
    let t = T::new();
    let s = t.setup();
    let out = s
        .configure_mcp(&ids(&["codex", "cursor", "opencode", "goose"]), &server())
        .unwrap();
    for o in &out {
        assert_eq!(o.change, Change::Created, "{o:?}");
        assert!(o.backup.is_none(), "nothing to back up");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&o.path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{}", o.path.display());
        }
    }
    let codex = t.read(&t.home.join(".codex/config.toml"));
    assert_eq!(
        codex,
        "[mcp_servers.cua]\ncommand = \"/opt/cua/bin/cua\"\nargs = [\"mcp\"]\n"
    );
    let oc: Value =
        serde_json::from_str(&t.read(&t.home.join(".config/opencode/opencode.json"))).unwrap();
    assert_eq!(
        oc["mcp"]["cua"],
        json!({"type": "local", "command": ["/opt/cua/bin/cua", "mcp"], "enabled": true})
    );
}

#[test]
fn malformed_configs_fail_clearly_and_are_not_touched() {
    let t = T::new();
    let broken_json = t.write(".cursor/mcp.json", "{ \"mcpServers\": { \"other\": ");
    let broken_toml = t.write(".codex/config.toml", "[mcp_servers.other\ncommand = 1\n");
    let wrong_shape = t.write(".kiro/settings/mcp.json", "{\"mcpServers\": [1]}");
    // A commented YAML file whose servers are a flow mapping: the splice
    // does not handle it, and a serde round trip would drop the comment.
    let commented_yaml = t.write(
        ".config/goose/config.yaml",
        "# mine\nextensions: {other: {cmd: x}}\n",
    );
    let s = t.setup();
    let out = s
        .configure_mcp(
            &ids(&["cursor", "codex", "kiro", "goose", "gemini-cli"]),
            &server(),
        )
        .unwrap();
    for (o, p) in out
        .iter()
        .zip([&broken_json, &broken_toml, &wrong_shape, &commented_yaml])
    {
        assert_eq!(o.change, Change::Failed, "{o:?}");
        assert!(o.detail.contains(&p.display().to_string()), "{}", o.detail);
        assert!(o.detail.contains("left unchanged"), "{}", o.detail);
        assert!(t.backups_of(p).is_empty());
    }
    assert_eq!(t.read(&broken_json), "{ \"mcpServers\": { \"other\": ");
    assert_eq!(t.read(&broken_toml), "[mcp_servers.other\ncommand = 1\n");
    assert_eq!(
        t.read(&commented_yaml),
        "# mine\nextensions: {other: {cmd: x}}\n"
    );
    // The healthy agent in the same batch still got configured.
    assert_eq!(out[4].change, Change::Created);
    // Detection reports the error instead of guessing.
    let st = s.detect().into_iter().find(|a| a.id == "cursor").unwrap();
    assert!(
        st.error.as_deref().unwrap().contains("left unchanged"),
        "{st:?}"
    );
    assert!(!st.cua_configured);
}

#[test]
fn a_foreign_cua_entry_is_restored_on_remove_and_user_edits_are_respected() {
    let t = T::new();
    let file = t.write(
        ".cursor/mcp.json",
        "{\"mcpServers\": {\"cua\": {\"command\": \"python\", \"args\": [\"my_cua.py\"]}}}",
    );
    let s = t.setup();
    assert_eq!(
        one(s.configure_mcp(&ids(&["cursor"]), &server()).unwrap()).change,
        Change::Updated
    );
    let o = one(s.remove(&ids(&["cursor"]), Parts::ALL).unwrap());
    assert_eq!(o.detail, "restored the previous entry");
    let v: Value = serde_json::from_str(&t.read(&file)).unwrap();
    assert_eq!(
        v["mcpServers"]["cua"],
        json!({"command": "python", "args": ["my_cua.py"]})
    );

    // An old cua registration (for example `cua daemon mcp` from the Spaces
    // app) is replaced, and remove deletes it rather than restoring it.
    let t = T::new();
    let file = t.write(
        ".cursor/mcp.json",
        "{\"mcpServers\": {\"cua\": {\"command\": \"/usr/local/bin/cua\", \"args\": [\"daemon\", \"mcp\"]}}}",
    );
    let s = t.setup();
    s.configure_mcp(&ids(&["cursor"]), &server()).unwrap();
    s.remove(&ids(&["cursor"]), Parts::ALL).unwrap();
    let v: Value = serde_json::from_str(&t.read(&file)).unwrap();
    assert!(v["mcpServers"].get("cua").is_none(), "{v}");

    // The user edits our entry afterwards: remove leaves it alone.
    let t = T::new();
    let s = t.setup();
    s.configure_mcp(&ids(&["gemini-cli"]), &server()).unwrap();
    let file = t.home.join(".gemini/settings.json");
    let edited = "{\"mcpServers\": {\"cua\": {\"command\": \"/opt/cua/bin/cua\", \"args\": [\"mcp\", \"--permissions\", \"sandbox:all\"]}}}";
    std::fs::write(&file, edited).unwrap();
    let o = one(s.remove(&ids(&["gemini-cli"]), Parts::ALL).unwrap());
    assert_eq!(o.change, Change::Skipped);
    assert!(
        o.detail.contains("changed since cua wrote it"),
        "{}",
        o.detail
    );
    assert_eq!(t.read(&file), edited);
}

#[test]
fn agent_overrides_and_xdg_are_honored() {
    let mut t = T::new();
    let codex_home = t.home.join("elsewhere/codex");
    let claude_dir = t.home.join("elsewhere/claude");
    let xdg = t.home.join("xdg");
    t.env
        .vars
        .insert("CODEX_HOME".into(), codex_home.display().to_string());
    t.env
        .vars
        .insert("CLAUDE_CONFIG_DIR".into(), claude_dir.display().to_string());
    t.env
        .vars
        .insert("XDG_CONFIG_HOME".into(), xdg.display().to_string());
    let s = t.setup();
    let out = s
        .configure_mcp(
            &ids(&["codex", "claude-code", "opencode", "amp"]),
            &server(),
        )
        .unwrap();
    let paths: Vec<PathBuf> = out.iter().map(|o| o.path.clone()).collect();
    assert_eq!(
        paths,
        [
            codex_home.join("config.toml"),
            claude_dir.join(".claude.json"),
            xdg.join("opencode/opencode.json"),
            xdg.join("amp/settings.json"),
        ]
    );
    let skills = s
        .install_skills(&ids(&["claude-code"]), &ids(&["cua-spaces"]), false)
        .unwrap();
    assert_eq!(one(skills).path, claude_dir.join("skills/cua-spaces"));
    // CUA_HOME moves the state file.
    t.env.vars.insert(
        "CUA_HOME".into(),
        t.home.join("cuahome").display().to_string(),
    );
    assert_eq!(
        t.setup().state_path(),
        t.home.join("cuahome/agent-setup.json")
    );
}

#[test]
fn opencode_prefers_an_existing_jsonc_file() {
    let t = T::new();
    let file = t.write(".config/opencode/opencode.jsonc", "{\n  // keep\n}\n");
    let o = one(t
        .setup()
        .configure_mcp(&ids(&["opencode"]), &server())
        .unwrap());
    assert_eq!(o.path, file);
    assert!(t.read(&file).contains("// keep"));
    assert!(!t.home.join(".config/opencode/opencode.json").exists());
}

#[cfg(unix)]
#[test]
fn a_symlinked_config_is_edited_through_the_link() {
    let t = T::new();
    let real = t.write("dotfiles/mcp.json", "{\"mcpServers\": {}}");
    std::fs::create_dir_all(t.home.join(".cursor")).unwrap();
    let link = t.home.join(".cursor/mcp.json");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    t.setup()
        .configure_mcp(&ids(&["cursor"]), &server())
        .unwrap();
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(t.read(&real).contains("/opt/cua/bin/cua"));
}

#[test]
fn detection_uses_path_markers_and_app_bundles() {
    let mut t = T::new();
    let s = t.setup();
    assert!(
        s.detect().iter().all(|a| !a.installed),
        "empty home detects nothing"
    );
    assert_eq!(s.select("all").unwrap(), Vec::<String>::new());

    let bin = t.home.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let codex = bin.join(if cfg!(windows) { "codex.exe" } else { "codex" });
    std::fs::write(&codex, "#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();
        // A non-executable file is not a binary.
        std::fs::write(bin.join("claude"), "").unwrap();
    }
    t.env.path = vec![bin.clone()];
    std::fs::create_dir_all(t.home.join(".kiro")).unwrap();
    std::fs::create_dir_all(t.home.join("Applications/Cursor.app")).unwrap();
    let s = t.setup();
    let st = s.detect();
    let get = |id: &str| st.iter().find(|a| a.id == id).unwrap();
    assert!(get("codex").installed);
    assert_eq!(get("codex").evidence, [format!("bin:{}", codex.display())]);
    assert!(get("kiro").installed);
    assert!(!get("claude-code").installed);
    assert_eq!(get("cursor").installed, cfg!(target_os = "macos"));
    assert!(get("pi").mcp_config.is_none(), "Pi has no MCP");
    let mut all = s.select("all").unwrap();
    all.sort();
    let mut want = vec!["codex".to_string(), "kiro".to_string()];
    if cfg!(target_os = "macos") {
        want.push("cursor".into());
    }
    want.sort();
    assert_eq!(all, want);
    assert_eq!(
        s.select("claude, openai-codex,claude").unwrap(),
        ["claude-code", "codex"]
    );
    assert!(
        s.select("nope")
            .unwrap_err()
            .to_string()
            .contains("unknown agent")
    );
    assert!(s.select("none").unwrap().is_empty());
}

#[test]
fn skills_install_once_per_shared_dir_and_remove_only_when_unused() {
    let t = T::new();
    let s = t.setup();
    let bundled = s.bundled_skills();
    let n = bundled.len();
    assert!(n >= 4);
    let agents = ids(&["codex", "gemini-cli", "claude-code", "pi"]);
    let out = s.install_skills(&agents, &[], false).unwrap();
    // ~/.agents/skills (codex, gemini, pi) + ~/.claude/skills.
    assert_eq!(out.len(), 2 * n, "{out:#?}");
    assert!(
        out.iter()
            .all(|o| o.change == Change::Created && o.target == Target::Skill)
    );
    let shared = t.home.join(".agents/skills");
    for sk in &bundled {
        assert!(shared.join(&sk.name).join("SKILL.md").is_file());
        assert!(
            t.home
                .join(".claude/skills")
                .join(&sk.name)
                .join("SKILL.md")
                .is_file()
        );
    }
    let st = s.detect();
    let codex = st.iter().find(|a| a.id == "codex").unwrap();
    assert_eq!(codex.skills_installed.len(), n);
    assert!(codex.skills_outdated.is_empty());

    // Idempotent.
    let again = s.install_skills(&agents, &[], false).unwrap();
    assert!(again.iter().all(|o| o.change == Change::Unchanged));

    // Removing codex keeps the shared copies for gemini and pi.
    let out = s.remove(&ids(&["codex"]), Parts::ALL).unwrap();
    assert!(
        out.iter()
            .filter(|o| o.target == Target::Skill)
            .all(|o| o.change == Change::Skipped && o.detail.starts_with("kept for"))
    );
    assert!(shared.join("cua-driver").is_dir());

    // A user edit survives removal.
    std::fs::write(shared.join("cua-spaces/NOTES.md"), "mine").unwrap();
    s.remove(&ids(&["gemini-cli", "pi"]), Parts::ALL).unwrap();
    assert!(!shared.join("cua-driver").exists());
    assert!(shared.join("cua-spaces/NOTES.md").is_file());
    // Claude's copies are untouched by removing other agents.
    assert!(t.home.join(".claude/skills/cua-driver").is_dir());
}

#[test]
fn agents_that_also_read_claude_skills_are_served_from_there() {
    let t = T::new();
    let s = t.setup();
    let n = s.bundled_skills().len();
    let claude = t.home.join(".claude/skills");
    let shared = t.home.join(".agents/skills");

    // Claude Code + Cursor: one copy, in ~/.claude/skills (Cursor loads it).
    let out = s
        .install_skills(&ids(&["claude-code", "cursor"]), &[], false)
        .unwrap();
    assert_eq!(out.len(), n, "{out:#?}");
    assert!(
        out.iter()
            .all(|o| o.path.starts_with(&claude) && o.agents == ids(&["claude-code", "cursor"]))
    );
    assert!(!shared.exists(), "no second copy for Cursor to list twice");
    let st = s.detect();
    let cursor = st.iter().find(|a| a.id == "cursor").unwrap();
    assert_eq!(cursor.skills_installed.len(), n);

    // Later, OpenCode alone: already served by cua's ~/.claude/skills copies.
    let out = s.install_skills(&ids(&["opencode"]), &[], false).unwrap();
    assert!(
        out.iter()
            .all(|o| o.path.starts_with(&claude) && o.change == Change::Unchanged)
    );
    assert!(!shared.exists());

    // Codex does not read ~/.claude/skills: it gets the shared directory.
    s.install_skills(&ids(&["codex"]), &[], false).unwrap();
    assert!(shared.join("cua-driver/SKILL.md").is_file());

    // Removing Claude Code keeps the copies Cursor and OpenCode use.
    s.remove(&ids(&["claude-code"]), Parts::ALL).unwrap();
    assert!(claude.join("cua-driver").is_dir());
    s.remove(&ids(&["cursor", "opencode"]), Parts::ALL).unwrap();
    assert!(!claude.join("cua-driver").exists());
}

#[test]
fn stale_spaces_python_mcp_entries_are_removed_and_others_kept() {
    let t = T::new();
    let s = t.setup();
    let cursor = t.write(
        ".cursor/mcp.json",
        r#"{"mcpServers": {"cua-spaces": {"command": "python3", "args": ["/x/apps/cua-spaces/mcp/spaces_mcp.py"]}, "other": {"command": "o"}}}"#,
    );
    let gemini = t.write(
        ".gemini/settings.json",
        r#"{"mcpServers": {"cua-spaces": {"command": "my-own-spaces-server"}}}"#,
    );
    let out = s
        .configure_mcp(&ids(&["cursor", "gemini-cli"]), &server())
        .unwrap();
    let removed: Vec<&Outcome> = out.iter().filter(|o| o.item == "cua-spaces").collect();
    assert_eq!(removed.len(), 1, "{out:#?}");
    assert_eq!(removed[0].change, Change::Removed);
    assert_eq!(removed[0].agents, ids(&["cursor"]));
    let c: Value = serde_json::from_str(&t.read(&cursor)).unwrap();
    assert!(c["mcpServers"].get("cua-spaces").is_none());
    assert_eq!(c["mcpServers"]["other"]["command"], "o");
    assert!(c["mcpServers"]["cua"].is_object());
    let g: Value = serde_json::from_str(&t.read(&gemini)).unwrap();
    assert_eq!(
        g["mcpServers"]["cua-spaces"]["command"],
        "my-own-spaces-server"
    );
    // Idempotent: nothing more to remove.
    let again = s.configure_mcp(&ids(&["cursor"]), &server()).unwrap();
    assert!(again.iter().all(|o| o.item != "cua-spaces"));
}

#[test]
fn foreign_skill_folders_are_skipped_unless_forced() {
    let t = T::new();
    let mine = t.write(
        ".kiro/skills/cua-driver/SKILL.md",
        "---\nname: cua-driver\ndescription: my fork\n---\n",
    );
    let s = t.setup();
    let o = one(s
        .install_skills(&ids(&["kiro"]), &ids(&["cua-driver"]), false)
        .unwrap());
    assert_eq!(o.change, Change::Skipped);
    assert!(o.detail.contains("not installed by cua"), "{}", o.detail);
    assert!(t.read(&mine).contains("my fork"));
    let o = one(s
        .install_skills(&ids(&["kiro"]), &ids(&["cua-driver"]), true)
        .unwrap());
    assert_eq!(o.change, Change::Updated);
    let bak = o.backup.unwrap();
    assert!(t.read(&bak.join("SKILL.md")).contains("my fork"));
    assert!(!t.read(&mine).contains("my fork"));
    assert!(
        s.install_skills(&ids(&["kiro"]), &ids(&["nope"]), false)
            .is_err()
    );
}

#[test]
fn update_refreshes_outdated_copies_cua_wrote() {
    let t = T::new();
    let s = t.setup();
    s.install_skills(&ids(&["cline"]), &ids(&["cua-sandboxes"]), false)
        .unwrap();
    let dest = t.home.join(".cline/skills/cua-sandboxes");
    // Pretend an older cua wrote an older version: change the content and
    // record its hash as the one cua wrote.
    std::fs::write(
        dest.join("SKILL.md"),
        "---\nname: cua-sandboxes\ndescription: old\n---\n",
    )
    .unwrap();
    let old_hash = cua_agent_setup::hash_dir(&dest).unwrap();
    let state_path = s.state_path().to_path_buf();
    let mut state: Value = serde_json::from_str(&t.read(&state_path)).unwrap();
    state["skills"][dest.display().to_string()]["hash"] = json!(old_hash);
    std::fs::write(&state_path, state.to_string()).unwrap();
    let st = s.detect().into_iter().find(|a| a.id == "cline").unwrap();
    assert_eq!(st.skills_outdated, ["cua-sandboxes"]);

    let out = s.update(None).unwrap();
    assert_eq!(one(out).change, Change::Updated);
    assert!(!t.read(&dest.join("SKILL.md")).contains("description: old"));

    // MCP: update re-points managed entries to the new command.
    s.configure_mcp(&ids(&["cline"]), &server()).unwrap();
    let out = s.update(Some(&McpServer::new("/new/cua"))).unwrap();
    assert!(
        out.iter()
            .any(|o| o.target == Target::Mcp && o.change == Change::Updated),
        "{out:#?}"
    );
}

#[cfg(unix)]
#[test]
fn claude_code_is_configured_through_its_own_cli_when_present() {
    use std::os::unix::fs::PermissionsExt;
    let mut t = T::new();
    let bin = t.home.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = t.home.join("claude-argv.log");
    let config = t.home.join(".claude.json");
    let entry = "{\"mcpServers\":{\"cua\":{\"type\":\"stdio\",\"command\":\"/opt/cua/bin/cua\",\"args\":[\"mcp\"]}}}";
    // The fake writes only inside the temp home (absolute paths baked in).
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\ncase \"$2\" in\n  add-json) printf '%s' '{}' > '{}' ;;\n  remove) printf '%s' '{{\"mcpServers\":{{}}}}' > '{}' ;;\nesac\n",
        log.display(),
        entry,
        config.display(),
        config.display()
    );
    let claude = bin.join("claude");
    std::fs::write(&claude, script).unwrap();
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    t.env.path = vec![bin];
    t.env.run_agent_clis = true;
    let s = t.setup();
    let o = one(s.configure_mcp(&ids(&["claude"]), &server()).unwrap());
    assert_eq!(o.change, Change::Created, "{o:?}");
    assert!(o.detail.contains("claude mcp add-json"));
    let argv = t.read(&log);
    assert!(
        argv.starts_with("mcp add-json --scope user cua {\"type\":\"stdio\",\"command\":\"/opt/cua/bin/cua\",\"args\":[\"mcp\"]}"),
        "{argv}"
    );
    let o = one(s
        .remove(
            &ids(&["claude"]),
            Parts {
                skills: false,
                mcp: true,
            },
        )
        .unwrap());
    assert_eq!(o.change, Change::Removed, "{o:?}");
    assert!(t.read(&log).contains("mcp remove --scope user cua"));
}

#[test]
fn state_file_records_what_cua_wrote() {
    let t = T::new();
    let s = t.setup();
    s.configure_mcp(&ids(&["kiro"]), &server()).unwrap();
    s.install_skills(&ids(&["kiro"]), &ids(&["cua-spaces"]), false)
        .unwrap();
    let state: Value = serde_json::from_str(&t.read(s.state_path())).unwrap();
    assert_eq!(state["version"], 1);
    let file = t.home.join(".kiro/settings/mcp.json").display().to_string();
    assert_eq!(state["mcp"][&file]["value"]["command"], "/opt/cua/bin/cua");
    assert_eq!(state["mcp"][&file]["agents"], json!(["kiro"]));
    let dir = t.home.join(".kiro/skills/cua-spaces").display().to_string();
    assert_eq!(state["skills"][&dir]["skill"], "cua-spaces");
    // Pi supports skills only; MCP is skipped with a reason.
    let o = one(s.configure_mcp(&ids(&["pi"]), &server()).unwrap());
    assert_eq!(o.change, Change::Skipped);
    assert!(o.detail.contains("does not support MCP"));
}

#[test]
fn cua_driver_setup_adds_its_skill_and_server_beside_cua_and_removes_both() {
    let t = T::new();
    let (rel, text) = fixture("claude-code");
    let file = t.write(rel, &text);
    let s = t.setup();
    let agents = ids(&["claude-code", "codex", "pi"]);
    s.configure_mcp(&ids(&["claude-code"]), &server()).unwrap();

    let driver = McpServer::driver("/opt/drv/bin/cua-driver");
    assert_eq!(driver.name, cua_agent_setup::DRIVER_MCP_SERVER_NAME);
    assert_eq!(driver.args, ["mcp"]);
    let out = s
        .setup_cua_driver(&agents, &driver, Parts::ALL, false)
        .unwrap();
    // Only the cua-driver skill, one copy per skills directory.
    let skills: Vec<&Outcome> = out.iter().filter(|o| o.target == Target::Skill).collect();
    assert!(
        skills
            .iter()
            .filter(|o| !o.item.is_empty())
            .all(|o| o.item == "cua-driver" && o.change == Change::Created),
        "{skills:#?}"
    );
    assert!(t.home.join(".claude/skills/cua-driver/SKILL.md").is_file());
    assert!(!t.home.join(".claude/skills/cua-spaces").exists());
    // The MCP server for the agents that have MCP (Pi has none: no row).
    let mcp: Vec<&Outcome> = out.iter().filter(|o| o.target == Target::Mcp).collect();
    assert_eq!(mcp.len(), 2, "{mcp:#?}");
    assert!(
        mcp.iter()
            .all(|o| o.item == "cua-driver" && o.change == Change::Created)
    );
    let after = t.read(&file);
    let m = registry::find("claude-code").unwrap().mcp.unwrap();
    let entry = cua_agent_setup_get(m.format, &file, &after, m.key_path, "cua-driver").unwrap();
    assert_eq!(entry["command"], "/opt/drv/bin/cua-driver");
    assert!(cua_agent_setup_get(m.format, &file, &after, m.key_path, "cua").is_some());

    // Status sees both; idempotent.
    let st = s
        .detect()
        .into_iter()
        .find(|x| x.id == "claude-code")
        .unwrap();
    assert!(
        st.cua_configured && st.cua_managed && st.cua_driver_configured,
        "{st:?}"
    );
    let again = s
        .setup_cua_driver(&agents, &driver, Parts::ALL, false)
        .unwrap();
    assert!(
        again.iter().all(|o| o.change == Change::Unchanged),
        "{again:#?}"
    );

    // The state keeps the two servers apart.
    let state: Value = serde_json::from_str(&t.read(s.state_path())).unwrap();
    let key = file.display().to_string();
    assert_eq!(state["mcp"][&key]["name"], "cua");
    assert_eq!(
        state["servers"]["cua-driver"][&key]["value"]["command"],
        "/opt/drv/bin/cua-driver"
    );

    // Remove undoes both entries and the skill; the unrelated server stays.
    let removed = s
        .remove(&ids(&["claude-code", "codex"]), Parts::ALL)
        .unwrap();
    let names: Vec<(&str, Change)> = removed
        .iter()
        .filter(|o| o.target == Target::Mcp && o.agents == ["claude-code"])
        .map(|o| (o.item.as_str(), o.change))
        .collect();
    assert_eq!(
        names,
        [("cua", Change::Removed), ("cua-driver", Change::Removed)]
    );
    let left = t.read(&file);
    assert!(cua_agent_setup_get(m.format, &file, &left, m.key_path, "cua-driver").is_none());
    assert!(cua_agent_setup_get(m.format, &file, &left, m.key_path, "other").is_some());
    let state: Value = serde_json::from_str(&t.read(s.state_path())).unwrap();
    assert!(state.get("servers").is_none(), "{state}");

    // A misnamed server is refused.
    assert!(
        s.setup_cua_driver(&agents, &server(), Parts::ALL, false)
            .is_err()
    );
}

#[test]
fn the_driver_command_resolves_from_path_then_local_bin() {
    let t = T::new();
    assert_eq!(McpServer::driver_for(&t.env).command, "cua-driver");
    let bin = t.home.join(".local/bin");
    std::fs::create_dir_all(&bin).unwrap();
    let exe = bin.join(if cfg!(windows) {
        "cua-driver.exe"
    } else {
        "cua-driver"
    });
    std::fs::write(&exe, "#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    assert_eq!(
        McpServer::driver_for(&t.env).command,
        exe.display().to_string()
    );
    let mut env = t.env.clone();
    let other = t.home.join("opt");
    std::fs::create_dir_all(&other).unwrap();
    let on_path = other.join(exe.file_name().unwrap());
    std::fs::copy(&exe, &on_path).unwrap();
    env.path = vec![other];
    assert_eq!(
        McpServer::driver_for(&env).command,
        on_path.display().to_string()
    );
}

#[test]
fn hermes_is_detected_from_its_bin_home_or_app() {
    let mut t = T::new();
    let get = |s: &AgentSetup| s.detect().into_iter().find(|a| a.id == "hermes").unwrap();
    let st = get(&t.setup());
    assert!(!st.installed, "not installed in an empty home: {st:?}");
    assert_eq!(st.name, "Hermes");
    assert_eq!(st.mcp_format, Some(Format::Yaml));
    assert_eq!(
        st.mcp_config.as_deref(),
        Some(t.home.join(hermes_rel("config.yaml")).as_path())
    );
    assert_eq!(
        st.skills_dir.as_deref(),
        Some(t.home.join(hermes_rel("skills")).as_path())
    );
    assert_eq!(registry::find("hermes-agent").unwrap().id, "hermes");

    // The `hermes` launcher on PATH (the installer's ~/.local/bin/hermes).
    let bin = t.home.join(".local/bin");
    std::fs::create_dir_all(&bin).unwrap();
    let exe = bin.join(if cfg!(windows) {
        "hermes.exe"
    } else {
        "hermes"
    });
    std::fs::write(&exe, "#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    t.env.path = vec![bin];
    let st = get(&t.setup());
    assert_eq!(st.evidence, [format!("bin:{}", exe.display())]);
    t.env.path.clear();

    // The Hermes home.
    std::fs::create_dir_all(t.home.join(hermes_rel(""))).unwrap();
    let st = get(&t.setup());
    assert!(
        st.installed && st.evidence[0].starts_with("path:"),
        "{st:?}"
    );
    std::fs::remove_dir_all(t.home.join(hermes_rel(""))).unwrap();

    // Hermes.app (macOS only).
    std::fs::create_dir_all(t.home.join("Applications/Hermes.app")).unwrap();
    assert_eq!(get(&t.setup()).installed, cfg!(target_os = "macos"));
}

#[test]
fn hermes_follows_hermes_home_and_the_windows_default() {
    let mut t = T::new();
    let hh = t.home.join("profiles/work");
    t.env
        .vars
        .insert("HERMES_HOME".into(), hh.display().to_string());
    let s = t.setup();
    let o = one(s.configure_mcp(&ids(&["hermes"]), &server()).unwrap());
    assert_eq!(o.path, hh.join("config.yaml"));
    assert_eq!(o.change, Change::Created);
    let skills = s
        .install_skills(&ids(&["hermes"]), &ids(&["cua-spaces"]), false)
        .unwrap();
    assert_eq!(one(skills).path, hh.join("skills/cua-spaces"));
    let st = s.detect().into_iter().find(|a| a.id == "hermes").unwrap();
    assert!(st.installed, "HERMES_HOME exists now: {st:?}");

    // Windows: %LOCALAPPDATA%\hermes, else ~/AppData/Local/hermes.
    let spec = registry::find("hermes").unwrap();
    let mut win = HostEnv::isolated("/u");
    win.os = cua_agent_setup::Os::Windows;
    let mcp = spec.mcp.unwrap().file;
    assert_eq!(
        mcp.resolve(&win),
        Path::new("/u/AppData/Local/hermes/config.yaml")
    );
    win.vars.insert("LOCALAPPDATA".into(), "/l".into());
    assert_eq!(mcp.resolve(&win), Path::new("/l/hermes/config.yaml"));
    assert_eq!(
        spec.skills.unwrap().resolve(&win),
        Path::new("/l/hermes/skills")
    );
    win.vars.insert("HERMES_HOME".into(), "/h".into());
    assert_eq!(mcp.resolve(&win), Path::new("/h/config.yaml"));
    // Other OSes ignore LOCALAPPDATA.
    let mut linux = HostEnv::isolated("/u");
    linux.os = cua_agent_setup::Os::Linux;
    linux.vars.insert("LOCALAPPDATA".into(), "/l".into());
    assert_eq!(mcp.resolve(&linux), Path::new("/u/.hermes/config.yaml"));
}

#[test]
fn hermes_mcp_update_and_remove_touch_only_cuas_entry() {
    let t = T::new();
    let file = t.write(hermes_rel("config.yaml"), HERMES_CONFIG);
    let s = t.setup();
    let srv = server().with_env("CUA_TELEMETRY", "0");
    let o = one(s.configure_mcp(&ids(&["hermes"]), &srv).unwrap());
    assert_eq!(o.change, Change::Created, "{o:?}");
    let after = t.read(&file);
    // Appended to the live mcp_servers block, everything else as it was.
    assert_eq!(
        after,
        HERMES_CONFIG.replace(
            "    args: []\n\napprovals:",
            "    args: []\n  cua:\n    command: /opt/cua/bin/cua\n    args:\n      - mcp\n    env:\n      CUA_TELEMETRY: '0'\n\napprovals:"
        )
    );
    let parsed: Value = serde_yaml_ng::from_str(&after).unwrap();
    assert_eq!(
        parsed["mcp_servers"]["cua"],
        json!({"command": "/opt/cua/bin/cua", "args": ["mcp"], "env": {"CUA_TELEMETRY": "0"}})
    );
    assert_eq!(parsed["approvals"]["mode"], "smart");

    // The user adds a server after cua's; update re-points only cua's.
    let edited = after.replace(
        "\napprovals:",
        "  notion:\n    url: https://mcp.notion.com/mcp\n\napprovals:",
    );
    std::fs::write(&file, &edited).unwrap();
    let out = s.update(Some(&McpServer::new("/new/bin/cua"))).unwrap();
    let o = out.iter().find(|o| o.agents == ["hermes"]).unwrap();
    assert_eq!(o.change, Change::Updated, "{o:?}");
    let updated = t.read(&file);
    assert!(
        updated.contains("  cua:\n    command: /new/bin/cua\n    args:\n      - mcp\n  notion:"),
        "{updated}"
    );
    assert!(updated.contains("command: other-mcp  # mine"));
    assert!(updated.starts_with("# Hermes Agent configuration\n"));

    // Remove: cua's entry goes; the user's servers and comments stay.
    let o = one(s
        .remove(
            &ids(&["hermes"]),
            Parts {
                skills: false,
                mcp: true,
            },
        )
        .unwrap());
    assert_eq!(o.change, Change::Removed, "{o:?}");
    assert_eq!(
        t.read(&file),
        HERMES_CONFIG.replace(
            "\napprovals:",
            "  notion:\n    url: https://mcp.notion.com/mcp\n\napprovals:"
        )
    );

    // An entry the user changed after cua wrote it is left alone.
    s.configure_mcp(&ids(&["hermes"]), &server()).unwrap();
    let mine = t.read(&file).replace(
        "      - mcp\n",
        "      - mcp\n      - --sandbox\n      - dev\n",
    );
    std::fs::write(&file, &mine).unwrap();
    assert!(
        s.update(Some(&McpServer::new("/x/cua")))
            .unwrap()
            .iter()
            .all(|o| o.target != Target::Mcp)
    );
    let o = one(s.remove(&ids(&["hermes"]), Parts::ALL).unwrap());
    assert!(o.detail.contains("changed since cua wrote it"), "{o:?}");
    assert_eq!(t.read(&file), mine);
}

#[test]
fn hermes_gets_its_own_skill_copies() {
    let t = T::new();
    let s = t.setup();
    let n = s.bundled_skills().len();
    let out = s
        .install_skills(&ids(&["hermes", "codex"]), &[], false)
        .unwrap();
    // ~/.hermes/skills (Hermes reads ~/.agents/skills only when configured
    // as an external dir) + ~/.agents/skills.
    assert_eq!(out.len(), 2 * n, "{out:#?}");
    let dir = t.home.join(hermes_rel("skills"));
    for sk in s.bundled_skills() {
        let md = std::fs::read_to_string(dir.join(&sk.name).join("SKILL.md")).unwrap();
        // Hermes requires agentskills.io front matter: name and description.
        assert!(md.starts_with("---\n"), "{}", sk.name);
        assert!(
            md.contains(&format!("\nname: {}\n", sk.name)),
            "{}",
            sk.name
        );
        assert!(md.contains("\ndescription:"), "{}", sk.name);
    }
    let st = s.detect().into_iter().find(|a| a.id == "hermes").unwrap();
    assert_eq!(st.skills_installed.len(), n);
    // Removing Hermes deletes only its copies.
    let out = s.remove(&ids(&["hermes"]), Parts::ALL).unwrap();
    assert!(
        out.iter()
            .filter(|o| o.target == Target::Skill)
            .all(|o| o.change == Change::Removed)
    );
    assert!(!dir.join("cua-spaces").exists());
    assert!(t.home.join(".agents/skills/cua-spaces/SKILL.md").is_file());
}

#[test]
fn the_docs_and_readme_list_every_agent() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let docs = std::fs::read_to_string(
        root.join("../../../../docs/content/docs/cua-cli/guides/mcp-server.mdx"),
    )
    .unwrap();
    let readme = std::fs::read_to_string(root.join("README.md")).unwrap();
    for a in AGENTS {
        assert!(
            docs.contains(&format!(" | `{}` | ", a.id)),
            "docs/content/docs/cua-cli/guides/mcp-server.mdx misses {}",
            a.id
        );
        assert!(
            readme.contains(&format!("\n| {} | ", a.name)),
            "README.md misses {}",
            a.name
        );
    }
}
