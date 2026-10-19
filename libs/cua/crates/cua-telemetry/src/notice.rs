//! The first-run notice. Nothing is sent from a machine until it has been
//! shown there once (on stderr by the CLI and SDKs, in the onboarding by
//! the Spaces app).

/// The docs page.
pub const DOCS_URL: &str = "https://cua.ai/docs/cua-sdk/concepts/telemetry";

/// Marker file under `$CUA_HOME/telemetry` once the notice was shown.
pub const MARKER: &str = "notice_shown";

/// The notice, as printed.
pub const TEXT: &str = "\
Cua collects anonymous usage data: which commands and features are used, sandbox
types, durations and error categories. Never file paths, names, hostnames, IPs,
prompts, screen content or anything from your Keyvault. Nothing has been sent yet.
  Turn it off:  cua telemetry off   (or DO_NOT_TRACK=1, or CUA_TELEMETRY=0)
  See what is sent:  cua telemetry show-last
  Details: https://cua.ai/docs/cua-sdk/concepts/telemetry";
