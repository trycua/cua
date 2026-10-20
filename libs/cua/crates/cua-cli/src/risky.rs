//! Risky actions on This Mac: `cua mcp` offers a separate Space before it
//! runs one (`cua config set nudges.risky_actions on`; off by default).
//!
//! **What counts as risky** is kept narrow on purpose, so the question is
//! rare and means something. A call is risky only when it targets This
//! Mac itself (the user's real desktop, not an extra Space) and it would:
//!
//! - delete files outside a project folder: `rm -r` of an absolute path,
//!   the home folder (`~`, `$HOME`), a parent folder (`..`) or everything
//!   (`*`, `.`); `rm -r node_modules` inside a project is not asked about;
//! - empty the Trash;
//! - uninstall apps (`brew uninstall`, deleting from `/Applications`);
//! - run an installer (`installer -pkg`, opening a `.pkg`);
//! - change system settings (`sudo`, `defaults write`, `systemsetup`,
//!   `networksetup -set…`, `pmset`, `launchctl` unload/bootout/disable,
//!   `tccutil reset`, `csrutil`, `spctl`), or open System Settings;
//! - touch sign-ins and passwords (the `security` keychain tool,
//!   `passwd`, `dscl … passwd`, `sysadminctl`, deleting `~/.ssh` or the
//!   keychains), or open Keychain Access or Passwords;
//! - erase a disk (`diskutil erase…`, `dd of=/dev/…`, `mkfs`).
//!
//! The commands come from `space_bash` and the apps from the driver's
//! `launch_app` through `call_tool`. Anything else (clicks, typing, a
//! command line this can't read) is not asked about: a click on "Delete"
//! in Finder looks like any other click. That's why it is off by default.
//!
//! **Asking never blocks for long.** `cua mcp` writes a request file
//! (`$CUA_HOME/nudges/<id>.json`); the Cua app shows "Run this in a
//! separate Space instead?" and writes the answer next to it
//! (`<id>.answer`: `separate` or `here`). With no answer within
//! [`ASK_FOR`], or with no app to ask, the call runs here as before.
//! "Use a separate Space" returns a plain result to the agent instead of
//! running the call, so it can move the work to a Space.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long a risky call waits for an answer before it runs here.
pub const ASK_FOR: Duration = Duration::from_secs(30);

/// How often the answer file is looked for.
const POLL: Duration = Duration::from_millis(300);

/// The config key and its environment variable.
pub const SETTING: &str = "nudges.risky_actions";

/// Whether the user turned the question on (`cua config`, or
/// `CUA_NUDGE_RISKY_ACTIONS`). Off when unset or unreadable.
pub fn enabled() -> bool {
    let Ok(key) = cua_sandbox_core::settings::key(SETTING) else {
        return false;
    };
    cua_sandbox_core::settings::Settings::load()
        .map(|s| s.entry(key).value == "on")
        .unwrap_or(false)
}

/// The words after "wants to …" for a risky shell command, or `None`.
pub fn command_risk(command: &str) -> Option<&'static str> {
    // One rule per simple command (`a && b; c | d`).
    command
        .split(['\n', ';', '|', '&'])
        .find_map(|part| simple_risk(&words(part)))
}

fn words(part: &str) -> Vec<String> {
    part.split_whitespace()
        .map(|w| {
            w.trim_matches(|c| c == '"' || c == '\'' || c == '(' || c == ')')
                .to_string()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

fn simple_risk(w: &[String]) -> Option<&'static str> {
    let w: Vec<&str> = w.iter().map(String::as_str).collect();
    let (&cmd, args) = w.split_first()?;
    let cmd = cmd.rsplit('/').next().unwrap_or(cmd);
    let any = |p: &dyn Fn(&str) -> bool| args.iter().any(|a| p(a));
    match cmd {
        "sudo" | "doas" => Some("make changes as an administrator"),
        "rm" | "rmdir" | "trash" => {
            if any(&|a| a.contains(".Trash")) {
                return Some("empty the Trash");
            }
            if any(&|a| a.contains("/Applications/") || a.ends_with(".app") || a.ends_with(".app/"))
            {
                return Some("uninstall an app");
            }
            if any(&|a| a.contains(".ssh") || a.contains("Library/Keychains")) {
                return Some("delete saved sign-ins and keys");
            }
            let recursive = cmd == "rmdir"
                || any(&|a| {
                    a == "--recursive"
                        || (a.starts_with('-')
                            && !a.starts_with("--")
                            && (a.contains('r') || a.contains('R')))
                });
            let outside = any(&|a| {
                !a.starts_with('-')
                    && (a.starts_with('/')
                        || a.starts_with('~')
                        || a.starts_with("$HOME")
                        || a.starts_with("${HOME}")
                        || a == ".."
                        || a.starts_with("../")
                        || a == "*"
                        || a == "."
                        || a == "./")
            });
            (recursive && outside).then_some("delete files outside a project folder")
        }
        "osascript" => {
            let script = args.join(" ").to_ascii_lowercase();
            if script.contains("empty trash") || script.contains("empty the trash") {
                Some("empty the Trash")
            } else if script.contains("system settings") || script.contains("system preferences") {
                Some("change system settings")
            } else {
                None
            }
        }
        "brew" if any(&|a| matches!(a, "uninstall" | "remove" | "rm" | "zap")) => {
            Some("uninstall an app")
        }
        "mas" | "pkgutil" if any(&|a| matches!(a, "uninstall" | "--forget")) => {
            Some("uninstall an app")
        }
        "installer" => Some("run an installer"),
        "open" if any(&|a| a.ends_with(".pkg") || a.ends_with(".mpkg")) => Some("run an installer"),
        "open" if is_settings_app(&args.join(" ")) => Some("change system settings"),
        "open" if is_passwords_app(&args.join(" ")) => Some("open your saved passwords"),
        "defaults" if any(&|a| matches!(a, "write" | "delete")) => Some("change system settings"),
        "systemsetup" | "pmset" | "csrutil" | "spctl" | "nvram" | "scutil" => {
            Some("change system settings")
        }
        "networksetup" if any(&|a| a.starts_with("-set") || a.starts_with("-remove")) => {
            Some("change system settings")
        }
        "launchctl" if any(&|a| matches!(a, "unload" | "bootout" | "disable" | "remove")) => {
            Some("change system settings")
        }
        "tccutil" => Some("change privacy settings"),
        "security" => Some("use your saved passwords and sign-ins"),
        "passwd" | "sysadminctl" => Some("change a password"),
        "dscl" if any(&|a| a.contains("passwd")) => Some("change a password"),
        "diskutil"
            if any(&|a| {
                a.starts_with("erase") || a.starts_with("zero") || a == "partitionDisk"
            }) =>
        {
            Some("erase a disk")
        }
        "dd" if any(&|a| a.starts_with("of=/dev/")) => Some("erase a disk"),
        c if c.starts_with("mkfs") || c == "newfs_apfs" || c == "newfs_hfs" => Some("erase a disk"),
        _ => None,
    }
}

fn is_settings_app(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("system settings")
        || n.contains("system preferences")
        || n.contains("com.apple.systempreferences")
        || n.contains("com.apple.settings")
        || n.starts_with("x-apple.systempreferences:")
}

fn is_passwords_app(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("keychain access")
        || n.contains("com.apple.keychainaccess")
        || n == "passwords"
        || n.ends_with(" passwords")
        || n.contains("com.apple.passwords")
        || n.contains("passwords.app")
}

/// The words for a risky contract tool call (`space_bash`, or the
/// driver's `launch_app` through `call_tool`), or `None`.
pub fn tool_risk(tool: &str, arguments: &Value) -> Option<&'static str> {
    match tool {
        "space_bash" => command_risk(arguments["command"].as_str()?),
        "call_tool" => {
            let inner = arguments["tool"].as_str()?;
            let args = &arguments["arguments"];
            match inner {
                "launch_app" | "activate_app" => {
                    let names = ["bundle_id", "name", "app", "path"]
                        .iter()
                        .filter_map(|k| args[*k].as_str())
                        .collect::<Vec<_>>();
                    if names.iter().any(|n| is_settings_app(n)) {
                        Some("change system settings")
                    } else if names.iter().any(|n| is_passwords_app(n)) {
                        Some("open your saved passwords")
                    } else if names
                        .iter()
                        .any(|n| n.ends_with(".pkg") || n.ends_with(".mpkg"))
                    {
                        Some("run an installer")
                    } else {
                        None
                    }
                }
                "shell" | "run_shell" | "bash" | "exec" => {
                    command_risk(args["command"].as_str().or(args["cmd"].as_str())?)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// The Space ids and names that mean This Mac: its own relay machine
/// (`relay:<id>` and the bare id), its host name, and the names the apps
/// give it.
pub fn this_mac_names(home: &Path) -> Vec<String> {
    let mut names: Vec<String> = ["this-mac", "this mac", "this-machine", "this machine"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if let Ok(Some(c)) = cua_host::Host::new(home.to_path_buf()).config() {
        if let Some(id) = c.machine_id.filter(|id| !id.is_empty()) {
            names.push(format!("relay:{id}"));
            names.push(id);
        }
        if !c.name.trim().is_empty() {
            names.push(c.name.trim().to_string());
        }
    }
    names.into_iter().map(|n| n.to_ascii_lowercase()).collect()
}

/// Whether `arguments` name This Mac as the Space.
pub fn targets_this_mac(arguments: &Value, names: &[String]) -> bool {
    arguments["space"]
        .as_str()
        .map(|s| s.trim().to_ascii_lowercase())
        .is_some_and(|s| names.contains(&s))
}

/// The user's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// "Use a separate Space": don't run it here.
    Separate,
    /// "Run it here", no answer in time, or nobody to ask.
    Here,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The folder requests and answers live in.
pub fn dir(home: &Path) -> PathBuf {
    home.join("nudges")
}

/// Asks the app and waits up to `wait` for the answer. Any failure to ask
/// (no folder, a write error) is "Run it here": the question never stops
/// the agent.
pub async fn ask(home: &Path, agent: &str, action: &str, wait: Duration) -> Answer {
    let dir = dir(home);
    let now = now_ms();
    let id = format!("risky-{}-{now}", std::process::id());
    let request = dir.join(format!("{id}.json"));
    let answer = dir.join(format!("{id}.answer"));
    let body = serde_json::json!({
        "id": id,
        "agent": agent,
        "action": action,
        "atMs": now,
        "expiresMs": now + wait.as_millis() as u64,
    });
    if std::fs::create_dir_all(&dir).is_err()
        || cua_home::write_private(&request, body.to_string().as_bytes()).is_err()
    {
        return Answer::Here;
    }
    let deadline = tokio::time::Instant::now() + wait;
    let mut out = Answer::Here;
    while tokio::time::Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(&answer) {
            if text.trim() == "separate" {
                out = Answer::Separate;
            }
            break;
        }
        tokio::time::sleep(POLL).await;
    }
    let _ = std::fs::remove_file(&request);
    let _ = std::fs::remove_file(&answer);
    out
}

/// What the agent reads when the user chose a separate Space.
pub fn separate_result(action: &str) -> String {
    format!(
        "Not run. This would {action} on the user's own Mac, and they asked to do it in a \
         separate Space instead. Cua is making one now (list_spaces shows it). Run this there \
         once it is ready, or ask the user what to do next."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn risky_commands_are_named_in_plain_words() {
        for (cmd, want) in [
            (
                "rm -rf ~/Documents",
                "delete files outside a project folder",
            ),
            (
                "rm -r /Users/ada/Desktop/old",
                "delete files outside a project folder",
            ),
            (
                "cd x && rm -fr ../other",
                "delete files outside a project folder",
            ),
            ("rm -rf *", "delete files outside a project folder"),
            ("rm -rf ~/.Trash/*", "empty the Trash"),
            (
                "osascript -e 'tell app \"Finder\" to empty trash'",
                "empty the Trash",
            ),
            ("rm -rf /Applications/Slack.app", "uninstall an app"),
            ("brew uninstall --cask slack", "uninstall an app"),
            (
                "sudo installer -pkg Recorder.pkg -target /",
                "make changes as an administrator",
            ),
            ("installer -pkg Recorder.pkg -target /", "run an installer"),
            ("open ~/Downloads/Recorder.pkg", "run an installer"),
            (
                "defaults write -g AppleShowAllExtensions -bool true",
                "change system settings",
            ),
            ("open -a 'System Settings'", "change system settings"),
            (
                "security find-generic-password -s github",
                "use your saved passwords and sign-ins",
            ),
            ("rm -rf ~/.ssh", "delete saved sign-ins and keys"),
            ("diskutil eraseDisk APFS X disk4", "erase a disk"),
            ("/usr/bin/tccutil reset All", "change privacy settings"),
        ] {
            assert_eq!(command_risk(cmd), Some(want), "{cmd}");
        }
    }

    #[test]
    fn everyday_commands_are_not_asked_about() {
        for cmd in [
            "rm -rf node_modules",
            "rm -rf build dist",
            "rm notes.txt",
            "rm /tmp/x.log",
            "ls -la ~",
            "git status && npm test",
            "brew install jq",
            "defaults read com.apple.dock",
            "open https://example.com",
            "launchctl list",
            "echo rm -rf",
        ] {
            assert_eq!(command_risk(cmd), None, "{cmd}");
        }
    }

    #[test]
    fn driver_launches_of_settings_and_passwords_count() {
        let call =
            |tool: &str, args: Value| json!({"space": "this-mac", "tool": tool, "arguments": args});
        assert_eq!(
            tool_risk(
                "call_tool",
                &call(
                    "launch_app",
                    json!({"bundle_id": "com.apple.systempreferences"})
                )
            ),
            Some("change system settings")
        );
        assert_eq!(
            tool_risk(
                "call_tool",
                &call("launch_app", json!({"name": "Passwords"}))
            ),
            Some("open your saved passwords")
        );
        assert_eq!(
            tool_risk("call_tool", &call("launch_app", json!({"name": "Safari"}))),
            None
        );
        assert_eq!(
            tool_risk("call_tool", &call("click", json!({"x": 1, "y": 2}))),
            None
        );
        assert_eq!(
            tool_risk(
                "space_bash",
                &json!({"space": "this-mac", "command": "brew zap slack"})
            ),
            Some("uninstall an app")
        );
        assert_eq!(tool_risk("list_spaces", &json!({})), None);
    }

    #[test]
    fn only_this_mac_is_this_mac() {
        let names = vec!["this-mac".to_string(), "relay:m-1".to_string()];
        assert!(targets_this_mac(&json!({"space": "relay:m-1"}), &names));
        assert!(targets_this_mac(&json!({"space": "This-Mac"}), &names));
        assert!(!targets_this_mac(
            &json!({"space": "local:linux-1"}),
            &names
        ));
        assert!(!targets_this_mac(&json!({}), &names));
    }

    #[tokio::test]
    async fn no_answer_runs_it_here_and_an_answer_is_followed() {
        let home = tempfile::tempdir().unwrap();
        let started = std::time::Instant::now();
        assert_eq!(
            ask(
                home.path(),
                "Codex",
                "empty the Trash",
                Duration::from_millis(400)
            )
            .await,
            Answer::Here
        );
        assert!(started.elapsed() < Duration::from_secs(3));
        // Nothing is left behind.
        assert_eq!(std::fs::read_dir(dir(home.path())).unwrap().count(), 0);

        // The app answers "separate" while the call waits.
        let d = dir(home.path());
        let answering = tokio::spawn(async move {
            for _ in 0..50 {
                if let Some(req) = std::fs::read_dir(&d)
                    .ok()
                    .and_then(|mut r| r.find_map(|e| e.ok()))
                    .map(|e| e.path())
                {
                    std::fs::write(req.with_extension("answer"), "separate").unwrap();
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        });
        assert_eq!(
            ask(
                home.path(),
                "Codex",
                "empty the Trash",
                Duration::from_secs(5)
            )
            .await,
            Answer::Separate
        );
        answering.await.unwrap();
    }
}
