//! `cua auth provider ...`: keys of the contrib sandbox providers
//! (`--on e2b`, `--on daytona`, `--on modal`). Values are read from stdin
//! (without echo on a terminal), stored owner-only under
//! `$CUA_HOME/contrib/credentials.json`, and never printed; an environment
//! variable of the same name wins over the stored value.

use crate::util::line;
use cua_contrib::common::{CredentialSource, CredentialStore, Secret, credential};
use cua_sdk::CuaError;
use std::io::{BufRead, IsTerminal, Write};

fn entry(name: &str) -> Result<cua_contrib::ProviderEntry, CuaError> {
    cua_contrib::catalog()
        .into_iter()
        .find(|e| e.name == name)
        .ok_or_else(|| {
            CuaError::InvalidArgument(format!(
                "unknown provider {name:?} (known: {})",
                cua_contrib::catalog()
                    .iter()
                    .map(|e| e.name)
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })
}

/// `cua auth provider ls`.
pub fn list(json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let store = CredentialStore::default();
    let rows: Vec<serde_json::Value> = cua_contrib::catalog()
        .into_iter()
        .map(|e| {
            let source = e
                .credential_env
                .iter()
                .map(|v| match credential(&store, e.name, &[v]) {
                    Some((_, _, CredentialSource::Env)) => format!("{v} (env)"),
                    Some((_, _, CredentialSource::Store)) => format!("{v} (stored)"),
                    None => format!("{v} (missing)"),
                })
                .collect::<Vec<_>>();
            let configured = e
                .credential_env
                .iter()
                .all(|v| credential(&store, e.name, &[v]).is_some());
            serde_json::json!({
                "provider": e.name,
                "built": e.built,
                "configured": configured,
                "credentials": source,
            })
        })
        .collect();
    if json {
        line(out, serde_json::to_string_pretty(&rows).unwrap_or_default());
        return Ok(0);
    }
    line(
        out,
        format!(
            "{:<10} {:<6} {:<11} CREDENTIALS",
            "PROVIDER", "BUILT", "CONFIGURED"
        ),
    );
    for r in &rows {
        line(
            out,
            format!(
                "{:<10} {:<6} {:<11} {}",
                r["provider"].as_str().unwrap_or_default(),
                if r["built"].as_bool() == Some(true) {
                    "yes"
                } else {
                    "no"
                },
                if r["configured"].as_bool() == Some(true) {
                    "yes"
                } else {
                    "no"
                },
                r["credentials"]
                    .as_array()
                    .map(|a| a
                        .iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(", "))
                    .unwrap_or_default()
            ),
        );
    }
    if rows.iter().any(|r| r["built"].as_bool() != Some(true)) {
        line(
            out,
            "note: providers marked BUILT no need a cua build with `--features contrib`",
        );
    }
    Ok(0)
}

/// `cua auth provider set NAME [--var VAR]`: stores one credential read
/// from stdin.
pub fn set(name: &str, var: Option<&str>, out: &mut dyn Write) -> Result<i32, CuaError> {
    let e = entry(name)?;
    let vars: Vec<&str> = match var {
        Some(v) if e.credential_env.contains(&v) => vec![v],
        Some(v) => {
            return Err(CuaError::InvalidArgument(format!(
                "{name} takes {} (not {v})",
                e.credential_env.join(", ")
            )));
        }
        None => e.credential_env.to_vec(),
    };
    let store = CredentialStore::default();
    for v in vars {
        let value = read_secret(&format!("{v} for {name}: "))?;
        if value.is_empty() {
            return Err(CuaError::InvalidArgument(format!("empty {v}")));
        }
        store
            .set(name, v, &Secret::new(value))
            .map_err(|e| CuaError::Internal(e.to_string()))?;
        line(
            out,
            format!("stored {v} for {name} in {}", store.path().display()),
        );
    }
    Ok(0)
}

/// `cua auth provider rm NAME`.
pub fn remove(name: &str, out: &mut dyn Write) -> Result<i32, CuaError> {
    entry(name)?;
    let had = CredentialStore::default()
        .remove(name)
        .map_err(|e| CuaError::Internal(e.to_string()))?;
    line(
        out,
        if had {
            format!("removed the stored {name} credentials")
        } else {
            format!("no stored {name} credentials")
        },
    );
    Ok(0)
}

/// One line from stdin; on a terminal, prompted with echo off.
fn read_secret(prompt: &str) -> Result<String, CuaError> {
    let stdin = std::io::stdin();
    let tty = stdin.is_terminal();
    if tty {
        eprint!("{prompt}");
        let _ = std::io::stderr().flush();
    }
    let _echo = tty.then(EchoOff::new);
    let mut s = String::new();
    stdin
        .lock()
        .read_line(&mut s)
        .map_err(|e| CuaError::Internal(e.to_string()))?;
    if tty {
        eprintln!();
    }
    Ok(s.trim().to_string())
}

/// Turns terminal echo off until dropped (unix; a no-op elsewhere).
struct EchoOff {
    #[cfg(unix)]
    saved: Option<libc::termios>,
}

impl EchoOff {
    fn new() -> Self {
        #[cfg(unix)]
        {
            // SAFETY: tcgetattr/tcsetattr on stdin with a struct they fill.
            unsafe {
                let mut t: libc::termios = std::mem::zeroed();
                if libc::tcgetattr(0, &mut t) != 0 {
                    return Self { saved: None };
                }
                let saved = t;
                t.c_lflag &= !libc::ECHO;
                libc::tcsetattr(0, libc::TCSANOW, &t);
                Self { saved: Some(saved) }
            }
        }
        #[cfg(not(unix))]
        {
            Self {}
        }
    }
}

impl Drop for EchoOff {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(t) = self.saved {
            // SAFETY: restores the attributes read in `new`.
            unsafe {
                libc::tcsetattr(0, libc::TCSANOW, &t);
            }
        }
    }
}
