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
    /// For `needs_connect`: a provider id of the Connections list (`anthropic`, `openrouter`),
    /// when the harness has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// The complete error as the harness or provider said it.
    pub message: String,
    /// Sending the same message again can work (after connecting).
    pub retryable: bool,
}

/// One way a provider or harness says "your credential does not work".
struct AuthText {
    /// The harness that says it; empty: any of them (a provider's own words come through every harness).
    harness: &'static str,
    /// Lower-case text to find.
    marker: &'static str,
    /// The provider it names, when the words name one.
    provider: Option<&'static str>,
    /// Definite enough to read a short reply by (a normal answer that talks about authentication
    /// would not say it this way), not only an error.
    reply: bool,
}

const fn any(marker: &'static str, reply: bool) -> AuthText {
    AuthText {
        harness: "",
        marker,
        provider: None,
        reply,
    }
}

/// Every wording of an authentication failure, in one place. Per-harness tests walk it
/// (`every_row_is_read_for_its_harness`): a harness that words it a new way gets a row and a test.
const AUTH_TEXTS: &[AuthText] = &[
    // What the providers say, through any harness.
    any("authentication_failed", true),
    any("authentication required", false),
    any("authentication failed", false),
    any("failed to authenticate", true),
    any("invalid x-api-key", true),
    any("invalid api key", true),
    any("incorrect api key", true),
    any("oauth token", false),
    any("oauth access token", true),
    any("invalid bearer token", true),
    any("unauthorized", false),
    // JSON-RPC's "internal error" that the ACP adapters use for a refused credential.
    any("(code -32000)", false),
    any("code: -32000", false),
    any("\"code\":-32000", false),
    any("\"code\": -32000", false),
    // Claude Code.
    AuthText {
        harness: "claude-code",
        marker: "invalid authentication credentials",
        provider: Some("anthropic"),
        reply: true,
    },
    AuthText {
        harness: "claude-code",
        marker: "please run /login",
        provider: Some("anthropic"),
        reply: true,
    },
    // Codex.
    AuthText {
        harness: "openai-codex",
        marker: "incorrect api key provided",
        provider: Some("openai"),
        reply: true,
    },
    AuthText {
        harness: "openai-codex",
        marker: "refresh token",
        provider: Some("openai"),
        reply: false,
    },
    // Hermes answers a refused key as an ordinary reply, with its own advice.
    AuthText {
        harness: "hermes",
        marker: "not connected to any ai provider",
        provider: None,
        reply: true,
    },
    AuthText {
        harness: "hermes",
        marker: "rejected your api key",
        provider: None,
        reply: true,
    },
    // Hermes' sign-in variant of the same sentence ("Anthropic rejected your sign-in, so the
    // model can't be reached"), from `agent/turn_failure_copy.py`.
    AuthText {
        harness: "hermes",
        marker: "rejected your sign-in",
        provider: None,
        reply: true,
    },
    AuthText {
        harness: "hermes",
        marker: "hermes setup",
        provider: None,
        reply: true,
    },
    AuthText {
        harness: "hermes",
        marker: "no credential in this space",
        provider: None,
        reply: true,
    },
];

/// Statuses that read as a refused credential when the text is the status of an API error.
const AUTH_STATUSES: &[&str] = &["api error: 401", "status 401", "http 401", "error 401"];

/// The longest reply that can be an error in disguise: a real answer is longer than any harness's error sentence.
const MAX_REPLY: usize = 900;

fn says_auth(harness: &str, t: &str, reply_only: bool) -> bool {
    AUTH_TEXTS
        .iter()
        .filter(|a| a.harness.is_empty() || a.harness == harness || harness.is_empty())
        .filter(|a| !reply_only || a.reply)
        .any(|a| t.contains(a.marker))
}

/// Whether `text` is an authentication failure (an error of a turn or a start: every wording counts).
pub fn is_auth_failure(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    says_auth("", &t, false) || AUTH_STATUSES.iter().any(|s| t.contains(s))
}

/// Whether `reply`, a harness's ordinary answer, is really an authentication failure it reported as
/// text (Hermes answers "Anthropic rejected your API key ..." as a successful turn). Only a short
/// reply that says it in a harness's definite words counts, so an answer that talks about
/// authentication, or quotes a 401 it found in someone's code, is left alone.
pub fn is_auth_failure_reply(harness: &str, reply: &str) -> bool {
    let t = reply.trim().to_ascii_lowercase();
    if t.is_empty() || t.chars().count() > MAX_REPLY {
        return false;
    }
    says_auth(harness, &t, true)
        || (AUTH_STATUSES.iter().any(|s| t.contains(s)) && t.contains("oauth"))
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
    // A row that names its provider.
    if let Some(p) = AUTH_TEXTS
        .iter()
        .filter(|a| a.harness == harness || a.harness.is_empty())
        .find(|a| t.contains(a.marker))
        .and_then(|a| a.provider)
    {
        return Some(p.into());
    }
    let from_text = provider_named(&t);
    let own = crate::harness::harness(harness).and_then(|h| h.connect_provider());
    match harness {
        "claude-code" => Some("anthropic".into()),
        "openai-codex" => Some("openai".into()),
        _ => from_text.or(own).map(str::to_string),
    }
}

/// The connectable provider a lower-cased `t` names first: by id, name, Hermes
/// id or an alias of four letters or more (so a model id further on, such as
/// `anthropic/claude-sonnet-5` in an OpenRouter error, does not win).
fn provider_named(t: &str) -> Option<&'static str> {
    let mut best: Option<(usize, &'static str)> = None;
    for p in cua_connections::visible() {
        let names = std::iter::once(p.id.to_string())
            .chain(std::iter::once(p.name.to_ascii_lowercase()))
            .chain((!p.hermes_id.is_empty()).then(|| p.hermes_id.to_string()))
            .chain(
                p.aliases
                    .iter()
                    .filter(|a| a.len() >= 4)
                    .map(|a| a.to_ascii_lowercase()),
            )
            .chain(match p.id {
                // Words an error of these providers uses for them.
                "anthropic" => vec!["claude".to_string()],
                "openai" => vec!["chatgpt".to_string(), "codex".to_string()],
                _ => vec![],
            });
        for n in names {
            if let Some(at) = t.find(&n)
                && best.is_none_or(|(b, _)| at < b)
            {
                best = Some((at, p.id));
            }
        }
    }
    best.map(|(_, id)| id)
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

/// The credential was there and the provider refused it (so signing in again, not connecting for
/// the first time, is the fix).
fn refused(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    [
        "rejected your api key",
        "rejected your sign-in",
        "invalid x-api-key",
        "invalid api key",
        "incorrect api key",
        "oauth access token is invalid",
        "invalid authentication credentials",
        "401",
    ]
    .iter()
    .any(|m| t.contains(m))
}

/// The one short word for a failed agent in a list: Needs reconnect when the provider refused a
/// credential that was there, Not connected when there was none, Stopped for any other failure.
/// The same words as the Connections states.
pub fn status_word(f: &Failure) -> &'static str {
    match f.class {
        FailureClass::NeedsConnect if refused(&f.message) => "Needs reconnect",
        FailureClass::NeedsConnect => "Not connected",
        FailureClass::Error => "Stopped",
    }
}

/// A provider's name for a person.
fn provider_label(provider: Option<&str>) -> &'static str {
    provider
        .and_then(cua_connections::provider)
        .map_or("The provider", |p| p.name)
}

/// What a person reads instead of the raw error, in plain words from the
/// agent's side (it is the agent's reply). The harness's own advice ("run
/// `hermes setup`") is never in it: Cua manages the sign-in, and the complete
/// text stays under Show details.
pub fn plain(f: &Failure, harness: &str) -> String {
    match f.class {
        FailureClass::NeedsConnect => match (harness, f.provider.as_deref()) {
            ("claude-code", _) if refused(&f.message) => "Claude needs you to sign in again.".into(),
            ("openai-codex", _) if refused(&f.message) => "OpenAI needs you to sign in again.".into(),
            ("claude-code", _) => "I can't reach Claude yet. Connect Claude, then try again.".into(),
            ("openai-codex", _) => "I can't reach OpenAI yet. Connect OpenAI, then try again.".into(),
            ("hermes", p) if refused(&f.message) => format!(
                "{} rejected the key Hermes used. Update it, then try again.",
                provider_label(p)
            ),
            // The subscription sign-ins are Claude Code's and Codex's alone.
            ("hermes", _) => "Hermes needs a connected provider. Connect one, then try again. Your Claude subscription sign-in only works in Claude Code.".into(),
            (h, _) => {
                let name = crate::harness::harness(h).map_or("This agent", |h| h.name);
                format!("{name} needs an API key for a provider it can use. Your Claude subscription sign-in only works in Claude Code.")
            }
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
    fn hermes_failures_name_the_provider_the_card_connects() {
        for (t, p) in [
            (
                "OpenRouter rejected your API key (model anthropic/claude-sonnet-5)",
                "openrouter",
            ),
            ("DeepSeek rejected your API key", "deepseek"),
            ("Moonshot rejected your API key", "kimi"),
            ("xAI rejected your API key", "xai"),
            ("Anthropic rejected your API key", "anthropic"),
            ("HTTP 401 from api.openai.com: invalid api key", "openai"),
        ] {
            let f = classify("hermes", t);
            assert_eq!(f.class, FailureClass::NeedsConnect, "{t}");
            assert_eq!(f.provider.as_deref(), Some(p), "{t}");
        }
        let f = classify("hermes", "OpenRouter rejected your API key");
        assert_eq!(
            plain(&f, "hermes"),
            "OpenRouter rejected the key Hermes used. Update it, then try again."
        );
    }

    #[test]
    fn a_failed_agent_reads_with_the_connections_words() {
        let w = |h: &str, t: &str| status_word(&classify(h, t));
        assert_eq!(w("claude-code", "Authentication required"), "Not connected");
        assert_eq!(
            w(
                "claude-code",
                "API Error: 401 OAuth access token is invalid"
            ),
            "Needs reconnect"
        );
        assert_eq!(
            w("hermes", "OpenRouter rejected your API key"),
            "Needs reconnect"
        );
        assert_eq!(
            w("hermes", "Hermes is not connected to any AI provider"),
            "Not connected"
        );
        assert_eq!(w("claude-code", "rate limited, try later"), "Stopped");
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
        let p = plain(&f, "claude-code");
        let h = plain(
            &classify("hermes", "not connected to any AI provider"),
            "hermes",
        );
        assert!(h.starts_with("Hermes needs a connected provider"), "{h}");
        assert!(h.contains("only works in Claude Code"));
        assert!(!p.contains("401") && !p.contains('{'), "{p}");
        assert!(!p.contains('\u{b7}'));
    }

    /// Every row of the table is read for the harness that says it (and a row that is not for a harness
    /// is read for all of them), as an error and, where it is definite, as a reply.
    #[test]
    fn every_row_is_read_for_its_harness() {
        for row in AUTH_TEXTS {
            let harnesses: Vec<&str> = if row.harness.is_empty() {
                vec!["claude-code", "openai-codex", "hermes"]
            } else {
                vec![row.harness]
            };
            for h in harnesses {
                let text = format!("Something failed: {} (please look)", row.marker);
                assert!(is_auth_failure(&text), "{h}: {}", row.marker);
                assert_eq!(
                    is_auth_failure_reply(h, &text),
                    row.reply,
                    "{h} reply: {}",
                    row.marker
                );
                assert_eq!(
                    classify(h, &text).class,
                    FailureClass::NeedsConnect,
                    "{h}: {}",
                    row.marker
                );
            }
        }
        // A harness's own wording is not another harness's.
        assert!(!is_auth_failure_reply(
            "claude-code",
            "run `hermes setup` first"
        ));
    }

    /// The owner's reply, word for word: Hermes answers a refused key as an ordinary turn.
    #[test]
    fn hermes_answering_a_refused_key_as_text_is_a_connect_failure() {
        let reply = "Anthropic rejected your API key, so the model can't be reached. Update it in Settings -> Providers, or run `hermes setup` in a terminal. Provider said: HTTP 401: OAuth access token is invalid.";
        assert!(is_auth_failure_reply("hermes", reply));
        let f = classify("hermes", reply);
        assert_eq!(f.class, FailureClass::NeedsConnect);
        assert_eq!(f.provider.as_deref(), Some("anthropic"));
        assert_eq!(f.message, reply, "the complete text stays for Show details");
        let p = plain(&f, "hermes");
        assert_eq!(
            p,
            "Anthropic rejected the key Hermes used. Update it, then try again."
        );
        // The harness's own advice never reaches the summary.
        assert!(
            !p.contains("hermes setup") && !p.contains("Settings ->") && !p.contains("401"),
            "{p}"
        );
    }

    #[test]
    fn claude_and_codex_say_sign_in_again_when_their_credential_was_refused() {
        let f = classify(
            "claude-code",
            "Internal error: Failed to authenticate. API Error: 401 OAuth access token is invalid",
        );
        assert_eq!(
            plain(&f, "claude-code"),
            "Claude needs you to sign in again."
        );
        let f = classify("openai-codex", "Incorrect API key provided: sk-...abcd");
        assert_eq!(
            plain(&f, "openai-codex"),
            "OpenAI needs you to sign in again."
        );
        // Nothing saved at all is a first connect, not a sign-in again.
        let f = classify("claude-code", "Authentication required");
        assert_eq!(
            plain(&f, "claude-code"),
            "I can't reach Claude yet. Connect Claude, then try again."
        );
    }

    /// An answer that talks about authentication is an answer.
    #[test]
    fn a_normal_reply_is_never_read_as_a_failure() {
        for reply in [
            "I fixed the login bug. The endpoint returned 401 Unauthorized when the token was missing, so I added the header.",
            "Done. Tests pass. The OAuth token refresh now runs before each request.",
            "Here is how authentication works in your app: the API checks the x-api-key header and answers HTTP 401 if it is missing.",
            "",
        ] {
            for h in ["claude-code", "openai-codex", "hermes"] {
                assert!(!is_auth_failure_reply(h, reply), "{h}: {reply}");
            }
        }
        // A long answer that happens to contain the words is an answer too.
        let long = format!("{} not connected to any AI provider", "word ".repeat(400));
        assert!(!is_auth_failure_reply("hermes", &long));
    }
}
