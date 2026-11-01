//! Removing Cua Spaces' agent entries never takes Cua Driver's with them.
//!
//! `cua agents remove` (and the uninstaller that runs it) removes the `cua`
//! MCP server (`cua-local` for Local) and the Cua Spaces skills. The
//! `cua-driver` server and the `cua-driver` skill are Cua Driver's: they stay,
//! as does everything of other tools, unless `include_cua_driver` asks. A
//! dry run returns exactly the rows a real run produces and writes nothing.
//! Configs in the formats of Claude Code (JSON), Codex (TOML) and Cursor
//! (JSON), in a temporary HOME, never the real one.

use cua_agent_setup::{
    AgentSetup, Change, CuaChannel, HostEnv, KEPT_CUA_DRIVER, McpServer, Outcome, Parts,
    RemoveOptions, Target, hash_dir, is_cua_driver_name,
};
use std::path::{Path, PathBuf};

const AGENTS: [&str; 3] = ["claude-code", "codex", "cursor"];

/// The Cua Spaces skills `cua agents setup` installs next to Cua Driver's.
const SPACES_SKILLS: [&str; 4] = [
    "cua-sandboxes",
    "cua-spaces",
    "cua-volume",
    "gui-automation",
];

struct Home {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

impl Home {
    /// A home whose agent configs hold one server of the user's own each.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("home");
        std::fs::create_dir_all(&path).unwrap();
        let h = Home { _dir: dir, path };
        h.write(
            ".claude.json",
            "{\n  \"numStartups\": 12,\n  \"mcpServers\": {\n    \"other\": {\"type\": \"stdio\", \"command\": \"other-mcp\", \"args\": []}\n  }\n}\n",
        );
        h.write(
            ".codex/config.toml",
            "# my codex config\n[mcp_servers.other]\ncommand = \"other-mcp\"\nargs = []\n",
        );
        h.write(
            ".cursor/mcp.json",
            "{\n  \"mcpServers\": {\n    \"other\": {\"command\": \"other-mcp\"}\n  }\n}\n",
        );
        h
    }

    fn write(&self, rel: &str, text: &str) {
        let p = self.path.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.path.join(rel)).unwrap()
    }

    fn setup(&self, channel: CuaChannel) -> AgentSetup {
        AgentSetup::new(HostEnv::isolated(&self.path).with_channel(channel))
    }

    /// Everything a remove could touch: the three configs, the state files
    /// and every skill folder (as tree hashes).
    fn snapshot(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for rel in [
            ".claude.json",
            ".codex/config.toml",
            ".cursor/mcp.json",
            ".cua/agent-setup.json",
            ".cua-local/agent-setup.json",
        ] {
            let text = std::fs::read_to_string(self.path.join(rel)).unwrap_or_default();
            out.push((rel.to_string(), text));
        }
        for dir in [".claude/skills", ".agents/skills"] {
            let Ok(entries) = std::fs::read_dir(self.path.join(dir)) else {
                continue;
            };
            for e in entries.flatten() {
                out.push((
                    format!("{dir}/{}", e.file_name().to_string_lossy()),
                    hash_dir(&e.path()).unwrap_or_default(),
                ));
            }
        }
        out.sort();
        out
    }

    fn skill_dir(&self, agent_dir: &str, name: &str) -> PathBuf {
        self.path.join(agent_dir).join(name)
    }
}

fn ids(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn driver() -> McpServer {
    McpServer::driver("/opt/drv/bin/cua-driver")
}

/// Cua Spaces' setup (default skills + the `cua` server) and Cua Driver's
/// (its skill + the `cua-driver` server) on all three agents.
fn set_up_both(h: &Home, channel: CuaChannel) -> AgentSetup {
    let s = h.setup(channel);
    s.install_skills(&ids(&AGENTS), &[], false).unwrap();
    let server = McpServer::for_channel(channel, "/opt/cua/bin/cua");
    s.configure_mcp(&ids(&AGENTS), &server).unwrap();
    s.setup_cua_driver(&ids(&AGENTS), &driver(), Parts::ALL, false)
        .unwrap();
    s
}

fn mcp_text(h: &Home, agent: &str) -> String {
    match agent {
        "claude-code" => h.read(".claude.json"),
        "codex" => h.read(".codex/config.toml"),
        "cursor" => h.read(".cursor/mcp.json"),
        other => panic!("{other}"),
    }
}

/// Whether the config of `agent` has an MCP server called `name`.
fn has_server(h: &Home, agent: &str, name: &str) -> bool {
    let text = mcp_text(h, agent);
    match agent {
        "codex" => text.contains(&format!("[mcp_servers.{name}]")),
        _ => {
            let v: serde_json::Value = serde_json::from_str(&text).unwrap();
            !v["mcpServers"][name].is_null()
        }
    }
}

fn rows(out: &[Outcome]) -> Vec<(Target, String, Change, PathBuf, Vec<String>)> {
    out.iter()
        .map(|o| {
            (
                o.target,
                o.item.clone(),
                o.change,
                o.path.clone(),
                o.agents.clone(),
            )
        })
        .collect()
}

fn items(out: &[Outcome], target: Target, change: Change) -> Vec<String> {
    let mut v: Vec<String> = out
        .iter()
        .filter(|o| o.target == target && o.change == change)
        .map(|o| o.item.clone())
        .collect();
    v.sort();
    v.dedup();
    v
}

#[test]
fn the_names_that_are_cua_drivers() {
    for n in ["cua-driver", "cua-driver-local", "cua-driver-rs"] {
        assert!(is_cua_driver_name(n), "{n}");
    }
    for n in [
        "cua",
        "cua-local",
        "cua-spaces",
        "cua-spaces-local",
        "cua-volume",
        "cua-drivers",
        "cua-driver2",
        "other",
        "",
    ] {
        assert!(!is_cua_driver_name(n), "{n}");
    }
}

#[test]
fn remove_takes_cua_spaces_entries_and_leaves_cua_driver_in_every_agent() {
    let h = Home::new();
    let s = set_up_both(&h, CuaChannel::Stable);
    for a in AGENTS {
        assert!(has_server(&h, a, "cua"), "{a}");
        assert!(has_server(&h, a, "cua-driver"), "{a}");
    }
    let driver_entry_before: Vec<String> = AGENTS.iter().map(|a| mcp_text(&h, a)).collect();

    let out = s.remove(&ids(&AGENTS), Parts::ALL).unwrap();

    for a in AGENTS {
        assert!(!has_server(&h, a, "cua"), "{a}: the cua server goes");
        assert!(has_server(&h, a, "cua-driver"), "{a}: cua-driver stays");
        assert!(has_server(&h, a, "other"), "{a}: the user's own stays");
    }
    // Only the `cua` entry left each config: nothing else changed in it.
    for (a, before) in AGENTS.iter().zip(&driver_entry_before) {
        let after = mcp_text(&h, a);
        assert!(after.len() < before.len(), "{a}");
        assert!(after.contains("/opt/drv/bin/cua-driver"), "{a}: {after}");
    }
    for dir in [".claude/skills", ".agents/skills"] {
        for name in SPACES_SKILLS {
            assert!(!h.skill_dir(dir, name).exists(), "{dir}/{name} stays");
        }
        assert!(
            h.skill_dir(dir, "cua-driver").join("SKILL.md").is_file(),
            "{dir}/cua-driver was removed"
        );
    }
    // What was removed is exactly the cua server and the Spaces skills.
    assert_eq!(items(&out, Target::Mcp, Change::Removed), ["cua"]);
    assert_eq!(
        items(&out, Target::Skill, Change::Removed),
        SPACES_SKILLS.map(String::from)
    );
    // And what stayed says so, per agent and skill folder.
    assert_eq!(items(&out, Target::Mcp, Change::Skipped), ["cua-driver"]);
    assert_eq!(items(&out, Target::Skill, Change::Skipped), ["cua-driver"]);
    assert!(
        out.iter()
            .filter(|o| o.item == "cua-driver")
            .all(|o| o.change == Change::Skipped && o.detail == KEPT_CUA_DRIVER),
        "{out:#?}"
    );
    assert!(out.iter().all(|o| !o.failed()), "{out:#?}");

    // Cua Driver's records survive in the state, so a later explicit remove
    // still finds them.
    let state = h.read(".cua/agent-setup.json");
    assert!(state.contains("\"servers\""), "{state}");
    assert!(state.contains("cua-driver"), "{state}");
}

#[test]
fn a_dry_run_lists_exactly_what_a_real_run_removes_and_changes_nothing() {
    for include_cua_driver in [false, true] {
        let h = Home::new();
        let s = set_up_both(&h, CuaChannel::Stable);
        let before = h.snapshot();

        let plan = s
            .remove_with(
                &ids(&AGENTS),
                Parts::ALL,
                RemoveOptions {
                    include_cua_driver,
                    dry_run: true,
                },
            )
            .unwrap();
        assert_eq!(h.snapshot(), before, "a dry run wrote something");
        assert!(
            plan.iter().any(|o| o.change == Change::Removed),
            "{plan:#?}"
        );

        let done = s
            .remove_with(
                &ids(&AGENTS),
                Parts::ALL,
                RemoveOptions {
                    include_cua_driver,
                    dry_run: false,
                },
            )
            .unwrap();
        assert_ne!(h.snapshot(), before, "a real run removed nothing");
        assert_eq!(rows(&plan), rows(&done), "include={include_cua_driver}");
        assert_eq!(
            plan.iter()
                .filter(|o| o.change == Change::Removed)
                .any(|o| o.item == "cua-driver"),
            include_cua_driver
        );
    }
}

#[test]
fn asking_for_cua_driver_removes_its_server_and_skill_too() {
    let h = Home::new();
    let s = set_up_both(&h, CuaChannel::Stable);
    let out = s
        .remove_with(
            &ids(&AGENTS),
            Parts::ALL,
            RemoveOptions {
                include_cua_driver: true,
                dry_run: false,
            },
        )
        .unwrap();
    for a in AGENTS {
        assert!(!has_server(&h, a, "cua"), "{a}");
        assert!(!has_server(&h, a, "cua-driver"), "{a}");
        assert!(has_server(&h, a, "other"), "{a}");
    }
    for dir in [".claude/skills", ".agents/skills"] {
        assert!(!h.skill_dir(dir, "cua-driver").exists(), "{dir}");
    }
    assert_eq!(
        items(&out, Target::Mcp, Change::Removed),
        ["cua", "cua-driver"]
    );
    assert!(
        out.iter()
            .all(|o| o.change != Change::Skipped || o.detail != KEPT_CUA_DRIVER)
    );
    // Codex's config is back to what the user had.
    assert_eq!(
        h.read(".codex/config.toml"),
        "# my codex config\n[mcp_servers.other]\ncommand = \"other-mcp\"\nargs = []\n"
    );
}

#[test]
fn the_mcp_only_and_skills_only_parts_keep_cua_driver_too() {
    let h = Home::new();
    let s = set_up_both(&h, CuaChannel::Stable);
    s.remove(
        &ids(&AGENTS),
        Parts {
            skills: false,
            mcp: true,
        },
    )
    .unwrap();
    for a in AGENTS {
        assert!(!has_server(&h, a, "cua"), "{a}");
        assert!(has_server(&h, a, "cua-driver"), "{a}");
    }
    assert!(h.skill_dir(".claude/skills", "cua-spaces").is_dir());

    s.remove(
        &ids(&AGENTS),
        Parts {
            skills: true,
            mcp: false,
        },
    )
    .unwrap();
    assert!(!h.skill_dir(".claude/skills", "cua-spaces").exists());
    assert!(h.skill_dir(".claude/skills", "cua-driver").is_dir());
    for a in AGENTS {
        assert!(has_server(&h, a, "cua-driver"), "{a}");
    }
}

#[test]
fn cua_driver_entries_cua_never_wrote_are_never_touched() {
    let h = Home::new();
    // Added by the driver's own installer, not by cua: no state record.
    h.write(
        ".claude.json",
        "{\n  \"mcpServers\": {\n    \"cua-driver\": {\"type\": \"stdio\", \"command\": \"/Applications/CuaDriver.app/Contents/MacOS/cua-driver\", \"args\": [\"mcp\"]},\n    \"cua-driver-local\": {\"type\": \"stdio\", \"command\": \"cua-driver-local\", \"args\": [\"mcp\"]}\n  }\n}\n",
    );
    let s = h.setup(CuaChannel::Stable);
    s.configure_mcp(&ids(&["claude-code"]), &McpServer::new("/opt/cua/bin/cua"))
        .unwrap();
    for include_cua_driver in [false, true] {
        let out = s
            .remove_with(
                &ids(&["claude-code"]),
                Parts::ALL,
                RemoveOptions {
                    include_cua_driver,
                    dry_run: false,
                },
            )
            .unwrap();
        assert!(
            out.iter()
                .all(|o| !is_cua_driver_name(&o.item) || o.change != Change::Removed),
            "{out:#?}"
        );
        assert!(has_server(&h, "claude-code", "cua-driver"));
        assert!(has_server(&h, "claude-code", "cua-driver-local"));
    }
    assert!(!has_server(&h, "claude-code", "cua"));
}

#[test]
fn local_removes_cua_local_and_leaves_cua_driver_and_cua_driver_local() {
    let h = Home::new();
    let local = h.setup(CuaChannel::Local);
    local.install_skills(&ids(&AGENTS), &[], false).unwrap();
    local
        .configure_mcp(
            &ids(&AGENTS),
            &McpServer::for_channel(CuaChannel::Local, "/opt/cua/bin/cua-local"),
        )
        .unwrap();
    local
        .setup_cua_driver(&ids(&AGENTS), &driver(), Parts::ALL, false)
        .unwrap();
    for a in AGENTS {
        assert!(has_server(&h, a, "cua-local"), "{a}");
        assert!(has_server(&h, a, "cua-driver"), "{a}");
    }
    assert!(h.skill_dir(".claude/skills", "cua-driver-local").is_dir());

    let plan = local
        .remove_with(
            &ids(&AGENTS),
            Parts::ALL,
            RemoveOptions {
                include_cua_driver: false,
                dry_run: true,
            },
        )
        .unwrap();
    assert_eq!(items(&plan, Target::Mcp, Change::Removed), ["cua-local"]);
    assert!(
        !items(&plan, Target::Skill, Change::Removed).contains(&"cua-driver-local".to_string()),
        "{plan:#?}"
    );

    let out = local.remove(&ids(&AGENTS), Parts::ALL).unwrap();
    assert_eq!(rows(&plan), rows(&out));
    for a in AGENTS {
        assert!(!has_server(&h, a, "cua-local"), "{a}");
        assert!(has_server(&h, a, "cua-driver"), "{a}");
        assert!(has_server(&h, a, "other"), "{a}");
    }
    assert!(!h.skill_dir(".claude/skills", "cua-spaces-local").exists());
    assert!(
        h.skill_dir(".claude/skills", "cua-driver-local")
            .join("SKILL.md")
            .is_file(),
        "Cua Driver's Local skill was removed"
    );
    // The Local skill is Cua Driver's: it goes only when asked.
    let out = local
        .remove_with(
            &ids(&AGENTS),
            Parts::ALL,
            RemoveOptions {
                include_cua_driver: true,
                dry_run: false,
            },
        )
        .unwrap();
    assert!(
        items(&out, Target::Skill, Change::Removed).contains(&"cua-driver-local".to_string()),
        "{out:#?}"
    );
    assert!(!h.skill_dir(".claude/skills", "cua-driver-local").exists());
}

#[test]
fn a_dry_run_on_a_clean_home_has_nothing_to_remove() {
    let h = Home::new();
    let before = h.snapshot();
    let out = h
        .setup(CuaChannel::Stable)
        .remove_with(
            &ids(&AGENTS),
            Parts::ALL,
            RemoveOptions {
                include_cua_driver: true,
                dry_run: true,
            },
        )
        .unwrap();
    assert!(out.iter().all(|o| o.change == Change::Skipped), "{out:#?}");
    assert_eq!(h.snapshot(), before);
    assert!(!Path::new(&h.path.join(".cua/agent-setup.json")).exists());
}

/// A dry run works the edit out as a real run does: a managed entry the format
/// cannot be edited around without losing the user's comments (a flow-style
/// YAML mapping beside comments) fails in both, and the file is not touched.
#[test]
fn a_dry_run_reports_the_same_failure_as_the_real_run_for_an_edit_the_format_refuses() {
    let h = Home::new();
    let config = ".hermes/config.yaml";
    h.write(
        config,
        "# my hermes config\nmcp_servers:\n  other:\n    command: other-mcp  # mine\n    args: []\n",
    );
    let s = h.setup(CuaChannel::Stable);
    s.configure_mcp(&ids(&["hermes"]), &McpServer::new("/opt/cua/bin/cua"))
        .unwrap();
    // The user tidies the whole mapping into one flow-style line, keeping the
    // entry's value and the comment.
    let written = h.read(config);
    assert!(
        written.contains("  cua:\n    command: /opt/cua/bin/cua\n    args:\n      - mcp\n"),
        "{written}"
    );
    let reflowed = "# my hermes config\nmcp_servers: {other: {command: other-mcp, args: []}, cua: {command: /opt/cua/bin/cua, args: [mcp]}}\n".to_string();
    h.write(config, &reflowed);
    let state = h.read(".cua/agent-setup.json");

    let plan = s
        .remove_with(
            &ids(&["hermes"]),
            Parts::ALL,
            RemoveOptions {
                include_cua_driver: false,
                dry_run: true,
            },
        )
        .unwrap();
    assert_eq!(h.read(config), reflowed);
    assert_eq!(h.read(".cua/agent-setup.json"), state);
    let real = s.remove(&ids(&["hermes"]), Parts::ALL).unwrap();
    assert_eq!(h.read(config), reflowed, "the real run left the file alone");
    assert_eq!(rows(&plan), rows(&real));
    let cua = |out: &[Outcome]| {
        out.iter()
            .find(|o| o.target == Target::Mcp && o.item == "cua")
            .cloned()
            .unwrap()
    };
    assert_eq!(cua(&real).change, Change::Failed, "{real:#?}");
    assert_eq!(cua(&plan).change, Change::Failed, "{plan:#?}");
    assert_eq!(cua(&plan).detail, cua(&real).detail);
}
