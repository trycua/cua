//! `cua teleport`: move a desktop app session from this machine into a
//! sandbox. The command line is defined here; teleport itself ships with
//! Cua Spaces (source-available): the Cua Spaces build of `cua` runs it, and
//! this build hands the command to it (see [`crate::extension`]).
//!
//! - `providers` prints the teleportable apps as JSON (ids, app ids, install
//!   probes) so consent UIs never hard-code a list;
//! - `manifest` prints what a teleport would move (JSON);
//! - `push` captures the selection and uploads it to a sandbox (by name) or
//!   a spacesd URL, which imports it and relaunches the app.

use clap::{Args, Subcommand};

#[derive(Subcommand, Debug, Clone)]
pub enum TeleportCmd {
    /// List the apps this machine can teleport (JSON).
    #[command(after_help = "Examples:
  cua teleport providers")]
    Providers,
    /// Describe what teleporting an app would move (JSON).
    #[command(after_help = "Examples:
  cua teleport manifest --app com.google.Chrome --scope tabs")]
    Manifest(AppArgs),
    /// Teleport an app session into a sandbox.
    #[command(after_help = "Examples:
  # Chrome's open tabs into the sandbox dev
  cua teleport push --app com.google.Chrome --scope tabs --sandbox dev
  # The whole Slack profile, every item
  cua teleport push --app Slack --sandbox dev --all")]
    Push(PushArgs),
}

#[derive(Args, Debug, Clone)]
pub struct AppArgs {
    /// App: a bundle id or app name ("com.google.Chrome", "Slack",
    /// "claude-code").
    #[arg(long)]
    pub app: String,
    /// `tabs` (open tabs and session state) or `full` (the whole profile).
    #[arg(long, default_value = "full", value_parser = parse_scope)]
    pub scope: String,
    /// Chrome profile to capture: a name ("Profile 1") or a path.
    #[arg(long)]
    pub profile: Option<String>,
    /// Display name for the app (UI only).
    #[arg(long)]
    pub display_name: Option<String>,
}

#[derive(Args, Debug, Clone)]
pub struct PushArgs {
    #[command(flatten)]
    pub app: AppArgs,
    /// Destination sandbox (name).
    #[arg(
        long,
        short = 's',
        conflicts_with = "url",
        required_unless_present = "url"
    )]
    pub sandbox: Option<String>,
    /// Destination cua-spacesd URL instead of a sandbox.
    #[arg(long)]
    pub url: Option<String>,
    /// Token for `--url`.
    #[arg(long, env = "CUA_ENV_TOKEN", hide_env_values = true, requires = "url")]
    pub token: Option<String>,
    /// Send exactly this manifest item (repeatable; see `manifest`). Default:
    /// the provider's default selection, which leaves credentials-heavy or
    /// bulky items (cookies, transcripts) out unless named here.
    #[arg(long = "include", conflicts_with = "all")]
    pub include: Vec<String>,
    /// Send every item the scope offers.
    #[arg(long)]
    pub all: bool,
    /// Do not launch the app in the sandbox after importing.
    #[arg(long)]
    pub no_launch: bool,
    /// Print `progress <sent> <total>` lines while uploading.
    #[arg(long)]
    pub progress: bool,
    /// Send over a `relay:` destination even though this Space's image
    /// predates end-to-end sealing (S1): without this, such a send refuses
    /// before reading or uploading anything. Set it only after you
    /// understand that relay.cua.ai (or a self-hosted cua-relay) could read
    /// this delivery in the clear; local and direct destinations are
    /// unaffected either way.
    #[arg(long)]
    pub relay_plaintext_ack: bool,
}

/// `tabs` or `full` (also `tabs-only`, `full-profile` and their `_`
/// spellings), normalized to `tabs` or `full`.
fn parse_scope(value: &str) -> Result<String, String> {
    match value {
        "tabs" | "tabs-only" | "tabs_only" => Ok("tabs".into()),
        "full" | "full-profile" | "full_profile" => Ok("full".into()),
        _ => Err(format!("invalid scope {value:?}; use tabs or full")),
    }
}

#[cfg(test)]
mod tests {
    use crate::Cli;
    use clap::Parser;

    #[test]
    fn push_requires_a_destination() {
        let err = Cli::try_parse_from(["cua", "teleport", "push", "--app", "Slack"]).unwrap_err();
        assert!(err.to_string().contains("--sandbox"), "{err}");
        assert!(
            Cli::try_parse_from([
                "cua",
                "teleport",
                "push",
                "--app",
                "x",
                "--url",
                "u",
                "--sandbox",
                "s"
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "cua",
                "teleport",
                "push",
                "--app",
                "x",
                "--sandbox",
                "s",
                "--all",
                "--include",
                "a"
            ])
            .is_err()
        );
    }
}
