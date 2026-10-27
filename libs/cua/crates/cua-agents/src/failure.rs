//! A failed turn or start, in one typed shape every surface reads.
//!
//! Providers and harnesses each word an authentication failure differently
//! (`401 OAuth token invalid`, JSON-RPC `-32000`, Hermes' "not connected to
//! any AI provider"). [`classify`] turns any of them into
//! [`FailureClass::NeedsConnect`] with the provider whose Connect row fixes
//! it; everything else is [`FailureClass::Error`]. The full text is kept
//! (up to [`MAX_MESSAGE`] characters) so a surface can offer it under "Show
//! details" and never has to clip or show it as the summary.

use serde::{Deserialize, Serialize};

/// The longest message kept (characters).
pub const MAX_MESSAGE: usize = 4096;

/// What kind of failure it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    /// The agent has no working credential: connect (or reconnect) one.
    NeedsConnect,
    /// Anything else.
    Error,
}

/// A failure with its class, the provider to connect, and the full text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub class: FailureClass,
    /// For `needs_connect`: `anthropic` or `openai` (the Connectors row),
    /// when the harness has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// The complete error as the harness or provider said it.
    pub message: String,
    /// Sending the same message again can work (after connecting).
    pub retryable: bool,
}

/// The words providers and harnesses use for "your credential does not work".
const AUTH_MARKERS: &[&str] = &[
    "authentication_failed",
    "authentication required",
    "authentication failed",
    "failed to authenticate",
    "invalid x-api-key",
    "invalid api key",
    "incorrect api key",
    "oauth token",
    "oauth access token",
    "not connected to any ai provider",
    "no credential in this space",
    "invalid bearer token",
    "unauthorized",
    "(code -32000)",
    "code: -32000",
    "\"code\":-32000",
    "\"code\": -32000",
];

/// Whether `text` is an authentication failure.
pub fn is_auth_failure(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    if AUTH_MARKERS.iter().any(|m| t.contains(m)) {
        return true;
    }
    // A bare 401 counts when it is the status of an API error.
    t.contains("api error: 401")
        || t.contains("status 401")
        || t.contains("http 401")
        || t.contains("error 401")
}

fn cap(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= MAX_MESSAGE {
        text.to_string()
    } else {
        let mut s: String = text.chars().take(MAX_MESSAGE - 1).collect();
        s.push('\u{2026}');
        s
    }
}

/// The provider whose Connect row fixes an auth failure of `harness`, read
/// from the text where the harness has more than one.
fn provider_for(harness: &str, text: &str) -> Option<String> {
    let t = text.to_ascii_lowercase();
    let from_text = if t.contains("anthropic") || t.contains("claude") {
        Some("anthropic")
    } else if t.contains("openai") || t.contains("chatgpt") || t.contains("codex") {
        Some("openai")
    } else {
        None
    };
    let own = crate::harness::harness(harness).and_then(|h| h.connect_provider());
    match harness {
        "claude-code" => Some("anthropic".into()),
        "openai-codex" => Some("openai".into()),
        _ => from_text.or(own).map(str::to_string),
    }
}

/// Classifies `text`, the error of a turn or start of `harness`.
pub fn classify(harness: &str, text: &str) -> Failure {
    let message = cap(text);
    if is_auth_failure(text) {
        Failure {
            class: FailureClass::NeedsConnect,
            provider: provider_for(harness, text),
            message,
            retryable: true,
        }
    } else {
        Failure {
            class: FailureClass::Error,
            provider: None,
            message,
            retryable: true,
        }
    }
}

/// What a person reads instead of the raw error, in plain words from the
/// agent's side (it is the agent's reply).
pub fn plain(f: &Failure) -> String {
    match f.class {
        FailureClass::NeedsConnect => match f.provider.as_deref() {
            Some("anthropic") => "I can't reach Claude yet. Connect Claude, then try again.".into(),
            Some("openai") => "I can't reach OpenAI yet. Connect OpenAI, then try again.".into(),
            _ => "I'm not connected to an AI provider yet. Connect one, then try again.".into(),
        },
        FailureClass::Error => "I couldn't finish that.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_failures_in_every_wording_need_a_connect() {
        for (h, t, p) in [
            (
                "claude-code",
                "Internal error: Failed to authenticate. API Error: 401 OAuth access token is invalid",
                "anthropic",
            ),
            ("claude-code", "Authentication required", "anthropic"),
            (
                "openai-codex",
                "Internal error (code -32000) {\"details\":\"x\"}",
                "openai",
            ),
            (
                "hermes",
                "Internal error (code -32603) {\"details\":\"Hermes is not connected to any AI provider\"}",
                "anthropic",
            ),
            (
                "hermes",
                "not connected to any AI provider; add an OpenAI key",
                "openai",
            ),
        ] {
            let f = classify(h, t);
            assert_eq!(f.class, FailureClass::NeedsConnect, "{t}");
            assert_eq!(f.provider.as_deref(), Some(p), "{t}");
            assert_eq!(f.message, t);
        }
    }

    #[test]
    fn other_errors_stay_errors_and_keep_their_text() {
        let f = classify("claude-code", "rate limited, try later");
        assert_eq!(f.class, FailureClass::Error);
        assert_eq!(f.provider, None);
        let long = "x".repeat(10_000);
        let f = classify("claude-code", &long);
        assert_eq!(f.message.chars().count(), MAX_MESSAGE);
    }

    #[test]
    fn plain_words_never_carry_the_raw_error() {
        let f = classify("claude-code", "API Error: 401 {\"error\":\"invalid\"}");
        let p = plain(&f);
        assert!(!p.contains("401") && !p.contains('{'), "{p}");
        assert!(!p.contains('\u{b7}'));
    }
}
