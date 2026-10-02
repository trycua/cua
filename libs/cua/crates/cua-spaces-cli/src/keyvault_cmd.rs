// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua keyvault` in the Cua Spaces build: one broker request per command
//! over the Keyvault IPC client (see `cua_cli::keyvault_cmd` for the command
//! line and the rules: passphrases are prompted for or read from stdin,
//! never arguments, and never logged or stored).

use std::io::Write;
use std::path::PathBuf;

use cua_cli::keyvault_cmd::{CredentialArgs, KeyvaultCmd};
use cua_cli::util::{self, line};
use cua_keyvault::broker::{InitRequest, Status, UnlockRequest};
use cua_keyvault::client::{ConnectError, KeyvaultClient, ServerCheck};
use cua_keyvault::protector::{MIN_PASSPHRASE_CHARS, ProtectorKind};
use cua_keyvault::{Error as KvError, Zeroizing};
use cua_sdk::CuaError;

fn socket() -> PathBuf {
    cua_home::cua_home().join("keyvault.sock")
}

fn connect_error(e: ConnectError) -> CuaError {
    match e {
        ConnectError::NotRunning(p) => CuaError::Transport(format!(
            "the Cua Keyvault is not running at {} (start the daemon: `cua daemon start`)",
            p.display()
        )),
        ConnectError::Impostor { path, who } => CuaError::Unauthenticated(format!(
            "the process serving {} is not the Cua daemon ({who}); refusing to talk to it",
            path.display()
        )),
        ConnectError::Other(m) => CuaError::Transport(format!("keyvault: {m}")),
    }
}

fn kv_error(e: KvError) -> CuaError {
    let m = e.to_string();
    match e {
        KvError::Unsupported(_) => CuaError::Unsupported(m),
        KvError::Invalid(_) | KvError::WrongCredential | KvError::Locked => {
            CuaError::InvalidArgument(m)
        }
        KvError::NoVault(_) | KvError::NotFound(_) => CuaError::NotFound(m),
        KvError::Forbidden(_) if cfg!(debug_assertions) => CuaError::PermissionDenied(format!(
            "{m} (a development daemon trusts only the binaries named in \
             CUA_KEYVAULT_TEST_REQUIREMENT; add this cua's cdhash)"
        )),
        KvError::Forbidden(_) | KvError::PresenceFailed(_) | KvError::Denied(_) => {
            CuaError::PermissionDenied(m)
        }
        _ => CuaError::Internal(m),
    }
}

async fn client() -> Result<(KeyvaultClient, bool), CuaError> {
    let check = ServerCheck::default_for_build();
    let verified = matches!(check, ServerCheck::Require(_));
    let c = KeyvaultClient::connect(&socket(), check)
        .await
        .map_err(connect_error)?;
    Ok((c, verified))
}

/// Turns terminal echo off on `fd` until dropped.
#[cfg(unix)]
struct EchoOff {
    fd: i32,
    saved: Option<libc::termios>,
}

#[cfg(unix)]
impl EchoOff {
    fn new(fd: i32) -> Self {
        // SAFETY: tcgetattr/tcsetattr on an open terminal fd with a struct
        // they fill.
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut t) != 0 {
                return Self { fd, saved: None };
            }
            let saved = t;
            t.c_lflag &= !libc::ECHO;
            libc::tcsetattr(fd, libc::TCSANOW, &t);
            Self {
                fd,
                saved: Some(saved),
            }
        }
    }
}

#[cfg(unix)]
impl Drop for EchoOff {
    fn drop(&mut self) {
        if let Some(t) = self.saved {
            // SAFETY: restores the attributes read in `new`.
            unsafe {
                libc::tcsetattr(self.fd, libc::TCSANOW, &t);
            }
        }
    }
}

/// Strips the line ending (only the line ending: spaces are part of a
/// passphrase).
fn strip_newline(mut s: Zeroizing<String>) -> Zeroizing<String> {
    while s.ends_with('\n') || s.ends_with('\r') {
        s.pop();
    }
    s
}

/// One line typed at the controlling terminal with echo off.
fn prompt_secret(prompt: &str) -> Result<Zeroizing<String>, CuaError> {
    #[cfg(unix)]
    {
        use std::io::BufRead;
        use std::os::fd::AsRawFd;
        let tty = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .map_err(|_| {
                CuaError::InvalidArgument(
                    "no terminal to prompt for the passphrase on; use --passphrase-stdin".into(),
                )
            })?;
        let mut w = &tty;
        let _ = write!(w, "{prompt}");
        let _ = w.flush();
        let mut s = Zeroizing::new(String::new());
        {
            let _echo = EchoOff::new(tty.as_raw_fd());
            std::io::BufReader::new(&tty)
                .read_line(&mut s)
                .map_err(|e| CuaError::Internal(e.to_string()))?;
        }
        let _ = writeln!(w);
        Ok(strip_newline(s))
    }
    #[cfg(not(unix))]
    {
        let _ = prompt;
        Err(CuaError::Unsupported(
            "the passphrase prompt needs a Unix terminal; use --passphrase-stdin".into(),
        ))
    }
}

/// The first line of stdin.
fn stdin_secret() -> Result<Zeroizing<String>, CuaError> {
    use std::io::BufRead;
    let mut s = Zeroizing::new(String::new());
    std::io::stdin()
        .lock()
        .read_line(&mut s)
        .map_err(|e| CuaError::Internal(e.to_string()))?;
    let s = strip_newline(s);
    if s.is_empty() {
        return Err(CuaError::InvalidArgument(
            "--passphrase-stdin read an empty line".into(),
        ));
    }
    Ok(s)
}

/// The passphrase for `init` (typed twice at a prompt), or `None` for the OS
/// key store.
fn new_passphrase(args: &CredentialArgs) -> Result<Option<Zeroizing<String>>, CuaError> {
    let p = if args.passphrase_stdin {
        stdin_secret()?
    } else if args.passphrase {
        let first = prompt_secret(&format!(
            "New Keyvault passphrase ({MIN_PASSPHRASE_CHARS}+ characters): "
        ))?;
        cua_keyvault::protector::check_new_passphrase(&first).map_err(kv_error)?;
        let again = prompt_secret("Type it again: ")?;
        if *again != *first {
            return Err(CuaError::InvalidArgument(
                "the passphrases don't match".into(),
            ));
        }
        first
    } else {
        return Ok(None);
    };
    cua_keyvault::protector::check_new_passphrase(&p).map_err(kv_error)?;
    Ok(Some(p))
}

fn unlock_passphrase(args: &CredentialArgs) -> Result<Option<Zeroizing<String>>, CuaError> {
    if args.passphrase_stdin {
        stdin_secret().map(Some)
    } else if args.passphrase {
        prompt_secret("Keyvault passphrase: ").map(Some)
    } else {
        Ok(None)
    }
}

fn kind_name(k: ProtectorKind) -> &'static str {
    match k {
        ProtectorKind::MacosKeychain => "macOS keychain",
        ProtectorKind::WindowsCredential => "Windows Credential Manager",
        ProtectorKind::Passphrase => "passphrase",
        ProtectorKind::Recovery => "recovery key",
    }
}

fn state_word(s: &Status) -> &'static str {
    if !s.initialized {
        "not set up"
    } else if s.disabled {
        "turned off"
    } else if s.unlocked {
        "unlocked"
    } else {
        "locked"
    }
}

fn print_status(s: &Status, verified: bool, out: &mut dyn Write) {
    line(out, format!("Keyvault: {}", state_word(s)));
    if !s.initialized {
        line(
            out,
            format!(
                "Setup: {}",
                if s.os_protector_available {
                    "Touch ID (OS key store) or a passphrase"
                } else {
                    "a passphrase (this daemon does not use the OS key store)"
                }
            ),
        );
    }
    if s.initialized && s.caller_first_party {
        let kinds: Vec<&str> = s.unlock_protectors.iter().map(|k| kind_name(*k)).collect();
        line(out, format!("Unlocks with: {}", kinds.join(", ")));
        line(out, format!("Items: {}", s.items));
    }
    line(
        out,
        format!(
            "This cua: {}",
            if s.caller_first_party {
                "first party".to_string()
            } else {
                format!("not first party ({})", s.caller_display)
            }
        ),
    );
    line(
        out,
        format!(
            "Daemon: {}",
            if verified {
                "signature verified"
            } else {
                "not verified (development build)"
            }
        ),
    );
}

/// Runs `cua keyvault`.
pub async fn run(cmd: KeyvaultCmd, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let (mut c, verified) = client().await?;
    match cmd {
        KeyvaultCmd::Status => {
            let s = c.status().await.map_err(kv_error)?;
            if json {
                let mut v =
                    serde_json::to_value(&s).map_err(|e| CuaError::Internal(e.to_string()))?;
                v["server_verified"] = serde_json::Value::Bool(verified);
                util::json_line(out, &v);
            } else {
                print_status(&s, verified, out);
            }
        }
        KeyvaultCmd::Init(args) => {
            let passphrase = new_passphrase(&args)?;
            let os = passphrase.is_none();
            if os {
                // Fail fast, before the daemon asks for Touch ID, with the
                // way that works (the broker refuses the same way).
                let s = c.status().await.map_err(kv_error)?;
                if !s.os_protector_available {
                    return Err(CuaError::Unsupported(
                        "this Cua daemon cannot use the OS key store (a development daemon, \
                         or not the signed Cua daemon); run `cua keyvault init --passphrase`"
                            .into(),
                    ));
                }
            }
            if !json {
                eprintln!(
                    "The Cua daemon asks for Touch ID or your login password to create the Keyvault."
                );
            }
            let key = c
                .init(InitRequest {
                    os_protector: os,
                    passphrase: passphrase.as_ref().map(|p| p.as_str().to_string()),
                    recovery_key: true,
                })
                .await
                .map_err(kv_error)?
                .map(Zeroizing::new);
            drop(passphrase);
            if json {
                util::json_line(
                    out,
                    &serde_json::json!({
                        "created": true,
                        "protector": if os { "os" } else { "passphrase" },
                        "recovery_key": key.as_ref().map(|k| k.as_str()),
                    }),
                );
            } else {
                line(out, "Keyvault created.");
                if let Some(k) = &key {
                    line(
                        out,
                        "Recovery key (shown once; keep it somewhere safe, Cua keeps no copy):",
                    );
                    line(out, format!("  {}", k.as_str()));
                }
            }
        }
        KeyvaultCmd::Unlock(args) => {
            let passphrase = unlock_passphrase(&args)?;
            c.unlock(UnlockRequest {
                passphrase: passphrase.as_ref().map(|p| p.as_str().to_string()),
                recovery_key: None,
            })
            .await
            .map_err(kv_error)?;
            drop(passphrase);
            if json {
                util::json_line(out, &serde_json::json!({ "unlocked": true }));
            } else {
                line(out, "Keyvault unlocked.");
            }
        }
        KeyvaultCmd::Lock => {
            c.lock().await.map_err(kv_error)?;
            if json {
                util::json_line(out, &serde_json::json!({ "locked": true }));
            } else {
                line(out, "Keyvault locked.");
            }
        }
        KeyvaultCmd::ImportPasswords(a) => {
            if !json {
                eprintln!(
                    "The Cua daemon asks for Touch ID or your login password to import saved passwords."
                );
            }
            let r = c
                .import_passwords(cua_keyvault::broker::PasswordImportSpec {
                    app: a.browser,
                    profile: a.profile,
                    sites: a.sites,
                })
                .await
                .map_err(kv_error)?;
            if json {
                util::json_line(out, &serde_json::json!({ "imported": r }));
            } else if r.saved == 0 {
                line(out, "No saved passwords found.");
            } else {
                line(out, import_summary(&r, "saved password"));
            }
        }
        KeyvaultCmd::ImportSession(a) => {
            if !json {
                eprintln!(
                    "The Cua daemon asks for Touch ID or your login password to import a session."
                );
            }
            let sites: Vec<cua_keyvault::broker::SiteChoice> = a
                .sites
                .iter()
                .map(|site| cua_keyvault::broker::SiteChoice {
                    site: site.clone(),
                    include_storage: a.include_storage,
                    include_passwords: a.include_passwords,
                })
                .collect();
            let whole_app = sites.is_empty();
            let spec = cua_keyvault::broker::ImportSpec {
                app: a.app,
                profile: a.profile,
                sites,
                whole_app,
                cookies: cua_keyvault::broker::CookieFilter {
                    session_only: a.session_only,
                    drop_long_lived: a.drop_long_lived,
                },
                confirm_passwords: a.include_passwords,
                paths: None,
                domains: None,
                passwords: false,
            };
            let r = c.import(spec).await.map_err(kv_error)?;
            if json {
                util::json_line(out, &serde_json::json!({ "imported": r }));
            } else if r.saved == 0 {
                line(out, "Nothing to import.");
            } else {
                line(out, import_summary(&r, "item"));
                line(
                    out,
                    "Sealed in the Keyvault, grouped by app. Saving the same app again updates \
                     these items. Deliver them with `cua teleport push --sandbox NAME` \
                     or the review sheet's \"Save to Keyvault\".",
                );
            }
        }
        KeyvaultCmd::Requests => {
            let pending = c.list_pending().await.map_err(kv_error)?;
            if json {
                let rows: Vec<serde_json::Value> = pending
                    .iter()
                    .map(|p| {
                        serde_json::json!({"id": p.id, "caller": p.caller_display,
                            "agent": p.request.agent, "actions": p.request.actions,
                            "targets": p.request.targets,
                            "items": p.items.iter().map(|i| i.redacted().label()).collect::<Vec<_>>()})
                    })
                    .collect();
                util::json_line(out, &serde_json::json!({ "requests": rows }));
            } else if pending.is_empty() {
                line(out, "No requests waiting.");
            } else {
                for p in &pending {
                    let what: Vec<String> = p
                        .items
                        .iter()
                        .map(|i| {
                            i.domain
                                .as_deref()
                                .map(cua_keyvault::record::site_of)
                                .unwrap_or_else(|| i.label())
                        })
                        .collect::<std::collections::BTreeSet<_>>()
                        .into_iter()
                        .collect();
                    let verb = if p.request.actions.contains(&cua_keyvault::Action::Login) {
                        "sign in to"
                    } else {
                        "teleport"
                    };
                    let who = match &p.request.agent {
                        Some(a) => format!("agent {a}"),
                        None => p.caller_display.clone(),
                    };
                    line(
                        out,
                        format!(
                            "{}  {who}: {verb} {} in {}",
                            p.id,
                            what.join(", "),
                            p.request.targets.join(", ")
                        ),
                    );
                }
            }
        }
        KeyvaultCmd::Approve { id } => {
            if !json {
                eprintln!("The Cua daemon asks for Touch ID or your login password to approve.");
            }
            let g = c.approve(&id, Default::default()).await.map_err(kv_error)?;
            if json {
                util::json_line(
                    out,
                    &serde_json::json!({"approved": true, "grant": g.id, "uses_left": g.uses_left}),
                );
            } else {
                line(out, format!("Approved {id}."));
            }
        }
        KeyvaultCmd::Deny { id } => {
            c.deny(&id).await.map_err(kv_error)?;
            if json {
                util::json_line(out, &serde_json::json!({ "denied": true }));
            } else {
                line(out, format!("Declined {id}."));
            }
        }
    }
    Ok(0)
}

/// "Saved 4 items: 3 new, 1 updated" (counts only; never a name or a value).
fn import_summary(r: &cua_keyvault::broker::ImportReport, noun: &str) -> String {
    let plural = if r.saved == 1 { "" } else { "s" };
    let mut parts = Vec::new();
    if r.created > 0 {
        parts.push(format!("{} new", r.created));
    }
    if r.updated > 0 {
        parts.push(format!("{} updated", r.updated));
    }
    if r.unchanged > 0 {
        parts.push(format!("{} unchanged", r.unchanged));
    }
    format!("Saved {} {noun}{plural}: {}", r.saved, parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_line_ending_is_stripped() {
        let s = strip_newline(Zeroizing::new("  two spaces  \r\n".into()));
        assert_eq!(s.as_str(), "  two spaces  ");
    }
}
