//! `cua-driver mcp-config --client <agent>` installs the skill pack for that
//! agent by default, prints one line about it on stderr, and leaves the agent
//! alone with `--no-skills`. Every run uses a throwaway home.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

const SKILL: &str = include_str!("../../../Skills/cua-driver/SKILL.md");

struct Sandbox {
    _root: TempDir,
    home: PathBuf,
    driver_home: PathBuf,
    hermes_home: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("temporary test root");
        let home = root.path().join("home");
        fs::create_dir_all(&home).unwrap();
        Sandbox {
            driver_home: root.path().join("driver"),
            hermes_home: home.join(".hermes"),
            home,
            _root: root,
        }
    }

    /// An agent that has been started once: its own folder exists.
    fn has(&self, relative: &str) {
        fs::create_dir_all(self.home.join(relative)).unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_cua-driver"))
            .arg("mcp-config")
            .args(args)
            .stdin(Stdio::null())
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("APPDATA", self.home.join("AppData/Roaming"))
            .env("CUA_DRIVER_RS_HOME", &self.driver_home)
            .env("HERMES_HOME", &self.hermes_home)
            .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "0")
            .output()
            .expect("run cua-driver mcp-config")
    }

    fn stage(&self) -> PathBuf {
        self.driver_home.join("skills").join("cua-driver")
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn only_line(output: &Output) -> String {
    let text = stderr(output);
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(lines.len(), 1, "expected one line on stderr: {text:?}");
    lines[0].to_owned()
}

fn assert_nothing_installed(sandbox: &Sandbox) {
    assert!(
        !sandbox.driver_home.exists(),
        "no local skill pack expected"
    );
    for relative in [
        ".claude/skills",
        ".agents/skills",
        ".config/opencode/skills",
    ] {
        assert!(
            !sandbox.home.join(relative).exists(),
            "{relative} should not exist"
        );
    }
}

#[test]
fn no_skills_leaves_the_agent_alone_and_keeps_stdout_the_same() {
    let with = Sandbox::new();
    with.has(".claude");
    let installed = with.run(&["--client", "claude"]);

    let without = Sandbox::new();
    without.has(".claude");
    let skipped = without.run(&["--client", "claude", "--no-skills"]);

    assert!(installed.status.success() && skipped.status.success());
    assert_eq!(stdout(&installed), stdout(&skipped));
    assert!(stdout(&skipped).starts_with("claude mcp add-json"));
    assert!(stderr(&skipped).is_empty(), "{}", stderr(&skipped));
    assert_nothing_installed(&without);
    assert!(!without.home.join(".claude/skills").exists());
    // The flag works in either position.
    let first = without.run(&["--no-skills", "--client", "claude"]);
    assert!(first.status.success() && stderr(&first).is_empty());
    assert_nothing_installed(&without);
}

#[test]
fn the_generic_snippet_and_unsupported_clients_install_nothing() {
    let sandbox = Sandbox::new();
    sandbox.has(".claude");
    sandbox.has(".codex");
    for args in [
        &[][..],
        &["--client", "qwen"],
        &["--client", "droid"],
        &["--client", "zcode"],
    ] {
        let output = sandbox.run(args);
        assert!(output.status.success(), "{args:?}: {output:?}");
        assert!(stderr(&output).is_empty(), "{args:?}: {}", stderr(&output));
    }
    assert_nothing_installed(&sandbox);
}

#[test]
fn an_unknown_client_is_refused_before_anything_is_installed() {
    let sandbox = Sandbox::new();
    sandbox.has(".claude");
    let output = sandbox.run(&["--client", "emacs"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("Unknown client 'emacs'"));
    assert_nothing_installed(&sandbox);
}

#[test]
fn an_agent_that_is_not_on_this_computer_gets_a_hint_not_a_folder() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(&["--client", "claude"]);
    assert!(output.status.success());
    assert!(stdout(&output).starts_with("claude mcp add-json"));
    let line = only_line(&output);
    assert!(line.contains("no Claude Code folder found"), "{line}");
    assert!(line.contains("cua-driver skills install"), "{line}");
    assert_nothing_installed(&sandbox);
}

#[cfg(unix)]
mod placement {
    use super::*;
    use std::os::unix::fs::symlink;

    /// Where each supported agent loads skills from: the agent's own folder
    /// (made first, as the agent does on its first run) and the link the
    /// skill pack must land at.
    const AGENTS: &[(&str, &str, &[&str])] = &[
        ("claude", ".claude", &[".claude/skills/cua-driver"]),
        ("claude-code", ".claude", &[".claude/skills/cua-driver"]),
        ("codex", ".codex", &[".agents/skills/cua-driver"]),
        ("cursor", ".cursor", &[".agents/skills/cua-driver"]),
        ("pi", ".pi/agent", &[".agents/skills/cua-driver"]),
        ("openclaw", ".openclaw", &[".openclaw/skills/cua-driver"]),
        (
            "opencode",
            ".config/opencode",
            &[".config/opencode/skills/cua-driver"],
        ),
        ("hermes", ".hermes", &[".hermes/skills/cua-driver"]),
        (
            "prime-agent",
            ".prime/agent",
            &[".prime/agent/skills/cua-driver"],
        ),
        (
            "antigravity",
            ".gemini/config",
            &[".gemini/config/skills/cua-driver"],
        ),
        (
            "gemini",
            ".gemini/antigravity-cli",
            &[".gemini/antigravity-cli/skills/cua-driver"],
        ),
    ];

    fn assert_linked_to_the_pack(sandbox: &Sandbox, link: &Path) {
        let meta = fs::symlink_metadata(link).unwrap_or_else(|_| panic!("{link:?} missing"));
        assert!(meta.file_type().is_symlink(), "{link:?} should be a link");
        assert_eq!(fs::read_link(link).unwrap(), sandbox.stage());
        assert_eq!(
            fs::read_to_string(link.join("SKILL.md")).unwrap(),
            SKILL,
            "the agent reads the SKILL.md compiled into this driver"
        );
    }

    #[test]
    fn each_supported_agent_gets_the_skill_pack_where_it_loads_skills() {
        for (client, folder, links) in AGENTS {
            let sandbox = Sandbox::new();
            sandbox.has(folder);
            let output = sandbox.run(&["--client", client]);
            assert!(output.status.success(), "{client}: {output:?}");
            for link in *links {
                assert_linked_to_the_pack(&sandbox, &sandbox.home.join(link));
            }
            let line = only_line(&output);
            assert!(
                line.contains("installed the skill pack"),
                "{client}: {line}"
            );
            assert!(line.contains("--no-skills"), "{client}: {line}");
            assert!(
                line.contains(&sandbox.home.join(links[0]).display().to_string()),
                "{client}: {line}"
            );
            // The config on stdout is the same with or without the install.
            let skipped = sandbox.run(&["--client", client, "--no-skills"]);
            assert_eq!(stdout(&output), stdout(&skipped), "{client}");
            assert!(
                stderr(&skipped).is_empty(),
                "{client}: {}",
                stderr(&skipped)
            );
        }
    }

    #[test]
    fn antigravity_gets_both_the_ide_and_the_cli_folder_that_exist() {
        let sandbox = Sandbox::new();
        sandbox.has(".gemini/config");
        sandbox.has(".gemini/antigravity-cli");
        let output = sandbox.run(&["--client", "antigravity"]);
        assert!(output.status.success());
        assert_linked_to_the_pack(
            &sandbox,
            &sandbox.home.join(".gemini/config/skills/cua-driver"),
        );
        assert_linked_to_the_pack(
            &sandbox,
            &sandbox
                .home
                .join(".gemini/antigravity-cli/skills/cua-driver"),
        );
        assert_eq!(only_line(&output).matches("skill pack").count(), 1);
    }

    #[test]
    fn the_pack_is_this_drivers_own_and_only_for_this_host() {
        let sandbox = Sandbox::new();
        sandbox.has(".claude");
        sandbox.run(&["--client", "claude"]);
        let stage = sandbox.stage();
        for file in ["SKILL.md", "WORKFLOW.md", "BROWSER.md", "RUNTIME.md"] {
            assert!(stage.join(file).is_file(), "{file} missing from the pack");
        }
        let (own, others) = if cfg!(target_os = "macos") {
            ("MACOS.md", ["WINDOWS.md", "LINUX.md"])
        } else {
            ("LINUX.md", ["WINDOWS.md", "MACOS.md"])
        };
        assert!(stage.join(own).is_file());
        for other in others {
            assert!(!stage.join(other).exists(), "{other} is another platform's");
        }
    }

    #[test]
    fn rerunning_is_idempotent_and_refreshes_a_stale_pack() {
        let sandbox = Sandbox::new();
        sandbox.has(".codex");
        let first = sandbox.run(&["--client", "codex"]);
        assert!(only_line(&first).contains("installed the skill pack"));
        fs::write(sandbox.stage().join("SKILL.md"), "stale").unwrap();

        let second = sandbox.run(&["--client", "codex"]);
        assert!(second.status.success());
        assert!(
            only_line(&second).contains("already installed"),
            "{}",
            stderr(&second)
        );
        assert_linked_to_the_pack(&sandbox, &sandbox.home.join(".agents/skills/cua-driver"));
    }

    #[test]
    fn a_skill_folder_the_user_made_is_never_replaced() {
        let sandbox = Sandbox::new();
        let mine = sandbox.home.join(".claude/skills/cua-driver");
        fs::create_dir_all(&mine).unwrap();
        fs::write(mine.join("SKILL.md"), "my own notes").unwrap();

        let output = sandbox.run(&["--client", "claude"]);
        assert!(output.status.success());
        assert!(only_line(&output).contains("already installed"));
        assert_eq!(
            fs::read_to_string(mine.join("SKILL.md")).unwrap(),
            "my own notes"
        );
        assert!(!fs::symlink_metadata(&mine)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn a_developers_linked_pack_is_linked_to_not_overwritten() {
        let sandbox = Sandbox::new();
        sandbox.has(".claude");
        let checkout = sandbox.home.join("checkout/Skills/cua-driver");
        fs::create_dir_all(&checkout).unwrap();
        fs::write(checkout.join("SKILL.md"), "work in progress").unwrap();
        fs::create_dir_all(sandbox.driver_home.join("skills")).unwrap();
        symlink(&checkout, sandbox.stage()).unwrap();

        let output = sandbox.run(&["--client", "claude"]);
        assert!(output.status.success());
        assert_eq!(
            fs::read_to_string(checkout.join("SKILL.md")).unwrap(),
            "work in progress",
            "the checkout behind the link must not be rewritten"
        );
        assert!(
            fs::symlink_metadata(sandbox.home.join(".claude/skills/cua-driver"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}
