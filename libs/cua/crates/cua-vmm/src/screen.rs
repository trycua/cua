//! Whether anyone is at this Mac's screen.
//!
//! macOS asks "Allow ... to find devices on local networks?" on the Mac's
//! own screen only. On a Mac nobody is signed in at, or whose screen is
//! locked (a Mac used over SSH or from another computer), the question
//! can never be answered, so a macOS VM there stays unreachable for ever.
//! Read-only: one `ioreg` call.

use serde::{Deserialize, Serialize};

/// Why nobody can answer a prompt on this Mac's screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unattended {
    /// Nobody is signed in at the screen (the login window, or no screen).
    NobodySignedIn,
    /// Someone is signed in, but the screen is locked.
    Locked,
}

impl Unattended {
    /// How the person would put it, to finish "so nobody can click Allow
    /// because ...".
    pub fn reason(self) -> &'static str {
        match self {
            Self::NobodySignedIn => "nobody is signed in at this Mac's screen",
            Self::Locked => "this Mac's screen is locked",
        }
    }
}

/// What `ioreg -n Root -d1` says about the console (pure).
pub fn parse_console(ioreg: &str) -> Option<Unattended> {
    if !ioreg.contains("IOConsoleLocked") && !ioreg.contains("IOConsoleUsers") {
        // Not the answer we expected: do not guess.
        return None;
    }
    let signed_in = ioreg.contains("\"kCGSSessionOnConsoleKey\"=Yes");
    let locked = ioreg
        .lines()
        .any(|l| l.contains("\"IOConsoleLocked\"") && l.contains("= Yes"));
    if !signed_in {
        Some(Unattended::NobodySignedIn)
    } else if locked {
        Some(Unattended::Locked)
    } else {
        None
    }
}

/// `Some(why)` when this is a Mac nobody can answer an on-screen prompt
/// on; `None` when someone can, on other systems, or when it cannot be
/// told.
pub fn unattended() -> Option<Unattended> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let out = std::process::Command::new("/usr/sbin/ioreg")
        .args(["-n", "Root", "-d1"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_console(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIGNED_IN: &str = r#"    "IOConsoleLocked" = No
    "IOConsoleUsers" = ({"kCGSSessionOnConsoleKey"=Yes,"kCGSSessionUserNameKey"="me"})"#;

    #[test]
    fn someone_signed_in_and_unlocked_can_answer() {
        assert_eq!(parse_console(SIGNED_IN), None);
    }

    #[test]
    fn a_locked_screen_cannot_be_answered() {
        let locked = SIGNED_IN.replace("\"IOConsoleLocked\" = No", "\"IOConsoleLocked\" = Yes");
        assert_eq!(parse_console(&locked), Some(Unattended::Locked));
    }

    #[test]
    fn no_console_user_cannot_answer() {
        let nobody = "    \"IOConsoleLocked\" = No\n    \"IOConsoleUsers\" = ()";
        assert_eq!(parse_console(nobody), Some(Unattended::NobodySignedIn));
        // At the login window the session is not on the console.
        let login = r#""IOConsoleUsers" = ({"kCGSSessionOnConsoleKey"=No})"#;
        assert_eq!(parse_console(login), Some(Unattended::NobodySignedIn));
    }

    #[test]
    fn an_unexpected_answer_is_not_guessed() {
        assert_eq!(parse_console(""), None);
        assert_eq!(parse_console("ioreg: command not found"), None);
    }
}
