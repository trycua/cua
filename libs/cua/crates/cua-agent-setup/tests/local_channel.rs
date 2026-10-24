//! Cua Spaces Local's agent setup beside Stable's, in one fake HOME (CUA-1776).
//!
//! The Local app's "Continue" on the welcome screen runs this engine. It must
//! register its own MCP server (`cua-local`, launching `cua-local`) and install
//! its own skill folders (`<name>-local`), keep its own state under
//! `~/.cua-local`, and leave everything Stable wrote byte for byte as it was.
//! Always in a temporary HOME, never the real one.

use cua_agent_setup::{
    AgentSetup, Change, CuaChannel, HostEnv, McpServer, Outcome, Parts, Target, hash_dir,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const STABLE_CUA: &str = "/Applications/Cua Spaces.app/Contents/MacOS/cua";
const AGENTS: [&str; 3] = ["claude-code", "codex", "cursor"];

struct Fixture {
    _dir: tempfile::TempDir,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        // The user's own configs, with a server of theirs in each.
        write(
            &home,
            ".claude.json",
            "{\n  \"numStartups\": 12,\n  \"mcpServers\": {\n    \"other\": {\"type\": \"stdio\", \"command\": \"other-mcp\", \"args\": []}\n  }\n}\n",
        );
        write(
            &home,
            ".codex/config.toml",
            "# my codex config\n[mcp_servers.other]\ncommand = \"other-mcp\"\nargs = []\n",
        );
        write(
            &home,
            ".cursor/mcp.json",
            "{\n  \"mcpServers\": {\n    \"other\": {\"command\": \"other-mcp\"}\n  }\n}\n",
        );
        Fixture { _dir: dir, home }
    }

    fn env(&self, channel: CuaChannel) -> HostEnv {
        HostEnv::isolated(&self.home).with_channel(channel)
    }

    fn setup(&self, channel: CuaChannel) -> AgentSetup {
        AgentSetup::new(self.env(channel))
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.home.join(rel)).unwrap()
    }

    /// What only Stable owns after its setup: its state file and its skill
    /// folders (as hashes). The agents' configs are shared by both channels,
    /// each with its own entry, so they are checked entry by entry.
    fn stable_files(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        out.insert(
            ".cua/agent-setup.json".to_string(),
            self.read(".cua/agent-setup.json"),
        );
        for dir in [".claude/skills", ".agents/skills"] {
            let Ok(entries) = std::fs::read_dir(self.home.join(dir)) else {
                continue;
            };
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.ends_with("-local") {
                    continue;
                }
                out.insert(
                    format!("{dir}/{name}"),
                    hash_dir(&e.path()).unwrap_or_default(),
                );
            }
        }
        out
    }
}

fn write(home: &Path, rel: &str, text: &str) {
    let p = home.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn ids(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn items(out: &[Outcome], target: Target) -> Vec<String> {
    let mut v: Vec<String> = out
        .iter()
        .filter(|o| o.target == target)
        .map(|o| o.item.clone())
        .collect();
    v.sort();
    v.dedup();
    v
}

fn json(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

/// A `cua-local` the way install-local.sh and the first run leave it:
/// executable, in `~/.local/bin`.
fn install_cua_local(home: &Path) -> PathBuf {
    let p = home.join(".local/bin/cua-local");
    write(home, ".local/bin/cua-local", "#!/bin/sh\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    p
}

#[test]
fn local_registers_cua_local_and_skills_of_its_own_and_leaves_stable_untouched() {
    let f = Fixture::new();
    let stable = f.setup(CuaChannel::Stable);
    stable
        .configure_mcp(&ids(&AGENTS), &McpServer::new(STABLE_CUA))
        .unwrap();
    stable.install_skills(&ids(&AGENTS), &[], false).unwrap();
    let before = f.stable_files();
    assert!(
        before.contains_key(".claude/skills/cua-spaces"),
        "{before:?}"
    );

    // Local, with the same agents and the same configs.
    let cua_local = install_cua_local(&f.home);
    let mut env = f.env(CuaChannel::Local);
    env.path.push(f.home.join(".local/bin"));
    let local = AgentSetup::new(env.clone());

    // Stable's `cua` entry is not Local's: nothing reads as configured yet.
    for a in local
        .detect()
        .iter()
        .filter(|a| AGENTS.contains(&a.id.as_str()))
    {
        assert!(!a.cua_configured && !a.cua_managed, "{}: {a:?}", a.id);
        assert!(a.skills_installed.is_empty(), "{}: {a:?}", a.id);
    }

    let server = McpServer::default_for(&env);
    assert_eq!(server.name, "cua-local");
    assert_eq!(server.command, cua_local.display().to_string());
    assert_eq!(server.args, ["mcp"]);

    let mcp = local.configure_mcp(&ids(&AGENTS), &server).unwrap();
    assert!(
        mcp.iter()
            .all(|o| o.item == "cua-local" && o.change == Change::Created),
        "{mcp:#?}"
    );
    let skills = local.install_skills(&ids(&AGENTS), &[], false).unwrap();
    assert!(
        skills.iter().all(|o| o.change == Change::Created),
        "{skills:#?}"
    );
    let installed = items(&skills, Target::Skill);
    assert!(!installed.is_empty());
    assert!(
        installed.iter().all(|n| n.ends_with("-local")),
        "{installed:?}"
    );
    assert_eq!(
        installed,
        local
            .bundled_skills()
            .into_iter()
            .map(|s| s.name)
            .collect::<Vec<_>>()
    );

    // The configs hold both entries, each launching its own binary.
    let claude = json(&f.read(".claude.json"));
    assert_eq!(claude["mcpServers"]["cua"]["command"], STABLE_CUA);
    assert_eq!(
        claude["mcpServers"]["cua-local"]["command"],
        cua_local.display().to_string()
    );
    assert_eq!(claude["mcpServers"]["cua-local"]["args"][0], "mcp");
    assert_eq!(claude["mcpServers"]["other"]["command"], "other-mcp");
    let codex = f.read(".codex/config.toml");
    assert!(codex.contains("[mcp_servers.cua]") && codex.contains(STABLE_CUA));
    assert!(codex.contains("[mcp_servers.cua-local]"), "{codex}");
    assert!(codex.contains("# my codex config"));
    let cursor = json(&f.read(".cursor/mcp.json"));
    assert_eq!(cursor["mcpServers"]["cua"]["command"], STABLE_CUA);
    assert!(cursor["mcpServers"]["cua-local"].is_object());

    // Local's skills have their own folder and name in SKILL.md; Stable's
    // folders are what they were.
    let md = f.read(".claude/skills/cua-spaces-local/SKILL.md");
    assert!(md.contains("name: cua-spaces-local"), "{md}");
    assert!(md.contains("cua-local"));
    assert!(f.home.join(".claude/skills/cua-spaces/SKILL.md").is_file());
    assert_eq!(f.stable_files(), before, "Stable's files changed");

    // Local's state is its own; Stable's file was not written.
    assert!(f.home.join(".cua-local/agent-setup.json").is_file());
    let state = json(&f.read(".cua-local/agent-setup.json"));
    assert!(state["servers"]["cua-local"].is_object(), "{state}");
    assert!(state["mcp"].as_object().is_none_or(|m| m.is_empty()));

    // Both read as configured, each by its own entry and skills.
    for a in local
        .detect()
        .iter()
        .filter(|a| AGENTS.contains(&a.id.as_str()))
    {
        assert!(a.cua_configured && a.cua_managed, "{}: {a:?}", a.id);
        assert_eq!(a.skills_installed.len(), local.bundled_skills().len());
        assert!(a.skills_outdated.is_empty(), "{}: {a:?}", a.id);
    }
    for a in stable
        .detect()
        .iter()
        .filter(|a| AGENTS.contains(&a.id.as_str()))
    {
        assert!(a.cua_configured && a.cua_managed, "{}: {a:?}", a.id);
        assert!(
            a.skills_installed.iter().all(|n| !n.ends_with("-local")),
            "{}: {a:?}",
            a.id
        );
    }

    // Idempotent, and an update keeps to Local's own copies.
    let again = local.configure_mcp(&ids(&AGENTS), &server).unwrap();
    assert!(
        again.iter().all(|o| o.change == Change::Unchanged),
        "{again:#?}"
    );
    let again = local.install_skills(&ids(&AGENTS), &[], false).unwrap();
    assert!(again.iter().all(|o| o.change == Change::Unchanged));
    let updated = local.update(Some(&server)).unwrap();
    assert!(
        updated.iter().all(|o| o.change == Change::Unchanged),
        "{updated:#?}"
    );
    assert_eq!(f.stable_files(), before);

    // Removing Local's setup removes Local's pieces and only those.
    let removed = local.remove(&ids(&AGENTS), Parts::ALL).unwrap();
    let gone = |t: Target| items(&removed, t);
    assert_eq!(gone(Target::Mcp), ["cua-local"], "{removed:#?}");
    assert!(
        removed
            .iter()
            .filter(|o| o.target == Target::Mcp)
            .all(|o| o.change == Change::Removed),
        "{removed:#?}"
    );
    assert!(!f.home.join(".claude/skills/cua-spaces-local").exists());
    let claude = json(&f.read(".claude.json"));
    assert!(claude["mcpServers"]["cua-local"].is_null());
    assert_eq!(claude["mcpServers"]["cua"]["command"], STABLE_CUA);
    assert_eq!(f.stable_files(), before, "removing Local changed Stable");

    // And Stable still removes its own.
    let out = stable.remove(&ids(&AGENTS), Parts::ALL).unwrap();
    assert!(
        out.iter()
            .filter(|o| o.target == Target::Mcp)
            .all(|o| o.item == "cua" && o.change == Change::Removed),
        "{out:#?}"
    );
    assert!(json(&f.read(".claude.json"))["mcpServers"]["cua"].is_null());
}

#[test]
fn a_stable_cua_entry_is_not_taken_over_or_removed_by_local() {
    let f = Fixture::new();
    // Stable's setup has run, then Local's state is asked to remove: it never
    // wrote anything, so it removes nothing.
    f.setup(CuaChannel::Stable)
        .configure_mcp(&ids(&["claude-code"]), &McpServer::new(STABLE_CUA))
        .unwrap();
    let before = f.read(".claude.json");
    let out = f
        .setup(CuaChannel::Local)
        .remove(&ids(&["claude-code"]), Parts::ALL)
        .unwrap();
    assert!(out.iter().all(|o| o.change != Change::Removed), "{out:#?}");
    assert_eq!(f.read(".claude.json"), before);
}

#[test]
fn local_without_a_cua_local_on_the_path_names_the_bare_command() {
    let f = Fixture::new();
    let env = f.env(CuaChannel::Local);
    let server = McpServer::default_for(&env);
    assert_eq!(
        (server.name.as_str(), server.command.as_str()),
        ("cua-local", "cua-local")
    );
    // A Stable environment is unchanged: `cua mcp`, named `cua`.
    let server = McpServer::default_for(&f.env(CuaChannel::Stable));
    assert_eq!(
        (server.name.as_str(), server.command.as_str()),
        ("cua", "cua")
    );
}

#[test]
fn local_recognises_its_own_binary_in_the_app_bundle_and_not_stables() {
    let f = Fixture::new();
    let local = f.setup(CuaChannel::Local);
    let bundled = "/Users/a/Applications/Cua Spaces Local.app/Contents/MacOS/cua";
    local
        .configure_mcp(
            &ids(&["claude-code"]),
            &McpServer::for_channel(CuaChannel::Local, bundled),
        )
        .unwrap();
    let a = local
        .detect()
        .into_iter()
        .find(|a| a.id == "claude-code")
        .unwrap();
    assert!(a.cua_configured && a.cua_managed, "{a:?}");

    // A `cua-local` entry that launches Stable's binary is not Local's.
    let f = Fixture::new();
    let local = f.setup(CuaChannel::Local);
    local
        .configure_mcp(
            &ids(&["claude-code"]),
            &McpServer::for_channel(CuaChannel::Local, STABLE_CUA),
        )
        .unwrap();
    let a = local
        .detect()
        .into_iter()
        .find(|a| a.id == "claude-code")
        .unwrap();
    assert!(!a.cua_configured, "{a:?}");
}

#[test]
fn the_state_folder_follows_the_channel_unless_cua_home_is_set() {
    let f = Fixture::new();
    assert_eq!(f.env(CuaChannel::Stable).cua_home(), f.home.join(".cua"));
    assert_eq!(
        f.env(CuaChannel::Local).cua_home(),
        f.home.join(".cua-local")
    );
    let mut env = f.env(CuaChannel::Local);
    env.vars.insert(
        "CUA_HOME".into(),
        f.home.join("pinned").display().to_string(),
    );
    assert_eq!(env.cua_home(), f.home.join("pinned"));
}
