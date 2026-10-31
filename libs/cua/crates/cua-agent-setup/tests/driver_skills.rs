//! Setting up cua-driver for an agent installs the cua-driver skill by
//! default, in the folder that agent loads skills from, and `skills: false`
//! (the engine behind `cua agents setup --cua-driver --no-skills` and the
//! installers' `--no-skills`) leaves every skills folder untouched. Always in
//! a temporary HOME, never the real one.

mod common;

use cua_agent_setup::{AGENTS, AgentSetup, Change, HostEnv, McpServer, Parts, Target};
use std::path::{Path, PathBuf};

struct Home {
    _dir: tempfile::TempDir,
    path: PathBuf,
    env: HostEnv,
}

impl Home {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("home");
        std::fs::create_dir_all(&path).unwrap();
        let env = HostEnv::isolated(&path);
        Home {
            _dir: dir,
            path,
            env,
        }
    }

    fn setup(&self) -> AgentSetup {
        AgentSetup::new(self.env.clone())
    }
}

fn driver() -> McpServer {
    McpServer::driver("/opt/drv/bin/cua-driver")
}

fn ids(id: &str) -> Vec<String> {
    vec![id.to_string()]
}

/// Every file under `dir`, for "nothing was written here".
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(files_under(&path));
        } else {
            out.push(path);
        }
    }
    out
}

#[test]
fn every_agent_with_a_skills_folder_gets_the_driver_skill_by_default() {
    let with_skills: Vec<_> = AGENTS.iter().filter(|a| a.skills.is_some()).collect();
    assert!(with_skills.len() >= 15, "registry lost agents");
    for agent in with_skills {
        let home = Home::new();
        let skills_dir = agent.skills.unwrap().resolve(&home.env);
        let out = home
            .setup()
            .setup_cua_driver(&ids(agent.id), &driver(), Parts::ALL, false)
            .unwrap();

        let skill: Vec<_> = out
            .iter()
            .filter(|o| o.target == Target::Skill && o.item == "cua-driver")
            .collect();
        assert_eq!(skill.len(), 1, "{}: {out:#?}", agent.id);
        assert_eq!(skill[0].change, Change::Created, "{}: {out:#?}", agent.id);
        assert_eq!(skill[0].path, skills_dir.join("cua-driver"), "{}", agent.id);
        let md =
            std::fs::read_to_string(skills_dir.join("cua-driver/SKILL.md")).unwrap_or_else(|e| {
                panic!("{}: no SKILL.md in {}: {e}", agent.id, skills_dir.display())
            });
        assert!(md.contains("\nname: cua-driver\n"), "{}", agent.id);
        // Only the driver skill: the other bundled skills are not forced on
        // someone who asked for background computer-use.
        assert!(!skills_dir.join("cua-spaces").exists(), "{}", agent.id);
    }
}

#[test]
fn no_skills_leaves_every_skills_folder_untouched() {
    for agent in AGENTS.iter().filter(|a| a.skills.is_some()) {
        let home = Home::new();
        let skills_dir = agent.skills.unwrap().resolve(&home.env);
        let out = home
            .setup()
            .setup_cua_driver(
                &ids(agent.id),
                &driver(),
                Parts {
                    skills: false,
                    mcp: true,
                },
                false,
            )
            .unwrap();
        assert!(
            out.iter().all(|o| o.target == Target::Mcp),
            "{}: {out:#?}",
            agent.id
        );
        assert!(
            !skills_dir.join("cua-driver").exists(),
            "{}: --no-skills must not install the skill",
            agent.id
        );
        assert!(
            files_under(&skills_dir).is_empty(),
            "{}: {:?}",
            agent.id,
            files_under(&skills_dir)
        );
    }
}

#[test]
fn skills_only_installs_the_skill_without_touching_mcp_config() {
    let home = Home::new();
    let out = home
        .setup()
        .setup_cua_driver(
            &ids("claude-code"),
            &driver(),
            Parts {
                skills: true,
                mcp: false,
            },
            false,
        )
        .unwrap();
    assert!(out.iter().all(|o| o.target == Target::Skill), "{out:#?}");
    assert!(
        home.path
            .join(".claude/skills/cua-driver/SKILL.md")
            .is_file()
    );
    assert!(!home.path.join(".claude.json").exists());
}

#[cfg(unix)]
#[test]
fn rerunning_the_setup_changes_nothing() {
    let home = Home::new();
    let setup = home.setup();
    let driver = McpServer::driver(common::answering_driver());
    setup
        .setup_cua_driver(&ids("codex"), &driver, Parts::ALL, false)
        .unwrap();
    let again = setup
        .setup_cua_driver(&ids("codex"), &driver, Parts::ALL, false)
        .unwrap();
    assert!(
        again.iter().all(|o| o.change == Change::Unchanged),
        "{again:#?}"
    );
}
