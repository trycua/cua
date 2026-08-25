//! Conservative CAPTCHA / bot-challenge detection for semantic browser snapshots.
//!
//! The detector reports visible challenge evidence for caller policy. It does
//! not act on the challenge, and it deliberately ignores a lone occurrence of
//! words such as "captcha" or "turnstile" in ordinary page content.

use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashSet;

const MAX_EVIDENCE_CHARS: usize = 96;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct BrowserChallengeSignal {
    source: &'static str,
    provider: &'static str,
    reason: &'static str,
    evidence: String,
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
            "handling": "caller_policy",
            "message": "A CAPTCHA or bot-verification challenge appears to be present. Route it through the caller's configured challenge handler before issuing another browser action.",
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
        "handling": "caller_policy",
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
                        evidence: Option<String>| {
        let Some(evidence) = evidence else { return };
        if !seen_signals.insert((source, provider, reason, evidence.clone())) {
            return;
        }
        signals.push(BrowserChallengeSignal {
            source,
            provider,
            reason,
            evidence,
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
                "URL identifies Cloudflare challenge infrastructure",
                Some(truncate_evidence(url)),
            );
        }
        if host == "www.google.com" && parsed.path().starts_with("/recaptcha/")
            || host == "www.recaptcha.net" && parsed.path().starts_with("/recaptcha/")
        {
            consider(
                "url",
                "recaptcha",
                60,
                "URL identifies reCAPTCHA challenge infrastructure",
                Some(truncate_evidence(url)),
            );
        }
        if host == "hcaptcha.com" || host.ends_with(".hcaptcha.com") {
            consider(
                "url",
                "hcaptcha",
                60,
                "URL identifies hCaptcha challenge infrastructure",
                Some(truncate_evidence(url)),
            );
        }
        if host == "funcaptcha.com"
            || host.ends_with(".funcaptcha.com")
            || host == "arkoselabs.com"
            || host.ends_with(".arkoselabs.com")
        {
            consider(
                "url",
                "arkose",
                60,
                "URL identifies Arkose challenge infrastructure",
                Some(truncate_evidence(url)),
            );
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
            "page title matches common challenge copy",
            context_evidence(&title, phrase),
        );
    }

    let texts = texts
        .into_iter()
        .map(normalize_text)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>();
    let combined_text = texts.join(" ");
    let challenge_copy = [
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
    .find_map(|phrase| context_evidence(&combined_text, phrase));

    if let Some(evidence) = challenge_copy {
        consider(
            "page_text",
            "generic",
            45,
            "visible page text contains challenge instructions",
            Some(evidence),
        );
        for (provider, marker, reason) in [
            (
                "recaptcha",
                "recaptcha",
                "challenge copy also identifies reCAPTCHA",
            ),
            (
                "hcaptcha",
                "hcaptcha",
                "challenge copy also identifies hCaptcha",
            ),
            (
                "cloudflare_turnstile",
                "turnstile",
                "challenge copy also identifies Cloudflare Turnstile",
            ),
            (
                "arkose",
                "funcaptcha",
                "challenge copy also identifies Arkose / FunCaptcha",
            ),
        ] {
            consider(
                "page_text",
                provider,
                50,
                reason,
                context_evidence(&combined_text, marker),
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

fn context_evidence(haystack: &str, needle: &str) -> Option<String> {
    let match_start = haystack.find(needle)?;
    let mut start = match_start.saturating_sub(MAX_EVIDENCE_CHARS / 2);
    while !haystack.is_char_boundary(start) {
        start += 1;
    }
    let mut end = (match_start + needle.len() + MAX_EVIDENCE_CHARS / 2).min(haystack.len());
    while !haystack.is_char_boundary(end) {
        end -= 1;
    }
    Some(truncate_evidence(haystack[start..end].trim()))
}

fn truncate_evidence(text: &str) -> String {
    text.chars().take(MAX_EVIDENCE_CHARS).collect()
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
        assert_eq!(value["handling"], "caller_policy");
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
    fn evidence_contains_page_context_instead_of_only_the_matcher_needle() {
        let value = browser_challenge_value(
            "https://example.test/login",
            "Account login",
            ["Before continuing, please verify you are human using the widget below."],
        );
        let evidence = value["signals"][0]["evidence"].as_str().unwrap();

        assert!(evidence.contains("before continuing"));
        assert!(evidence.contains("widget below"));
    }
}
