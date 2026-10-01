//! `cua config`: the user defaults in `$CUA_HOME/config.toml` (default
//! `~/.cua/config.toml`): where sandboxes run when `--on` is not given,
//! the default kind and runtime, and the cloud defaults. Each value's
//! effective setting and its source (explicit flag > environment > config
//! file > built-in default) is what `list` and `get` print.

use crate::util::{self, line};
use clap::Subcommand;
use cua_sandbox_core::settings::{self, Entry, Settings, Source};
use cua_sdk::CuaError;
use std::io::Write;

/// `cua config` subcommands.
#[derive(Subcommand, Debug, Clone)]
pub enum ConfigCmd {
    /// Every setting: its effective value and where it comes from (env,
    /// config or default).
    #[command(
        visible_alias = "ls",
        after_help = "Examples:
  cua config list
  cua config list --json"
    )]
    List,
    /// One setting's effective value.
    #[command(after_help = "Examples:
  cua config get default.on")]
    Get {
        /// Setting (`default.on`, `default.kind`, `default.runtime`,
        /// `cloud.warm`, `cloud.max_pool_size`, `cloud.claim_ttl`).
        key: String,
    },
    /// Write a setting to the config file.
    #[command(after_help = "Examples:
  # Create sandboxes in the cloud unless --on says otherwise
  cua config set default.on cloud
  # Back to this machine
  cua config set default.on local
  # Prefer VMs when the image offers both
  cua config set default.kind vm")]
    Set {
        /// Setting.
        key: String,
        /// Value (checked: `default.on` takes local, cloud or a provider).
        value: String,
    },
    /// Remove a setting from the config file (the environment variable or
    /// the built-in default applies again).
    #[command(after_help = "Examples:
  cua config unset default.on")]
    Unset {
        /// Setting.
        key: String,
    },
    /// The config file's path.
    #[command(after_help = "Examples:
  cua config path")]
    Path,
}

fn err(e: settings::SettingsError) -> CuaError {
    CuaError::InvalidArgument(e.0)
}

/// An entry as JSON: `key`, `value`, `source` (`env`, `config`,
/// `default`), `env` (the overriding variable), `default`.
pub fn entry_json(e: &Entry) -> serde_json::Value {
    serde_json::json!({
        "key": e.key.name,
        "value": e.value,
        "source": e.source.kind(),
        "from": e.source.to_string(),
        "env": e.key.env,
        "default": e.key.default,
    })
}

/// Runs `cua config`.
pub fn run(cmd: ConfigCmd, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let mut s = Settings::load().map_err(err)?;
    match cmd {
        ConfigCmd::List => {
            let entries = s.list();
            if json {
                let v: Vec<_> = entries.iter().map(entry_json).collect();
                util::json_line(out, &serde_json::Value::Array(v));
            } else {
                let rows: Vec<Vec<String>> = entries
                    .iter()
                    .map(|e| {
                        vec![
                            e.key.name.to_string(),
                            e.value.clone(),
                            source_cell(&e.source),
                        ]
                    })
                    .collect();
                util::table(out, &["KEY", "VALUE", "SOURCE"], &rows);
                line(out, format!("\nConfig file: {}", s.path().display()));
            }
        }
        ConfigCmd::Get { key } => {
            let e = s.entry(settings::key(&key).map_err(err)?);
            if json {
                util::json_line(out, &entry_json(&e));
            } else {
                line(out, &e.value);
            }
        }
        ConfigCmd::Set { key, value } => {
            let k = settings::key(&key).map_err(err)?;
            let v = s.set(k, &value).map_err(err)?;
            let e = s.entry(k);
            if json {
                util::json_line(out, &entry_json(&e));
            } else {
                line(out, format!("{} = {v} ({})", k.name, s.path().display()));
                // The environment still wins; say so rather than surprise.
                if let Source::Env(var) = e.source {
                    line(
                        out,
                        format!(
                            "note: {var}={} is set and overrides it in this shell",
                            e.value
                        ),
                    );
                }
            }
        }
        ConfigCmd::Unset { key } => {
            let k = settings::key(&key).map_err(err)?;
            let removed = s.unset(k).map_err(err)?;
            let e = s.entry(k);
            if json {
                let mut v = entry_json(&e);
                v["removed"] = serde_json::json!(removed);
                util::json_line(out, &v);
            } else if removed {
                line(
                    out,
                    format!("{} unset; now {} ({})", k.name, e.value, e.source),
                );
            } else {
                line(
                    out,
                    format!("{} was not set in {}", k.name, s.path().display()),
                );
            }
        }
        ConfigCmd::Path => line(out, s.path().display().to_string()),
    }
    Ok(0)
}

fn source_cell(s: &Source) -> String {
    match s {
        Source::Config(_) => "config".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_s(cmd: ConfigCmd, json: bool) -> (Result<i32, CuaError>, String) {
        let mut out = Vec::new();
        let r = run(cmd, json, &mut out);
        (r, String::from_utf8(out).unwrap())
    }

    /// set / get / list / unset round trip, with the source of each value
    /// and explicit > env > config > default.
    #[test]
    fn set_get_list_unset_and_precedence() {
        let env = util::test_env::isolated();
        let (_, o) = run_s(
            ConfigCmd::Get {
                key: "default.on".into(),
            },
            false,
        );
        assert_eq!(o.trim(), "local");
        let (r, o) = run_s(
            ConfigCmd::Set {
                key: "default.on".into(),
                value: "cloud".into(),
            },
            false,
        );
        assert_eq!(r.unwrap(), 0);
        assert!(o.starts_with("default.on = cloud ("), "{o}");
        let (_, o) = run_s(ConfigCmd::List, true);
        let v: serde_json::Value = serde_json::from_str(&o).unwrap();
        let on = v
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["key"] == "default.on")
            .unwrap();
        assert_eq!(
            (on["value"].as_str(), on["source"].as_str()),
            (Some("cloud"), Some("config"))
        );
        let kind = v
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["key"] == "default.kind")
            .unwrap();
        assert_eq!(kind["source"], "default");

        // The environment beats the config file, and `set` says so.
        // SAFETY: the test_env guard serializes environment changes.
        unsafe { std::env::set_var("CUA_DEFAULT_ON", "local") };
        let (_, o) = run_s(
            ConfigCmd::Get {
                key: "default.on".into(),
            },
            true,
        );
        let v: serde_json::Value = serde_json::from_str(&o).unwrap();
        assert_eq!(
            (v["value"].as_str(), v["source"].as_str()),
            (Some("local"), Some("env"))
        );
        let (_, o) = run_s(
            ConfigCmd::Set {
                key: "default.on".into(),
                value: "cloud".into(),
            },
            false,
        );
        assert!(
            o.contains("CUA_DEFAULT_ON=local is set and overrides it"),
            "{o}"
        );
        unsafe { std::env::remove_var("CUA_DEFAULT_ON") };

        let (_, o) = run_s(
            ConfigCmd::Unset {
                key: "default.on".into(),
            },
            false,
        );
        assert!(o.contains("default.on unset; now local (default)"), "{o}");
        let (_, o) = run_s(ConfigCmd::Path, false);
        assert!(o.trim().ends_with("config.toml"));
        drop(env);
    }

    #[test]
    fn bad_keys_and_values_are_usage_errors() {
        let _env = util::test_env::isolated();
        let (r, _) = run_s(
            ConfigCmd::Set {
                key: "default.on".into(),
                value: "qemu".into(),
            },
            false,
        );
        let e = r.unwrap_err();
        assert!(matches!(e, CuaError::InvalidArgument(_)));
        assert!(e.to_string().contains("--runtime qemu"), "{e}");
        let (r, _) = run_s(
            ConfigCmd::Get {
                key: "default.where".into(),
            },
            false,
        );
        assert!(r.unwrap_err().to_string().contains("default.on"));
    }
}
