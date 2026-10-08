//! `cua agent keys`: the provider keys agents get, kept in the Keychain by
//! the Cua daemon (what Cua Spaces → Settings → Agents shows).
//!
//! ```text
//! cua agent keys                          # list (provider, variable, last 4, added)
//! pbpaste | cua agent keys set anthropic  # the key comes from stdin, never argv
//! cua agent keys set other --env MISTRAL_API_KEY < key.txt
//! cua agent keys rm ANTHROPIC_API_KEY
//! ```
//!
//! These are the daemon's app methods (`agent_keys.*`), not MCP tools: no
//! agent can read or change a key, and nothing here prints one.

use crate::util::line;
use clap::Subcommand;
use cua_sdk::{Cua, CuaError};
use std::io::{IsTerminal, Read, Write};
use std::sync::Arc;

#[derive(Subcommand, Debug, Clone)]
pub enum KeysCmd {
    /// List the saved keys (never their values).
    #[command(after_help = "Examples:
  cua agent keys ls
  cua --json agent keys ls")]
    Ls,
    /// Add or replace a key, read from stdin. PROVIDER: anthropic, openai or
    /// other (with --env).
    /// Pipe the key in (`pbpaste | cua agent keys set anthropic`); a
    /// terminal is refused so the key isn't echoed.
    #[command(after_help = "Examples:
  cua agent keys set anthropic
  cua agent keys set other --env GEMINI_API_KEY")]
    Set {
        provider: String,
        /// The variable an Other key is given to runs as.
        #[arg(long)]
        env: Option<String>,
    },
    /// Remove the key a run gets as ENV.
    #[command(after_help = "Examples:
  cua agent keys rm ANTHROPIC_API_KEY")]
    Rm { env: String },
}

/// The key from stdin: piped, never typed (a terminal would echo it).
fn read_key(stdin: &mut dyn Read, is_terminal: bool) -> Result<String, CuaError> {
    if is_terminal {
        return Err(CuaError::InvalidArgument(
            "pipe the key in, so it isn't shown or kept in your shell history: \
             pbpaste | cua agent keys set anthropic"
                .into(),
        ));
    }
    let mut s = String::new();
    stdin
        .take(16 * 1024)
        .read_to_string(&mut s)
        .map_err(|e| CuaError::InvalidArgument(format!("reading the key from stdin: {e}")))?;
    let s = s.trim().to_string();
    if s.is_empty() {
        return Err(CuaError::InvalidArgument("no key on stdin".into()));
    }
    Ok(s)
}

pub async fn run(
    cua: &Arc<Cua>,
    cmd: Option<KeysCmd>,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let (method, args) = match cmd.unwrap_or(KeysCmd::Ls) {
        KeysCmd::Ls => ("agent_keys.list", serde_json::json!({})),
        KeysCmd::Set { provider, env } => {
            let value = read_key(&mut std::io::stdin(), std::io::stdin().is_terminal())?;
            let mut a = serde_json::json!({ "provider": provider, "value": value });
            if let Some(env) = env {
                a["env"] = env.into();
            }
            ("agent_keys.set", a)
        }
        KeysCmd::Rm { env } => ("agent_keys.remove", serde_json::json!({ "env": env })),
    };
    let r = cua
        .spaces()
        .call_tool_json(method.into(), Some(args.to_string()))
        .await?;
    if r.is_error {
        let msg = r.text.strip_prefix("error: ").unwrap_or(&r.text).to_string();
        return Err(CuaError::InvalidArgument(msg));
    }
    let report: serde_json::Value = serde_json::from_str(&r.text).unwrap_or_default();
    if json {
        line(out, report.to_string());
        return Ok(0);
    }
    for l in lines(&report) {
        line(out, l);
    }
    Ok(0)
}

/// The report as a person reads it.
fn lines(report: &serde_json::Value) -> Vec<String> {
    let mut v = vec![];
    if report["available"] == false {
        v.push(format!(
            "Keys can't be saved here: {}.",
            report["unavailable"].as_str().unwrap_or("no Keychain")
        ));
    }
    let keys = report["keys"].as_array().cloned().unwrap_or_default();
    if keys.is_empty() {
        v.push("No agent keys saved. Add one with: pbpaste | cua agent keys set anthropic".into());
    }
    for k in keys {
        let last4 = k["last4"].as_str().unwrap_or("");
        v.push(format!(
            "{:<10} {:<24} {}",
            k["provider"].as_str().unwrap_or(""),
            k["env"].as_str().unwrap_or(""),
            if last4.is_empty() {
                "saved".to_string()
            } else {
                format!("****{last4}")
            }
        ));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_comes_only_from_a_pipe() {
        assert!(read_key(&mut "sk".as_bytes(), true).is_err());
        assert!(read_key(&mut "  \n".as_bytes(), false).is_err());
        assert_eq!(
            read_key(&mut "sk-ant-test-0000\n".as_bytes(), false).unwrap(),
            "sk-ant-test-0000"
        );
    }

    #[test]
    fn listing_never_prints_more_than_the_last_four() {
        let r = serde_json::json!({"keys": [{"provider": "anthropic", "env": "ANTHROPIC_API_KEY", "last4": "0000", "added_ms": 1}], "available": true});
        assert_eq!(lines(&r), vec!["anthropic  ANTHROPIC_API_KEY        ****0000"]);
        let r = serde_json::json!({"keys": [], "available": false, "unavailable": "no Keychain"});
        assert!(lines(&r)[0].contains("no Keychain"));
    }
}
