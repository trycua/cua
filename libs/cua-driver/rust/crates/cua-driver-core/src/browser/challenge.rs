//! Conservative CAPTCHA / bot-challenge detection for semantic browser snapshots.
//!
//! The detector reports fixed challenge classifications for explicit caller
//! resume or user handoff. It does not copy page text into the classification,
//! act on the challenge, or treat a lone word such as "captcha" or "turnstile"
//! in ordinary page content as a blocker.

use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct BrowserChallengeSignal {
    source: &'static str,
    provider: &'static str,
    reason: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BrowserChallengeObservation {
    origin: String,
    provider: &'static str,
    confidence: &'static str,
    signals: Vec<BrowserChallengeSignal>,
}

impl BrowserChallengeObservation {
    pub(crate) fn to_value(&self) -> Value {
        json!({
            "required": true,
            "kind": "anti_bot_challenge",
            "origin": self.origin,
            "provider": self.provider,
            "confidence": self.confidence,
            "requires_user": true,
            "handling": "explicit_resume_or_user_handoff",
            "message": "A CAPTCHA or bot-verification challenge appears to be present. Do not issue another action to this origin until the caller explicitly resumes or a user takes over.",
            "signals": self.signals,
        })
    }
}

pub(crate) fn no_browser_challenge(origin: &str) -> Value {
    json!({
        "required": false,
        "kind": Value::Null,
        "origin": origin,
        "provider": Value::Null,
        "confidence": Value::Null,
        "requires_user": false,
        "handling": "none",
        "message": Value::Null,
        "signals": [],
    })
}

pub(crate) fn browser_challenge_value<'a>(
    url: &str,
    title: &str,
    texts: impl IntoIterator<Item = &'a str>,
) -> Value {
    let origin = browser_origin(url);
    detect_browser_challenge(url, title, texts)
        .map(|observation| observation.to_value())
        .unwrap_or_else(|| no_browser_challenge(&origin))
}

pub(crate) fn detect_browser_challenge<'a>(
    url: &str,
    title: &str,
    texts: impl IntoIterator<Item = &'a str>,
) -> Option<BrowserChallengeObservation> {
    let origin = browser_origin(url);
    let mut signals = Vec::new();
    let mut seen_signals = HashSet::new();
    let mut provider_rank: Option<(&'static str, u8)> = None;
    let mut strongest_signal = 0_u8;

    let mut consider = |source: &'static str,
                        provider: &'static str,
                        rank: u8,
                        reason: &'static str,
                        matched: bool| {
        if !matched || !seen_signals.insert((source, provider, reason)) {
            return;
        }
        signals.push(BrowserChallengeSignal {
            source,
            provider,
            reason,
        });
        strongest_signal = strongest_signal.max(rank);
        if provider != "generic"
            && provider_rank
                .map(|(_, current_rank)| rank > current_rank)
                .unwrap_or(true)
        {
            provider_rank = Some((provider, rank));
        }
    };

    if let Ok(parsed) = url::Url::parse(url) {
        let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
        let path_and_query = parsed[url::Position::BeforePath..].to_ascii_lowercase();
        if host == "challenges.cloudflare.com"
            || path_and_query.contains("/cdn-cgi/challenge-platform/")
            || path_and_query.contains("cf_chl_")
            || path_and_query.contains("cf-chl-")
        {
            consider(
                "url",
                "cloudflare_turnstile",
                60,
                "challenge_infrastructure",
                true,
            );
        }
        if host == "www.google.com" && parsed.path().starts_with("/recaptcha/")
            || host == "www.recaptcha.net" && parsed.path().starts_with("/recaptcha/")
        {
            consider("url", "recaptcha", 60, "challenge_infrastructure", true);
        }
        if host == "www.google.com" && parsed.path().starts_with("/sorry/") {
            consider("url", "generic", 60, "challenge_infrastructure", true);
        }
        if host == "hcaptcha.com" || host.ends_with(".hcaptcha.com") {
            consider("url", "hcaptcha", 60, "challenge_infrastructure", true);
        }
        if host == "funcaptcha.com"
            || host.ends_with(".funcaptcha.com")
            || host == "arkoselabs.com"
            || host.ends_with(".arkoselabs.com")
        {
            consider("url", "arkose", 60, "challenge_infrastructure", true);
        }
    }

    let title = normalize_text(title);
    for phrase in [
        "checking your browser",
        "checking if the site connection is secure",
        "verify you are human",
    ] {
        consider(
            "title",
            "generic",
            45,
            "challenge_copy",
            title.contains(phrase),
        );
    }

    let texts = texts
        .into_iter()
        .map(normalize_text)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>();
    let combined_text = texts.join(" ");
    let challenge_copy_present = [
        "i'm not a robot",
        "i’m not a robot",
        "verify you are human",
        "verify that you are human",
        "prove you are human",
        "complete the security check",
        "complete this security check",
        "checking your browser",
        "checking if the site connection is secure",
        "review the security of your connection",
    ]
    .iter()
    .any(|phrase| combined_text.contains(phrase));

    if challenge_copy_present {
        consider("page_text", "generic", 45, "challenge_copy", true);
        for (provider, marker, reason) in [
            ("recaptcha", "recaptcha", "provider_marker"),
            ("hcaptcha", "hcaptcha", "provider_marker"),
            ("cloudflare_turnstile", "turnstile", "provider_marker"),
            ("arkose", "funcaptcha", "provider_marker"),
        ] {
            consider(
                "page_text",
                provider,
                50,
                reason,
                combined_text.contains(marker),
            );
        }
    }

    if signals.is_empty() {
        return None;
    }

    signals.truncate(8);
    let provider = provider_rank
        .map(|(provider, _)| provider)
        .unwrap_or("generic");
    let confidence = if strongest_signal >= 50 {
        "high"
    } else {
        "medium"
    };

    Some(BrowserChallengeObservation {
        origin,
        provider,
        confidence,
        signals,
    })
}

fn browser_origin(url: &str) -> String {
    let Ok(parsed) = url::Url::parse(url) else {
        return String::new();
    };
    match parsed.origin() {
        url::Origin::Tuple(_, _, _) => parsed.origin().ascii_serialization(),
        url::Origin::Opaque(_) => String::new(),
    }
}

fn normalize_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_cloudflare_turnstile_from_infrastructure_and_copy() {
        let observation = detect_browser_challenge(
            "https://challenges.cloudflare.com/cdn-cgi/challenge-platform/h/b/orchestrate/turnstile",
            "Just a moment...",
            ["Cloudflare Turnstile: Verify you are human before continuing"],
        )
        .expect("challenge observation");

        assert_eq!(observation.origin, "https://challenges.cloudflare.com");
        assert_eq!(observation.provider, "cloudflare_turnstile");
        assert_eq!(observation.confidence, "high");
        assert!(observation
            .signals
            .iter()
            .any(|signal| signal.source == "url"));
    }

    #[test]
    fn detects_generic_human_verification_copy() {
        let value = browser_challenge_value(
            "https://example.test/login",
            "Account login",
            ["Please verify you are human before continuing."],
        );

        assert_eq!(value["required"], true);
        assert_eq!(value["origin"], "https://example.test");
        assert_eq!(value["provider"], "generic");
        assert_eq!(value["requires_user"], true);
        assert_eq!(value["handling"], "explicit_resume_or_user_handoff");
    }

    #[test]
    fn detects_google_unusual_traffic_interstitial_without_a_checkbox() {
        let value = browser_challenge_value(
            "https://www.google.com/sorry/index?continue=https%3A%2F%2Fwww.google.com%2Fsearch",
            "",
            ["Our systems have detected unusual traffic. Please try your request again later."],
        );

        assert_eq!(value["required"], true);
        assert_eq!(value["origin"], "https://www.google.com");
        assert_eq!(value["provider"], "generic");
        assert_eq!(value["confidence"], "high");
        assert_eq!(
            value["signals"],
            json!([{
                "source": "url",
                "provider": "generic",
                "reason": "challenge_infrastructure",
            }])
        );
    }

    #[test]
    fn does_not_treat_other_sites_sorry_pages_as_google_challenges() {
        let value = browser_challenge_value(
            "https://example.test/sorry/index",
            "Sorry about that",
            ["The requested article moved."],
        );

        assert_eq!(value["required"], false);
    }

    #[test]
    fn ordinary_article_about_captcha_and_turnstile_is_not_marked() {
        let value = browser_challenge_value(
            "https://news.example/articles/turnstile-history",
            "How CAPTCHA systems changed the web",
            [
                "This article explains CAPTCHA accessibility tradeoffs.",
                "Cloudflare Turnstile and hCaptcha are two products discussed by researchers.",
            ],
        );

        assert_eq!(value["required"], false);
        assert_eq!(value["origin"], "https://news.example");
    }

    #[test]
    fn challenge_schema_has_a_stable_key_set() {
        let present = browser_challenge_value(
            "https://example.test/login",
            "Verify you are human",
            std::iter::empty(),
        );
        let absent = browser_challenge_value(
            "https://example.test/",
            "Example Domain",
            ["Illustrative examples live here."],
        );

        let mut present_keys = present.as_object().unwrap().keys().collect::<Vec<_>>();
        let mut absent_keys = absent.as_object().unwrap().keys().collect::<Vec<_>>();
        present_keys.sort();
        absent_keys.sort();
        assert_eq!(present_keys, absent_keys);
    }

    #[test]
    fn signals_do_not_echo_page_content_or_url_details() {
        let value = browser_challenge_value(
            "https://example.test/login?secret=do-not-copy",
            "Account login",
            ["Private account name: Alice. Please verify you are human."],
        );
        let signals = value["signals"].as_array().unwrap();
        let serialized = serde_json::to_string(signals).unwrap();

        assert!(!serialized.contains("Alice"));
        assert!(!serialized.contains("secret"));
        assert!(signals
            .iter()
            .all(|signal| signal.get("evidence").is_none()));
        assert_eq!(value["origin"], "https://example.test");
    }
}
