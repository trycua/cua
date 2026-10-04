//! `cua telemetry`: see and control anonymous usage telemetry
//! (cua-telemetry).

use crate::util::{self, line};
use clap::Subcommand;
use cua_sdk::CuaError;
use std::io::Write;

/// `cua telemetry` subcommands.
#[derive(Subcommand, Debug, Clone)]
pub enum TelemetryCmd {
    /// Whether telemetry is on, why, and the exact properties every event
    /// carries.
    #[command(after_help = "Examples:
  cua telemetry status
  cua telemetry status --json")]
    Status,
    /// Turn usage telemetry on (writes [telemetry] enabled = \"on\" to
    /// ~/.cua/config.toml).
    #[command(after_help = "Examples:
  cua telemetry on")]
    On,
    /// Turn usage telemetry off (same as `cua config set telemetry off`).
    #[command(after_help = "Examples:
  cua telemetry off")]
    Off,
    /// Print the last events this machine queued or sent, exactly as sent.
    #[command(
        name = "show-last",
        after_help = "Examples:
  cua telemetry show-last
  cua telemetry show-last -n 5 --json"
    )]
    ShowLast {
        /// How many.
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
    /// Delete the anonymous install id and salt (new ones are created on
    /// the next event).
    #[command(
        name = "reset-id",
        after_help = "Examples:
  cua telemetry reset-id"
    )]
    ResetId,
    /// Every event and property that may be sent, with its allowed values.
    #[command(after_help = "Examples:
  cua telemetry schema --json")]
    Schema,
}

fn io(e: impl std::fmt::Display) -> CuaError {
    CuaError::Internal(e.to_string())
}

/// Runs `cua telemetry`.
pub fn run(cmd: TelemetryCmd, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let t = cua_telemetry::global();
    match cmd {
        TelemetryCmd::Status => {
            let s = t.status();
            let envelope = serde_json::Value::Object(t.envelope_preview());
            if json {
                let mut v = serde_json::to_value(&s).map_err(io)?;
                v["envelope"] = envelope;
                util::json_line(out, &v);
            } else {
                line(
                    out,
                    format!(
                        "Usage telemetry: {} ({})",
                        if s.enabled { "on" } else { "off" },
                        s.source
                    ),
                );
                line(
                    out,
                    format!(
                        "Install id: {}",
                        s.install_id.as_deref().unwrap_or("none yet")
                    ),
                );
                line(out, format!("Endpoint: {}", s.endpoint));
                line(out, format!("First-run notice shown: {}", s.notice_shown));
                line(out, format!("Offline spool: {} events", s.spooled));
                line(out, format!("State: {}", s.state_dir));
                line(
                    out,
                    "\nEvery event carries exactly these properties, plus its own:",
                );
                line(
                    out,
                    serde_json::to_string_pretty(&envelope).unwrap_or_default(),
                );
                line(
                    out,
                    format!(
                        "\n`cua telemetry schema` lists every event. Turn it off: cua telemetry off (or DO_NOT_TRACK=1). Details: {}",
                        cua_telemetry::notice::DOCS_URL
                    ),
                );
            }
        }
        TelemetryCmd::On | TelemetryCmd::Off => {
            let on = matches!(cmd, TelemetryCmd::On);
            let path = t.set_enabled(on).map_err(io)?;
            let s = t.status();
            if json {
                util::json_line(out, &serde_json::to_value(&s).map_err(io)?);
            } else {
                line(
                    out,
                    format!(
                        "Usage telemetry turned {} ({}).",
                        if on { "on" } else { "off" },
                        path.display()
                    ),
                );
                if s.enabled != on {
                    line(
                        out,
                        format!(
                            "Note: {} still wins; it is {}.",
                            s.source,
                            if s.enabled { "on" } else { "off" }
                        ),
                    );
                }
            }
        }
        TelemetryCmd::ShowLast { limit } => {
            let last = t.show_last(limit);
            if json {
                util::json_line(out, &serde_json::Value::Array(last));
            } else if last.is_empty() {
                line(
                    out,
                    if t.is_enabled() {
                        "Nothing sent from this machine yet."
                    } else {
                        "Nothing sent: usage telemetry is off."
                    },
                );
            } else {
                for e in last {
                    line(
                        out,
                        format!(
                            "[{}] {}",
                            e["status"].as_str().unwrap_or(""),
                            serde_json::to_string_pretty(&e["payload"]).unwrap_or_default()
                        ),
                    );
                }
            }
        }
        TelemetryCmd::ResetId => {
            let removed = t.reset_id().map_err(io)?;
            if json {
                util::json_line(out, &serde_json::json!({"removed": removed}));
            } else {
                line(
                    out,
                    format!("Install id reset ({} files removed).", removed.len()),
                );
            }
        }
        TelemetryCmd::Schema => {
            let v = cua_telemetry::schema::to_json();
            if json {
                util::json_line(out, &v);
            } else {
                line(out, serde_json::to_string_pretty(&v).unwrap_or_default());
            }
        }
    }
    Ok(0)
}
