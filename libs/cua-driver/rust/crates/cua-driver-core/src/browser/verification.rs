//! Heuristic CAPTCHA / bot-verification detection for browser snapshots.
//!
//! This is deliberately advisory. It helps an agent pause and hand the visible
//! browser to a human instead of blindly clicking into a site challenge. It
//! does not solve, bypass, or weaken the challenge.

use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashSet;

const MAX_EVIDENCE_CHARS: usize = 96;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct BrowserVerificationSignal {
    provider: &'static str,
    reason: &'static str,
    evidence: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BrowserVerificationObservation {
    provider: &'static str,
    confidence: &'static str,
    signals: Vec<BrowserVerificationSignal>,
}

impl BrowserVerificationObservation {
    pub(crate) fn to_value(&self) -> Value {
        json!({
            "required": true,
            "kind": "captcha_or_bot_verification",
            "provider": self.provider,
            "confidence": self.confidence,
            "next_action": "human_verification",
            "message": "A CAPTCHA or bot-verification challenge appears to be present. Pause automation and ask a human to complete it in the visible browser, then call get_browser_state again.",
            "signals": self.signals,
        })
    }
}

pub(crate) fn no_browser_verification() -> Value {
    json!({ "required": false })
}

pub(crate) fn browser_verification_value<'a>(
    url: &str,
    title: &str,
    texts: impl IntoIterator<Item = &'a str>,
) -> Value {
    detect_browser_verification(url, title, texts)
        .map(|observation| observation.to_value())
        .unwrap_or_else(no_browser_verification)
}

pub(crate) fn detect_browser_verification<'a>(
    url: &str,
    title: &str,
    texts: impl IntoIterator<Item = &'a str>,
) -> Option<BrowserVerificationObservation> {
    let mut signals = Vec::new();
    let mut seen_signals = HashSet::new();
    let mut provider_rank: Option<(&'static str, u8)> = None;

    let mut consider =
        |provider: &'static str, rank: u8, reason: &'static str, evidence: Option<String>| {
            let Some(evidence) = evidence else { return };
            if !seen_signals.insert((provider, reason, evidence.clone())) {
                return;
            }
            signals.push(BrowserVerificationSignal {
                provider,
                reason,
                evidence,
            });
            if provider != "generic"
                && provider_rank
                    .map(|(_, current_rank)| rank > current_rank)
                    .unwrap_or(true)
            {
                provider_rank = Some((provider, rank));
            }
        };

    let url_lower = url.to_ascii_lowercase();
    consider(
        "recaptcha",
        50,
        "url references reCAPTCHA",
        contains_evidence(&url_lower, &["recaptcha", "google.com/recaptcha"]),
    );
    consider(
        "hcaptcha",
        50,
        "url references hCaptcha",
        contains_evidence(&url_lower, &["hcaptcha.com", "hcaptcha"]),
    );
    consider(
        "cloudflare_turnstile",
        50,
        "url references Cloudflare or Turnstile challenge infrastructure",
        contains_evidence(
            &url_lower,
            &[
                "challenges.cloudflare.com",
                "/cdn-cgi/challenge-platform/",
                "turnstile",
                "cf-chl",
            ],
        ),
    );
    consider(
        "arkose",
        50,
        "url references Arkose / FunCaptcha challenge infrastructure",
        contains_evidence(&url_lower, &["arkoselabs", "funcaptcha"]),
    );

    let title_lower = title.to_ascii_lowercase();
    consider(
        "cloudflare_turnstile",
        40,
        "page title matches common Cloudflare verification copy",
        contains_evidence(
            &title_lower,
            &[
                "just a moment",
                "checking your browser",
                "checking if the site connection is secure",
            ],
        ),
    );
    consider(
        "generic",
        10,
        "page title asks for human verification",
        contains_evidence(
            &title_lower,
            &[
                "captcha",
                "verify you are human",
                "human verification",
                "security check",
            ],
        ),
    );

    for text in texts {
        let normalized = normalize_text(text);
        if normalized.is_empty() {
            continue;
        }
        consider(
            "recaptcha",
            40,
            "page text references reCAPTCHA",
            contains_evidence(&normalized, &["recaptcha", "g-recaptcha"]),
        );
        consider(
            "hcaptcha",
            40,
            "page text references hCaptcha",
            contains_evidence(&normalized, &["hcaptcha"]),
        );
        consider(
            "cloudflare_turnstile",
            40,
            "page text references Cloudflare Turnstile",
            contains_evidence(&normalized, &["turnstile", "cf-chl"]),
        );
        consider(
            "arkose",
            40,
            "page text references Arkose / FunCaptcha",
            contains_evidence(&normalized, &["arkose", "funcaptcha"]),
        );
        consider(
            "generic",
            20,
            "page text contains CAPTCHA or bot-verification copy",
            contains_evidence(
                &normalized,
                &[
                    "captcha",
                    "i'm not a robot",
                    "i’m not a robot",
                    "not a robot",
                    "verify you are human",
                    "verify that you are human",
                    "prove you are human",
                    "human verification",
                    "complete the security check",
                    "complete this security check",
                    "browser verification",
                    "checking your browser",
                    "checking if the site connection is secure",
                    "review the security of your connection",
                ],
            ),
        );
    }

    if signals.is_empty() {
        return None;
    }

    signals.truncate(8);
    let provider = provider_rank
        .map(|(provider, _)| provider)
        .unwrap_or("generic");
    let confidence = if provider != "generic" || signals.len() >= 2 {
        "high"
    } else {
        "medium"
    };

    Some(BrowserVerificationObservation {
        provider,
        confidence,
        signals,
    })
}

fn contains_evidence(haystack: &str, needles: &[&str]) -> Option<String> {
    needles
        .iter()
        .find(|needle| haystack.contains(**needle))
        .map(|needle| truncate_evidence(needle))
}

fn normalize_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn truncate_evidence(text: &str) -> String {
    let mut result = String::new();
    for ch in text.chars().take(MAX_EVIDENCE_CHARS) {
        result.push(ch);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_cloudflare_turnstile_from_url_and_copy() {
        let observation = detect_browser_verification(
            "https://challenges.cloudflare.com/cdn-cgi/challenge-platform/h/b/orchestrate/turnstile",
            "Just a moment...",
            ["Verify you are human before continuing"],
        )
        .expect("verification observation");

        assert_eq!(observation.provider, "cloudflare_turnstile");
        assert_eq!(observation.confidence, "high");
        assert!(observation
            .signals
            .iter()
            .any(|signal| signal.provider == "cloudflare_turnstile"));
    }

    #[test]
    fn detects_generic_captcha_copy() {
        let value = browser_verification_value(
            "https://example.test/login",
            "Security check",
            ["Please complete the CAPTCHA to continue."],
        );

        assert_eq!(value["required"], true);
        assert_eq!(value["provider"], "generic");
        assert_eq!(value["next_action"], "human_verification");
    }

    #[test]
    fn ordinary_page_is_not_marked() {
        let value = browser_verification_value(
            "https://example.com/",
            "Example Domain",
            [
                "Example Domain",
                "This domain is for use in illustrative examples.",
            ],
        );

        assert_eq!(value, json!({ "required": false }));
    }
}
