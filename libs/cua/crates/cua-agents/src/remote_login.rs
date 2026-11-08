//! An agent's own sign-in, made on the computer it runs on.
//!
//! Cua does not move a Claude Code or ChatGPT sign-in between computers by
//! default. When Claude Code or Codex on another computer (a Space, a Mac
//! mini) is not signed in, Cua starts the agent's own remote sign-in there
//! and shows the person the link and code, like signing in to a TV:
//!
//! | Agent | Command on that computer | What the person does | Where the sign-in lands |
//! |---|---|---|---|
//! | Codex | `codex login --device-auth` | opens the link, types the code | `~/.codex/auth.json` |
//! | Claude Code | `claude auth login --claudeai` (prints a link, then "Paste code here if prompted") | opens the link, signs in, pastes the code the page shows back into Cua | `~/.claude/.credentials.json` (Linux) or the Keychain (macOS) |
//!
//! Checked against codex-cli 0.164.0 and Claude Code 2.1.318 (8 Nov 2026).
//!
//! The login runs detached in `~/.cua/agent-signin/<harness>/` (stdin from a
//! FIFO fed by the file `in`, output in `out`, exit code in `exit`, stopped
//! after [`LOGIN_WAIT`]). Cua only reads the link and code the command
//! prints; the sign-in itself stays in the agent's own config on that
//! computer and never passes through Cua.

use serde::Serialize;

use crate::harness::{self, Harness};
use crate::quote;

/// How long a login waits for the person (Codex's device code lasts 15 minutes).
pub const LOGIN_WAIT_SECS: u64 = 15 * 60;

/// Where the login's files live, under the guest home.
pub const DIR: &str = ".cua/agent-signin";

/// Separates the parts of [`status_script`]'s answer.
pub const RS: &str = "--cua-signin--";

/// Largest part of the login's output read back.
const OUT_TAIL: usize = 16 * 1024;

/// The harnesses that sign in on the computer they run on.
pub fn supported(harness: &str) -> bool {
    matches!(harness, "claude-code" | "openai-codex")
}

/// The login command, as arguments for `sh`.
fn command(h: &Harness) -> Option<&'static [&'static str]> {
    match h.id {
        "claude-code" => Some(&["claude", "auth", "login", "--claudeai"]),
        "openai-codex" => Some(&["codex", "login", "--device-auth"]),
        _ => None,
    }
}

/// Whether the person pastes a code back (Claude Code) rather than typing a
/// code on the provider's page (Codex).
pub fn pastes_code(harness: &str) -> bool {
    harness == "claude-code"
}

fn dir_expr(h: &Harness) -> String {
    format!("\"$HOME\"/{}", quote(&format!("{DIR}/{}", h.id)))
}

/// The guest script that starts `h`'s own sign-in, stopping one already
/// going. The agent's CLI must be installed (`~/.cua/bin`).
pub fn start_script(h: &Harness) -> Option<String> {
    let cmd = command(h)?
        .iter()
        .map(|a| quote(a))
        .collect::<Vec<_>>()
        .join(" ");
    let d = dir_expr(h);
    Some(format!(
        "umask 077\n\
         export PATH=\"$HOME/.cua/bin:$PATH\"\n\
         unset CLAUDE_CONFIG_DIR CODEX_HOME CLAUDE_CODE_OAUTH_TOKEN ANTHROPIC_API_KEY OPENAI_API_KEY\n\
         export BROWSER=true\n\
         d={d}\n\
         [ -s \"$d/child\" ] && kill \"$(cat \"$d/child\")\" 2>/dev/null\n\
         rm -rf \"$d\" && mkdir -p \"$d\" && : > \"$d/in\" && mkfifo \"$d/fifo\" || exit 1\n\
         nohup sh -c '\n\
         d=\"$1\"; shift\n\
         tail -n +1 -f \"$d/in\" > \"$d/fifo\" 2>/dev/null & t=$!\n\
         \"$@\" < \"$d/fifo\" > \"$d/out\" 2>&1 & c=$!\n\
         echo \"$c\" > \"$d/child\"\n\
         ( sleep {LOGIN_WAIT_SECS}; kill \"$c\" 2>/dev/null ) > /dev/null 2>&1 & w=$!\n\
         wait \"$c\"; echo \"$?\" > \"$d/exit\"\n\
         kill \"$t\" \"$w\" 2>/dev/null; rm -f \"$d/in\" \"$d/fifo\"\n\
         ' sh \"$d\" {cmd} > /dev/null 2>&1 &\n\
         echo started\n"
    ))
}

/// The guest script that hands the pasted code to a running Claude Code login.
pub fn code_script(h: &Harness, code: &str) -> String {
    let d = dir_expr(h);
    format!(
        "d={d}\n[ -e \"$d/in\" ] || {{ echo gone; exit 0; }}\nprintf '%s\\n' {} >> \"$d/in\" && echo sent\n",
        quote(code.trim())
    )
}

/// The guest script that stops a login.
pub fn cancel_script(h: &Harness) -> String {
    let d = dir_expr(h);
    format!(
        "d={d}\n[ -s \"$d/child\" ] && kill \"$(cat \"$d/child\")\" 2>/dev/null\nrm -rf \"$d\"\necho stopped\n"
    )
}

/// The guest script that reports: whether the agent's own login files are
/// there, whether a login is running, its exit code, and the end of what it
/// printed.
pub fn status_script(h: &Harness) -> String {
    let d = dir_expr(h);
    let mut probe = String::new();
    for t in h.logins.unwrap_or_default() {
        probe.push_str(&format!("{t} && {{ echo yes; }} || "));
    }
    probe.push_str("echo no");
    format!(
        "d={d}\n\
         {{ {probe}; }} | head -n 1\n\
         echo {RS}\n\
         if [ -s \"$d/child\" ] && [ ! -e \"$d/exit\" ] && kill -0 \"$(cat \"$d/child\")\" 2>/dev/null; then echo running; \
         elif [ -e \"$d\" ]; then echo ended; else echo none; fi\n\
         echo {RS}\n\
         cat \"$d/exit\" 2>/dev/null\n\
         echo {RS}\n\
         tail -c {OUT_TAIL} \"$d/out\" 2>/dev/null\n"
    )
}

/// What to do with a computer's sign-in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Status,
    Start,
    /// The code the person pasted (Claude Code).
    Code(String),
    Cancel,
}

impl Action {
    /// `status`, `start`, `code` (with `code`), `cancel`.
    pub fn parse(action: &str, code: Option<String>) -> Option<Action> {
        Some(match (action, code) {
            ("status", _) => Action::Status,
            ("start", _) => Action::Start,
            ("code", Some(c)) => Action::Code(c),
            ("cancel", _) => Action::Cancel,
            _ => return None,
        })
    }
}

/// Where a computer's sign-in stands for one agent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LoginState {
    /// The agent has no sign-in of its own on that computer.
    NotSignedIn,
    /// The sign-in is going: what to show the person.
    SigningIn {
        /// The page to open; `None` while the agent is still starting.
        url: Option<String>,
        /// The code to type on that page (Codex).
        code: Option<String>,
        /// The person pastes the code the page shows back into Cua (Claude Code).
        paste_code: bool,
    },
    /// Signed in on that computer.
    SignedIn,
    /// The sign-in stopped; `reason` in plain words.
    Failed { reason: String },
    /// It has a sign-in, but the agent's provider refused it: sign in again.
    Expired,
}

/// What [`status_script`] printed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Probe {
    pub has_login: bool,
    pub running: bool,
    /// A login was started and its folder is still there.
    pub started: bool,
    pub exit: Option<i32>,
    pub out: String,
}

/// Reads [`status_script`]'s answer.
pub fn parse_probe(text: &str) -> Probe {
    let sep = format!("{RS}\n");
    let mut parts = text.split(sep.as_str());
    let has_login = parts.next().is_some_and(|p| p.trim() == "yes");
    let run = parts.next().unwrap_or("").trim().to_string();
    let exit = parts.next().and_then(|p| p.trim().parse().ok());
    let out = parts.collect::<Vec<_>>().join(&sep);
    Probe {
        has_login,
        running: run == "running",
        started: run == "running" || run == "ended",
        exit,
        out,
    }
}

/// The link and Codex's one-time code in what a login printed.
pub fn find_link(harness: &str, out: &str) -> (Option<String>, Option<String>) {
    if harness == "openai-codex"
        && let Some((url, code)) = crate::signin::find_device_code(out)
    {
        return (Some(url), Some(code));
    }
    let url = out
        .split_whitespace()
        .rev()
        .find(|w| w.starts_with("https://") && w.len() > 12)
        .map(|w| w.trim_end_matches(['.', ',']).to_string());
    // Claude Code prints its authorize link once; the newest wins if it prints another.
    (url, None)
}

/// Decides the state from a [`Probe`]. `after_refusal`: the agent's last run
/// on that computer was refused its sign-in (so a sign-in that is there has
/// run out).
pub fn decide(harness: &str, p: &Probe, after_refusal: bool) -> LoginState {
    if p.running {
        let (url, code) = find_link(harness, &p.out);
        return LoginState::SigningIn {
            url,
            code,
            paste_code: pastes_code(harness),
        };
    }
    if p.started {
        match p.exit {
            Some(0) if p.has_login => return LoginState::SignedIn,
            Some(0) => {}
            Some(c) => {
                let name = harness::harness(harness).map_or(harness, |h| h.name);
                return LoginState::Failed {
                    reason: crate::signin::sign_in_failure(name, Some(c), &p.out),
                };
            }
            None => {
                return LoginState::Failed {
                    reason: "The sign-in was stopped before it finished. Try again.".into(),
                };
            }
        }
    }
    match (p.has_login, after_refusal) {
        (true, false) => LoginState::SignedIn,
        (true, true) => LoginState::Expired,
        (false, _) => LoginState::NotSignedIn,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE_OUT: &str = "Opening browser to sign in…\nIf the browser didn't open, visit: https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c&response_type=code&state=abc\nPaste code here if prompted > ";
    const CODEX_OUT: &str = "\u{1b}[94mWelcome to Codex\u{1b}[0m\nFollow these steps to sign in with ChatGPT using device code authorization:\n\n1. Open this link in your browser and sign in to your account\n   \u{1b}[94mhttps://auth.openai.com/codex/device\u{1b}[0m\n\n2. Enter this one-time code \u{1b}[90m(expires in 15 minutes)\u{1b}[0m\n   \u{1b}[94mABCD-12345\u{1b}[0m\n";

    fn probe(has_login: bool, run: &str, exit: &str, out: &str) -> Probe {
        parse_probe(&format!(
            "{}\n{RS}\n{run}\n{RS}\n{exit}\n{RS}\n{out}",
            if has_login { "yes" } else { "no" }
        ))
    }

    #[test]
    fn not_signed_in() {
        let p = probe(false, "none", "", "");
        assert_eq!(decide("claude-code", &p, false), LoginState::NotSignedIn);
        assert_eq!(decide("openai-codex", &p, true), LoginState::NotSignedIn);
    }

    #[test]
    fn signing_in_shows_claudes_link_and_asks_for_the_pasted_code() {
        let p = probe(false, "running", "", CLAUDE_OUT);
        assert_eq!(
            decide("claude-code", &p, false),
            LoginState::SigningIn {
                url: Some("https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c&response_type=code&state=abc".into()),
                code: None,
                paste_code: true,
            }
        );
    }

    #[test]
    fn signing_in_shows_codexs_link_and_code() {
        let p = probe(false, "running", "", CODEX_OUT);
        assert_eq!(
            decide("openai-codex", &p, false),
            LoginState::SigningIn {
                url: Some("https://auth.openai.com/codex/device".into()),
                code: Some("ABCD-12345".into()),
                paste_code: false,
            }
        );
    }

    #[test]
    fn a_login_still_starting_has_no_link_yet() {
        let p = probe(false, "running", "", "");
        assert!(matches!(
            decide("openai-codex", &p, false),
            LoginState::SigningIn { url: None, .. }
        ));
    }

    #[test]
    fn signed_in_after_the_login_finishes_or_when_already_there() {
        assert_eq!(
            decide("claude-code", &probe(true, "ended", "0", CLAUDE_OUT), false),
            LoginState::SignedIn
        );
        assert_eq!(
            decide("openai-codex", &probe(true, "none", "", ""), false),
            LoginState::SignedIn
        );
    }

    #[test]
    fn failed_says_what_the_agent_said() {
        let p = probe(
            false,
            "ended",
            "1",
            "Paste code here if prompted > \nOAuth error: invalid code\n",
        );
        match decide("claude-code", &p, false) {
            LoginState::Failed { reason } => assert!(reason.contains("invalid code"), "{reason}"),
            s => panic!("{s:?}"),
        }
        // Stopped without an exit code (cancelled or the computer restarted).
        assert!(matches!(
            decide("claude-code", &probe(false, "ended", "", ""), false),
            LoginState::Failed { .. }
        ));
    }

    #[test]
    fn expired_when_the_sign_in_is_there_but_was_refused() {
        assert_eq!(
            decide("claude-code", &probe(true, "none", "", ""), true),
            LoginState::Expired
        );
        // A fresh sign-in made after the refusal counts as signed in.
        assert_eq!(
            decide("claude-code", &probe(true, "ended", "0", ""), true),
            LoginState::SignedIn
        );
    }

    #[test]
    fn only_claude_code_and_codex_sign_in_on_the_computer() {
        for h in harness::HARNESSES {
            assert_eq!(start_script(h).is_some(), supported(h.id), "{}", h.id);
        }
    }

    #[test]
    fn scripts_use_the_agents_own_login_and_never_a_copied_sign_in() {
        let claude = harness::harness("claude-code").unwrap();
        let codex = harness::harness("openai-codex").unwrap();
        let s = start_script(claude).unwrap();
        assert!(
            s.contains("'claude' 'auth' 'login' '--claudeai'")
                || s.contains("claude auth login --claudeai"),
            "{s}"
        );
        assert!(s.contains("unset CLAUDE_CONFIG_DIR CODEX_HOME CLAUDE_CODE_OAUTH_TOKEN"));
        let s = start_script(codex).unwrap();
        assert!(s.contains("--device-auth"), "{s}");
        assert!(!s.contains("CODEX_AUTH_JSON="));
    }

    #[test]
    fn scripts_parse() {
        let claude = harness::harness("claude-code").unwrap();
        for s in [
            start_script(claude).unwrap(),
            status_script(claude),
            code_script(claude, "abc#def'; rm -rf /"),
            cancel_script(claude),
        ] {
            let out = std::process::Command::new("sh")
                .args(["-n", "-c", &s])
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{s}\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    /// The real scripts, on this machine, with a stand-in `claude` that prints
    /// Claude Code's words and signs in when it reads a code.
    #[cfg(unix)]
    #[test]
    fn a_login_runs_shows_its_link_takes_the_pasted_code_and_signs_in() {
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join(".cua/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let fake = bin.join("claude");
        std::fs::write(
            &fake,
            "#!/bin/sh\nprintf 'If the browser didn'\"'\"'t open, visit: https://claude.com/cai/oauth/authorize?state=x\\nPaste code here if prompted > '\n\
             read code\n[ \"$code\" = good#code ] || { echo 'OAuth error: invalid code'; exit 1; }\n\
             mkdir -p \"$HOME/.claude\" && echo '{}' > \"$HOME/.claude/.credentials.json\"\necho 'Login successful.'\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let h = harness::harness("claude-code").unwrap();
        let sh = |s: &str| {
            let o = std::process::Command::new("sh")
                .arg("-c")
                .arg(s)
                .env("HOME", home.path())
                .output()
                .unwrap();
            String::from_utf8_lossy(&o.stdout).into_owned()
        };
        let state = || decide("claude-code", &parse_probe(&sh(&status_script(h))), false);
        assert_eq!(state(), LoginState::NotSignedIn);
        assert!(sh(&start_script(h).unwrap()).contains("started"));
        let wait = |want: &dyn Fn(&LoginState) -> bool| {
            for _ in 0..100 {
                let s = state();
                if want(&s) {
                    return s;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            state()
        };
        let s = wait(&|s| matches!(s, LoginState::SigningIn { url: Some(_), .. }));
        assert!(
            matches!(&s, LoginState::SigningIn { url: Some(u), paste_code: true, .. } if u.contains("oauth/authorize")),
            "{s:?}"
        );
        assert!(sh(&code_script(h, " good#code ")).contains("sent"));
        let s = wait(&|s| matches!(s, LoginState::SignedIn));
        assert_eq!(s, LoginState::SignedIn);
        // A wrong code fails in plain words.
        std::fs::remove_dir_all(home.path().join(".claude")).unwrap();
        sh(&start_script(h).unwrap());
        wait(&|s| matches!(s, LoginState::SigningIn { url: Some(_), .. }));
        sh(&code_script(h, "bad"));
        let s = wait(&|s| matches!(s, LoginState::Failed { .. }));
        assert!(
            matches!(&s, LoginState::Failed { reason } if reason.contains("invalid code")),
            "{s:?}"
        );
        sh(&cancel_script(h));
    }

    /// The real `claude` and `codex` on this machine, through the same scripts, in a
    /// throwaway home: each prints the link (and Codex its code) Cua shows, and is
    /// stopped before anyone signs in. Run with `--ignored` where both are installed.
    #[cfg(unix)]
    #[test]
    #[ignore = "needs the real claude and codex installed"]
    fn the_real_agents_show_their_sign_in_link() {
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join(".cua/bin");
        std::fs::create_dir_all(&bin).unwrap();
        for cli in ["claude", "codex"] {
            let path = String::from_utf8(
                std::process::Command::new("sh")
                    .args(["-c", &format!("command -v {cli}")])
                    .output()
                    .unwrap()
                    .stdout,
            )
            .unwrap();
            std::os::unix::fs::symlink(path.trim(), bin.join(cli)).unwrap();
        }
        let sh = |s: &str| {
            let o = std::process::Command::new("sh")
                .arg("-c")
                .arg(s)
                .env("HOME", home.path())
                .output()
                .unwrap();
            String::from_utf8_lossy(&o.stdout).into_owned()
        };
        for id in ["openai-codex", "claude-code"] {
            let h = harness::harness(id).unwrap();
            let state = || decide(id, &parse_probe(&sh(&status_script(h))), false);
            assert_eq!(state(), LoginState::NotSignedIn, "{id}");
            sh(&start_script(h).unwrap());
            let mut s = state();
            for _ in 0..300 {
                if matches!(s, LoginState::SigningIn { url: Some(_), .. }) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
                s = state();
            }
            eprintln!("{id}: {s:?}");
            match (id, &s) {
                (
                    "openai-codex",
                    LoginState::SigningIn {
                        url: Some(u),
                        code: Some(c),
                        paste_code: false,
                    },
                ) => {
                    assert!(u.starts_with("https://auth.openai.com/"), "{u}");
                    assert_eq!(c.len(), 10, "{c}");
                }
                (
                    "claude-code",
                    LoginState::SigningIn {
                        url: Some(u),
                        code: None,
                        paste_code: true,
                    },
                ) => {
                    assert!(u.contains("/oauth/authorize"), "{u}");
                }
                _ => panic!("{id}: {s:?}"),
            }
            sh(&cancel_script(h));
            assert_eq!(state(), LoginState::NotSignedIn, "{id} after cancel");
        }
    }
}
