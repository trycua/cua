//! Linking a signed-in install's usage data to its Cua account (CUA-1881).
//!
//! The Spaces API hands a signed-in client an opaque account id
//! (`analytics_id` on `GET /v1/me`): `acct_` and 32 hex, an HMAC of the
//! account the server keeps to itself. It is never the email, the name or
//! the Keycloak subject. The server only hands it out while linking is
//! turned on there (`SPACES_CP_LINK_USAGE`), so that flag is the one switch
//! for every client: with it off, no client links anything.
//!
//! On the client, linking happens only when all of these hold:
//!
//! - usage data is on (every opt-out still wins);
//! - the server offered an account id ([`crate::Telemetry::offer_account`]);
//! - this machine showed the linking notice once: the first-run notice of a
//!   new install (it says so), or the one-time notice existing installs get
//!   ([`crate::Telemetry::acknowledge_link_notice`]);
//! - the process that showed the notice is not the one sending.
//!
//! Then the next event sends one PostHog `$identify` (`distinct_id` the
//! account id, `$anon_distinct_id` the install id), which merges the
//! install's events into the account, and later events carry the account
//! id. Signing out ([`crate::Telemetry::forget_account`]) goes back to a new
//! anonymous install id, like PostHog's `reset`.
//!
//! State lives next to the install id in `$CUA_HOME/telemetry/`.

/// The account id the server offered (one line).
pub const ACCOUNT_FILE: &str = "account";
/// The account id `$identify` was sent for (one line).
pub const LINKED_FILE: &str = "linked";
/// Marker: this machine showed the linking notice.
pub const NOTICE_MARKER: &str = "link_notice_shown";

/// The one-time notice for installs that saw the older "anonymous" wording,
/// as the CLI prints it. The app shows [`NOTICE_TITLE`] and [`NOTICE_BODY`].
pub const NOTICE_TEXT: &str = "\
We've updated how usage data works. Cua collects usage data, like which features
you use and when something goes wrong. Until now, it wasn't connected to your
account. Now, when you're signed in, it's linked to your account through a
random ID, so we can see where people get stuck and fix it. We never see your
files, what you type or your screen.
  Turn it off:  cua telemetry off
  What is collected:  https://cua.ai/docs/cua-sdk/concepts/telemetry";

/// The linking notice as the terminal shows it: two lines.
pub const NOTICE_SHORT: &str = "\
Usage data is now linked to your Cua account through a random ID when you are signed in.
Turn it off: cua telemetry off. Details: https://cua.ai/docs/cua-sdk/concepts/telemetry";

/// The app's one-time notice: title.
pub const NOTICE_TITLE: &str = "We've updated how usage data works";
/// The app's one-time notice: body.
pub const NOTICE_BODY: &str = "Cua collects usage data, like which features you use and when something goes wrong. Until now, it wasn't connected to your account. Now, when you're signed in, it's linked to your account through a random ID, so we can see where people get stuck and fix it. We never see your files, what you type or your screen. You can turn it off at any time.";
/// The app's one-time notice: the button that keeps usage data on.
pub const NOTICE_OK: &str = "Got it";
/// The app's one-time notice: the button that turns usage data off.
pub const NOTICE_TURN_OFF: &str = "Turn off usage data";
/// The app's one-time notice: the link to the docs page.
pub const NOTICE_DETAILS: &str = "What is collected";

/// Whether `id` is an account id the server mints: `acct_` and 32 lowercase
/// hex. Anything else (an email, a subject, a name) is refused.
pub fn valid_account_id(id: &str) -> bool {
    id.len() == 37
        && id.starts_with("acct_")
        && id[5..]
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Where linking stands on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkState {
    /// Usage data is off: nothing is linked or sent.
    Off,
    /// Not signed in, or the server does not offer linking.
    NotOffered,
    /// Offered, but this machine has not shown the linking notice yet.
    NoticeDue,
    /// Linked: events carry the account id.
    Linked,
}

impl LinkState {
    /// Short name (`off`, `not_offered`, `notice_due`, `linked`).
    pub fn as_str(self) -> &'static str {
        match self {
            LinkState::Off => "off",
            LinkState::NotOffered => "not_offered",
            LinkState::NoticeDue => "notice_due",
            LinkState::Linked => "linked",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_server_minted_ids_pass() {
        assert!(valid_account_id("acct_0123456789abcdef0123456789abcdef"));
        for bad in [
            "",
            "acct_",
            "acct_0123456789ABCDEF0123456789abcdef",
            "acct_0123456789abcdef0123456789abcde",
            "acct_0123456789abcdef0123456789abcdef0",
            "user_0123456789abcdef0123456789abcdef",
            "alice@example.com",
            "f3c1a2b4-1111-2222-3333-444455556666",
        ] {
            assert!(!valid_account_id(bad), "{bad}");
        }
    }
}
