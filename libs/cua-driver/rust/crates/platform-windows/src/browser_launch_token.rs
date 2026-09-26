//! Pure decision logic for the token that runs a Windows isolated browser.
//!
//! Cua Driver launches an isolated Chrome or Edge only from a vendor-signed
//! installation that the token running the browser cannot modify. An
//! elevated token (an elevated administrator, the built-in Administrator, or
//! any administrator when UAC is off) can modify `Program Files`, so such a
//! Driver never runs the browser with its own token. It derives a standard
//! user token from its own token instead: the same user, logon session, and
//! desktop, with `BUILTIN\Administrators` and the other groups that
//! `runas /trustlevel:0x20000` disables set to deny-only, administrative
//! privileges removed, and Medium integrity, marked as a UAC-filtered token.
//! Every protection check is then made for that derived token, and the
//! browser runs with it.
//!
//! The Win32 adapter gathers token facts and this module decides. It has no
//! Win32 dependencies so its unit tests run on any host.

/// Mandatory integrity level RIDs (`SECURITY_MANDATORY_*_RID`).
pub(crate) const MEDIUM_INTEGRITY_RID: u32 = 0x2000;
pub(crate) const HIGH_INTEGRITY_RID: u32 = 0x3000;

/// How a token holds `BUILTIN\Administrators` (S-1-5-32-544).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AdministratorsMembership {
    Absent,
    /// Present only for deny access control entries, as in a UAC-filtered or
    /// standard-user token.
    DenyOnly,
    /// Present and usable for allow access control entries.
    Enabled,
}

/// Security-relevant facts about one access token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TokenFacts {
    /// `TokenElevation` reported the token as elevated.
    pub elevated: bool,
    pub integrity_rid: u32,
    pub administrators: AdministratorsMembership,
    /// Every privilege present in the token, enabled or not, by name.
    pub privileges: Vec<String>,
}

/// The token that runs a driver-owned isolated browser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BrowserLaunchToken {
    /// The Driver's own token. Used whenever the Driver is not elevated, so
    /// that configuration behaves exactly as before.
    Driver,
    /// A standard-user token derived from the elevated Driver's token.
    StandardUser,
}

/// Privileges an ordinary standard-user token holds. A derived token that
/// keeps any other privilege is refused.
pub(crate) const STANDARD_USER_PRIVILEGES: &[&str] = &[
    "SeChangeNotifyPrivilege",
    "SeShutdownPrivilege",
    "SeUndockPrivilege",
    "SeIncreaseWorkingSetPrivilege",
    "SeTimeZonePrivilege",
];

/// Choose the browser token for a Driver with these token facts.
pub(crate) fn browser_launch_token(driver: &TokenFacts) -> BrowserLaunchToken {
    if driver.elevated
        || driver.administrators == AdministratorsMembership::Enabled
        || driver.integrity_rid >= HIGH_INTEGRITY_RID
    {
        BrowserLaunchToken::StandardUser
    } else {
        BrowserLaunchToken::Driver
    }
}

/// Privileges present in a derived token that a standard user does not hold.
pub(crate) fn privileges_beyond_standard_user(privileges: &[String]) -> Vec<&str> {
    privileges
        .iter()
        .map(String::as_str)
        .filter(|name| !STANDARD_USER_PRIVILEGES.contains(name))
        .collect()
}

/// Require the derived browser token to be a standard-user token that sits
/// below the Driver. Integrity ordering matters: Windows forbids a lower
/// integrity process from writing to a higher integrity process, so the
/// browser cannot tamper with the elevated Driver process.
pub(crate) fn validate_standard_user_token(
    driver: &TokenFacts,
    derived: &TokenFacts,
) -> Result<(), String> {
    if derived.elevated {
        return Err("the derived token is still elevated".to_owned());
    }
    if derived.administrators == AdministratorsMembership::Enabled {
        return Err("the derived token still has BUILTIN\\Administrators enabled".to_owned());
    }
    if derived.integrity_rid > MEDIUM_INTEGRITY_RID {
        return Err(format!(
            "the derived token has integrity level 0x{:04x}, above Medium",
            derived.integrity_rid
        ));
    }
    if derived.integrity_rid >= driver.integrity_rid {
        return Err(format!(
            "the derived token integrity level 0x{:04x} is not below the Driver's 0x{:04x}, so \
             Windows would not stop the browser from writing to the Driver process",
            derived.integrity_rid, driver.integrity_rid
        ));
    }
    let extra = privileges_beyond_standard_user(&derived.privileges);
    if !extra.is_empty() {
        return Err(format!(
            "the derived token keeps privileges a standard user does not hold: {}",
            extra.join(", ")
        ));
    }
    Ok(())
}

/// Refusal for an elevated Driver that cannot produce a verified
/// standard-user token. The Driver never falls back to its elevated token.
pub(crate) fn standard_user_token_unavailable_message(reason: &str) -> String {
    format!(
        "Cua Driver is running elevated and could not prepare a verified standard-user token \
         for the isolated browser ({reason}); it refuses to run the browser with its elevated \
         token. Run Cua Driver from a non-elevated session to use isolated browsers"
    )
}

const BACKSLASH: u16 = b'\\' as u16;
const QUOTE: u16 = b'"' as u16;

/// Build a `CreateProcess` command line whose `CommandLineToArgvW` parse is
/// exactly `program` followed by `args`. Works on UTF-16 so no path is lossily
/// converted. The program is always quoted and must not contain a quote.
pub(crate) fn windows_command_line(program: &[u16], args: &[Vec<u16>]) -> Result<Vec<u16>, String> {
    if program.is_empty() || program.contains(&QUOTE) {
        return Err("the browser executable path cannot be quoted safely".to_owned());
    }
    let mut line = Vec::with_capacity(program.len() + 2);
    line.push(QUOTE);
    line.extend_from_slice(program);
    line.push(QUOTE);
    for arg in args {
        if arg.contains(&0) {
            return Err("a browser argument contains a NUL character".to_owned());
        }
        line.push(b' ' as u16);
        append_quoted_argument(&mut line, arg);
    }
    Ok(line)
}

fn append_quoted_argument(line: &mut Vec<u16>, arg: &[u16]) {
    let needs_quotes = arg.is_empty()
        || arg
            .iter()
            .any(|&unit| matches!(unit, 0x20 | 0x09 | 0x0a | 0x0b) || unit == QUOTE);
    if needs_quotes {
        line.push(QUOTE);
    }
    let mut backslashes = 0usize;
    for &unit in arg {
        if unit == BACKSLASH {
            backslashes += 1;
        } else {
            if unit == QUOTE {
                // Double the preceding backslashes and escape the quote.
                line.extend(std::iter::repeat_n(BACKSLASH, backslashes + 1));
            }
            backslashes = 0;
        }
        line.push(unit);
    }
    if needs_quotes {
        // Trailing backslashes before the closing quote must be doubled.
        line.extend(std::iter::repeat_n(BACKSLASH, backslashes));
        line.push(QUOTE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(
        elevated: bool,
        integrity_rid: u32,
        administrators: AdministratorsMembership,
        privileges: &[&str],
    ) -> TokenFacts {
        TokenFacts {
            elevated,
            integrity_rid,
            administrators,
            privileges: privileges.iter().map(|name| (*name).to_owned()).collect(),
        }
    }

    fn elevated_admin() -> TokenFacts {
        facts(
            true,
            HIGH_INTEGRITY_RID,
            AdministratorsMembership::Enabled,
            &[
                "SeChangeNotifyPrivilege",
                "SeDebugPrivilege",
                "SeBackupPrivilege",
                "SeImpersonatePrivilege",
            ],
        )
    }

    fn standard_user() -> TokenFacts {
        facts(
            false,
            MEDIUM_INTEGRITY_RID,
            AdministratorsMembership::DenyOnly,
            &["SeChangeNotifyPrivilege", "SeShutdownPrivilege"],
        )
    }

    #[test]
    fn non_elevated_driver_keeps_its_own_token() {
        // A standard user, and an administrator's UAC-filtered token.
        for driver in [
            facts(
                false,
                MEDIUM_INTEGRITY_RID,
                AdministratorsMembership::Absent,
                &["SeChangeNotifyPrivilege"],
            ),
            standard_user(),
        ] {
            assert_eq!(browser_launch_token(&driver), BrowserLaunchToken::Driver);
        }
    }

    #[test]
    fn elevated_driver_uses_a_standard_user_token() {
        // Elevated administrator with UAC on.
        assert_eq!(
            browser_launch_token(&elevated_admin()),
            BrowserLaunchToken::StandardUser
        );
        // Built-in Administrator or UAC off: Administrators enabled even if
        // TokenElevation were to report false.
        let uac_off = facts(
            false,
            HIGH_INTEGRITY_RID,
            AdministratorsMembership::Enabled,
            &[],
        );
        assert_eq!(
            browser_launch_token(&uac_off),
            BrowserLaunchToken::StandardUser
        );
        // Each signal alone is enough.
        let admins_only = facts(
            false,
            MEDIUM_INTEGRITY_RID,
            AdministratorsMembership::Enabled,
            &[],
        );
        assert_eq!(
            browser_launch_token(&admins_only),
            BrowserLaunchToken::StandardUser
        );
        let high_only = facts(
            false,
            HIGH_INTEGRITY_RID,
            AdministratorsMembership::DenyOnly,
            &[],
        );
        assert_eq!(
            browser_launch_token(&high_only),
            BrowserLaunchToken::StandardUser
        );
        let system = facts(false, 0x4000, AdministratorsMembership::DenyOnly, &[]);
        assert_eq!(
            browser_launch_token(&system),
            BrowserLaunchToken::StandardUser
        );
        let elevated_only = facts(
            true,
            MEDIUM_INTEGRITY_RID,
            AdministratorsMembership::DenyOnly,
            &[],
        );
        assert_eq!(
            browser_launch_token(&elevated_only),
            BrowserLaunchToken::StandardUser
        );
    }

    #[test]
    fn verified_standard_user_token_is_accepted() {
        assert_eq!(
            validate_standard_user_token(&elevated_admin(), &standard_user()),
            Ok(())
        );
        let mut absent = standard_user();
        absent.administrators = AdministratorsMembership::Absent;
        assert_eq!(
            validate_standard_user_token(&elevated_admin(), &absent),
            Ok(())
        );
        let all_standard = facts(
            false,
            MEDIUM_INTEGRITY_RID,
            AdministratorsMembership::DenyOnly,
            STANDARD_USER_PRIVILEGES,
        );
        assert_eq!(
            validate_standard_user_token(&elevated_admin(), &all_standard),
            Ok(())
        );
    }

    #[test]
    fn derived_token_must_not_be_elevated_or_administrator() {
        let mut elevated = standard_user();
        elevated.elevated = true;
        assert!(validate_standard_user_token(&elevated_admin(), &elevated)
            .unwrap_err()
            .contains("still elevated"));
        let mut admin = standard_user();
        admin.administrators = AdministratorsMembership::Enabled;
        assert!(validate_standard_user_token(&elevated_admin(), &admin)
            .unwrap_err()
            .contains("Administrators enabled"));
    }

    #[test]
    fn derived_token_must_be_medium_or_lower_and_below_the_driver() {
        let mut high = standard_user();
        high.integrity_rid = HIGH_INTEGRITY_RID;
        assert!(validate_standard_user_token(&elevated_admin(), &high)
            .unwrap_err()
            .contains("above Medium"));
        let mut medium_plus = standard_user();
        medium_plus.integrity_rid = 0x2100;
        assert!(validate_standard_user_token(&elevated_admin(), &medium_plus).is_err());
        // An elevated-by-group Driver at Medium integrity cannot be protected
        // from a Medium browser by integrity ordering.
        let medium_admin_driver = facts(
            false,
            MEDIUM_INTEGRITY_RID,
            AdministratorsMembership::Enabled,
            &[],
        );
        assert!(
            validate_standard_user_token(&medium_admin_driver, &standard_user())
                .unwrap_err()
                .contains("not below the Driver")
        );
    }

    #[test]
    fn derived_token_must_not_keep_administrative_privileges() {
        for privilege in [
            "SeDebugPrivilege",
            "SeBackupPrivilege",
            "SeRestorePrivilege",
            "SeTakeOwnershipPrivilege",
            "SeImpersonatePrivilege",
            "SeCreateGlobalPrivilege",
            "SeLoadDriverPrivilege",
        ] {
            let mut derived = standard_user();
            derived.privileges.push(privilege.to_owned());
            let error = validate_standard_user_token(&elevated_admin(), &derived).unwrap_err();
            assert!(error.contains(privilege), "{error}");
        }
        assert_eq!(
            privileges_beyond_standard_user(&elevated_admin().privileges),
            vec![
                "SeDebugPrivilege",
                "SeBackupPrivilege",
                "SeImpersonatePrivilege"
            ]
        );
    }

    #[test]
    fn unavailable_token_message_refuses_the_elevated_fallback() {
        let message = standard_user_token_unavailable_message("SaferCreateLevel failed");
        assert!(message.contains("running elevated"));
        assert!(message.contains("SaferCreateLevel failed"));
        assert!(message.contains("refuses to run the browser with its elevated token"));
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    fn command_line(program: &str, args: &[&str]) -> Result<String, String> {
        let args = args.iter().map(|arg| wide(arg)).collect::<Vec<_>>();
        windows_command_line(&wide(program), &args).map(|line| String::from_utf16(&line).unwrap())
    }

    /// Reference `CommandLineToArgvW` argument parser (post-2008 rules) used
    /// to prove the quoting round-trips.
    fn parse_arguments(line: &str) -> Vec<String> {
        let chars = line.chars().collect::<Vec<_>>();
        let mut index = 0;
        // Program name: quoted or up to whitespace; no escape processing.
        if chars.first() == Some(&'"') {
            index = 1;
            while index < chars.len() && chars[index] != '"' {
                index += 1;
            }
            index += 1;
        } else {
            while index < chars.len() && chars[index] != ' ' && chars[index] != '\t' {
                index += 1;
            }
        }
        let mut arguments = Vec::new();
        loop {
            while index < chars.len() && (chars[index] == ' ' || chars[index] == '\t') {
                index += 1;
            }
            if index >= chars.len() {
                return arguments;
            }
            let mut current = String::new();
            let mut quoted = false;
            while index < chars.len() {
                let ch = chars[index];
                if ch == '\\' {
                    let start = index;
                    while index < chars.len() && chars[index] == '\\' {
                        index += 1;
                    }
                    let count = index - start;
                    if index < chars.len() && chars[index] == '"' {
                        current.extend(std::iter::repeat_n('\\', count / 2));
                        if count % 2 == 1 {
                            current.push('"');
                            index += 1;
                        }
                    } else {
                        current.extend(std::iter::repeat_n('\\', count));
                    }
                    continue;
                }
                if ch == '"' {
                    if quoted && chars.get(index + 1) == Some(&'"') {
                        current.push('"');
                        index += 2;
                        continue;
                    }
                    quoted = !quoted;
                    index += 1;
                    continue;
                }
                if !quoted && (ch == ' ' || ch == '\t') {
                    break;
                }
                current.push(ch);
                index += 1;
            }
            arguments.push(current);
        }
    }

    #[test]
    fn command_line_round_trips_browser_arguments() {
        let program = r"C:\Program Files\Google\Chrome\Application\chrome.exe";
        let args = [
            "--remote-debugging-port=0",
            r"--user-data-dir=C:\Users\Jane Doe\AppData\Local\CuaDriver\BrowserProfiles\isolated-1",
            "",
            r"trailing\",
            r"spaced trailing\\",
            r#"quote"inside"#,
            r#"back\"slash"#,
            "tab\there",
            "about:blank",
        ];
        let line = command_line(program, &args).expect("quotable");
        assert!(line.starts_with(&format!("\"{program}\" ")));
        assert_eq!(parse_arguments(&line), args);
    }

    #[test]
    fn simple_arguments_stay_unquoted() {
        assert_eq!(
            command_line(r"C:\b.exe", &["--no-first-run", r"C:\p\q"]).unwrap(),
            r#""C:\b.exe" --no-first-run C:\p\q"#
        );
    }

    #[test]
    fn command_line_rejects_unquotable_input() {
        assert!(command_line(r#"C:\bad"name.exe"#, &[]).is_err());
        assert!(command_line("", &[]).is_err());
        let nul = vec![wide("a\0b")];
        assert!(windows_command_line(&wide(r"C:\b.exe"), &nul).is_err());
    }
}
