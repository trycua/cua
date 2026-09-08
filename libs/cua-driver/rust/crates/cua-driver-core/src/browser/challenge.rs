//! Conservative CAPTCHA / bot-challenge detection for semantic browser snapshots.
//!
//! The detector reports fixed challenge classifications for explicit caller
//! resume or user handoff. It does not copy page text into the classification,
//! act on the challenge, or treat a lone word such as "captcha" or "turnstile"
//! in ordinary page content as a blocker.

use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::time::Duration;
use time::{format_description::well_known::Rfc2822, OffsetDateTime};

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
    pub(crate) fn origin(&self) -> &str {
        &self.origin
    }

    pub(crate) fn to_value(&self) -> Value {
        json!({
            "required": true,
            "detection_status": "detected",
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

pub(crate) fn no_browser_challenge(origin: &str, observation_complete: bool) -> Value {
    json!({
        "required": false,
        "detection_status": if observation_complete { "not_detected" } else { "unknown" },
        "kind": Value::Null,
        "origin": origin,
        "provider": Value::Null,
        "confidence": Value::Null,
        "requires_user": false,
        "handling": "none",
        "message": if observation_complete {
            Value::Null
        } else {
            Value::String(
                "The available page state was incomplete, so the absence of a challenge could not be proven."
                    .to_owned(),
            )
        },
        "signals": [],
    })
}

#[cfg(test)]
pub(crate) fn browser_challenge_value<'a>(
    url: &str,
    texts: impl IntoIterator<Item = &'a str>,
    observation_complete: bool,
) -> Value {
    let origin = browser_origin(url);
    detect_browser_challenge(url, texts)
        .map(|observation| observation.to_value())
        .unwrap_or_else(|| no_browser_challenge(&origin, observation_complete))
}

pub(crate) fn detect_browser_challenge<'a>(
    url: &str,
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
        let path = parsed.path().to_ascii_lowercase();
        let cloudflare_challenge_parameter = parsed.query_pairs().any(|(name, _)| {
            let name = name.to_ascii_lowercase();
            name.starts_with("cf_chl_") || name.starts_with("cf-chl-")
        });
        if host == "challenges.cloudflare.com"
            || path.starts_with("/cdn-cgi/challenge-platform/")
            || cloudflare_challenge_parameter
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
        if (host == "hcaptcha.com" || host.ends_with(".hcaptcha.com"))
            && (path.starts_with("/captcha/")
                || path.starts_with("/checksiteconfig")
                || path.starts_with("/getcaptcha/"))
        {
            consider("url", "hcaptcha", 60, "challenge_infrastructure", true);
        }
        if (host == "funcaptcha.com"
            || host.ends_with(".funcaptcha.com")
            || host == "arkoselabs.com"
            || host.ends_with(".arkoselabs.com"))
            && path.starts_with("/fc/")
        {
            consider("url", "arkose", 60, "challenge_infrastructure", true);
        }
    }

    let texts = texts
        .into_iter()
        .map(normalize_text)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>();
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
    .any(|phrase| texts.iter().any(|text| text.contains(phrase)));

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
                texts.iter().any(|text| text.contains(marker)),
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

pub(crate) fn browser_origin(url: &str) -> String {
    let Ok(parsed) = url::Url::parse(url) else {
        return String::new();
    };
    match parsed.origin() {
        url::Origin::Tuple(_, _, _) => parsed.origin().ascii_serialization(),
        url::Origin::Opaque(_) => String::new(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NavigationResponseObservation {
    pub(crate) status: u16,
    pub(crate) origin: String,
    pub(crate) retry_after: Option<Duration>,
    pub(crate) is_redirect: bool,
}

pub(crate) fn navigation_response_observation(
    params: &Value,
    expected_loader_id: Option<&str>,
    expected_frame_id: Option<&str>,
    now: OffsetDateTime,
) -> Option<NavigationResponseObservation> {
    if params.get("type").and_then(Value::as_str) != Some("Document") {
        return None;
    }
    if expected_loader_id.is_some()
        && params.get("loaderId").and_then(Value::as_str) != expected_loader_id
    {
        return None;
    }
    if expected_frame_id.is_some()
        && params.get("frameId").and_then(Value::as_str) != expected_frame_id
    {
        return None;
    }

    let response = params.get("response")?.as_object()?;
    let status = response.get("status")?.as_f64()?;
    if !(0.0..=u16::MAX as f64).contains(&status) || status.fract() != 0.0 {
        return None;
    }
    let origin = browser_origin(response.get("url")?.as_str()?);
    if origin.is_empty() {
        return None;
    }
    let headers = response.get("headers").and_then(Value::as_object);
    let retry_after = headers
        .and_then(|headers| {
            headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
                .and_then(|(_, value)| match value {
                    Value::String(value) => Some(value.clone()),
                    Value::Number(value) => Some(value.to_string()),
                    _ => None,
                })
        })
        .and_then(|value| parse_retry_after(&value, now));
    let is_redirect = matches!(status as u16, 301 | 302 | 303 | 307 | 308)
        && headers.is_some_and(|headers| {
            headers.iter().any(|(name, value)| {
                name.eq_ignore_ascii_case("location")
                    && value.as_str().is_some_and(|value| !value.is_empty())
            })
        });

    Some(NavigationResponseObservation {
        status: status as u16,
        origin,
        retry_after,
        is_redirect,
    })
}

fn parse_retry_after(value: &str, now: OffsetDateTime) -> Option<Duration> {
    let value = value.trim();
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return value.parse::<u64>().ok().map(Duration::from_secs);
    }

    let retry_at = OffsetDateTime::parse(value, &Rfc2822).ok()?;
    let milliseconds = (retry_at - now).whole_milliseconds().max(0);
    Some(Duration::from_millis(
        u64::try_from(milliseconds).unwrap_or(u64::MAX),
    ))
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
    fn detects_known_hcaptcha_and_arkose_challenge_paths() {
        for (url, provider) in [
            (
                "https://newassets.hcaptcha.com/captcha/v1/example/static/hcaptcha.html",
                "hcaptcha",
            ),
            ("https://client-api.arkoselabs.com/fc/gc/", "arkose"),
        ] {
            let observation = detect_browser_challenge(url, std::iter::empty())
                .expect("known challenge endpoint");
            assert_eq!(observation.provider, provider, "{url}");
        }
    }

    #[test]
    fn detects_generic_human_verification_copy() {
        let value = browser_challenge_value(
            "https://example.test/login",
            ["Please verify you are human before continuing."],
            true,
        );

        assert_eq!(value["required"], true);
        assert_eq!(value["detection_status"], "detected");
        assert_eq!(value["origin"], "https://example.test");
        assert_eq!(value["provider"], "generic");
        assert_eq!(value["requires_user"], true);
        assert_eq!(value["handling"], "explicit_resume_or_user_handoff");
    }

    #[test]
    fn detects_google_unusual_traffic_interstitial_without_a_checkbox() {
        let value = browser_challenge_value(
            "https://www.google.com/sorry/index?continue=https%3A%2F%2Fwww.google.com%2Fsearch",
            ["Our systems have detected unusual traffic. Please try your request again later."],
            true,
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
            ["Sorry about that", "The requested article moved."],
            true,
        );

        assert_eq!(value["required"], false);
    }

    #[test]
    fn ordinary_article_about_captcha_and_turnstile_is_not_marked() {
        let value = browser_challenge_value(
            "https://news.example/articles/turnstile-history",
            [
                "How CAPTCHA systems changed the web",
                "This article explains CAPTCHA accessibility tradeoffs.",
                "Cloudflare Turnstile and hCaptcha are two products discussed by researchers.",
            ],
            true,
        );

        assert_eq!(value["required"], false);
        assert_eq!(value["origin"], "https://news.example");
    }

    #[test]
    fn challenge_markers_in_query_values_do_not_block_an_ordinary_page() {
        let value = browser_challenge_value(
            "https://docs.example/search?q=%2Fcdn-cgi%2Fchallenge-platform%2F+cf_chl_token",
            ["Search results", "Documentation search results"],
            true,
        );

        assert_eq!(value["required"], false);
        assert_eq!(value["origin"], "https://docs.example");
    }

    #[test]
    fn challenge_path_fragments_outside_the_reserved_prefix_are_not_marked() {
        let value = browser_challenge_value(
            "https://docs.example/reference/cdn-cgi/challenge-platform/",
            ["Cloud service documentation"],
            true,
        );

        assert_eq!(value["detection_status"], "not_detected");
    }

    #[test]
    fn challenge_copy_must_be_present_in_one_semantic_text_value() {
        let value = browser_challenge_value(
            "https://example.test/ordinary",
            ["Please verify you are", "human resources policy"],
            true,
        );

        assert_eq!(value["detection_status"], "not_detected");
    }

    #[test]
    fn provider_homepages_and_docs_are_not_challenge_infrastructure() {
        for url in [
            "https://www.hcaptcha.com/",
            "https://docs.hcaptcha.com/configuration/",
            "https://www.arkoselabs.com/resources/",
            "https://developer.funcaptcha.com/docs/",
        ] {
            let value =
                browser_challenge_value(url, ["Documentation", "Product documentation"], true);
            assert_eq!(value["required"], false, "{url}: {value}");
        }
    }

    #[test]
    fn challenge_schema_has_a_stable_key_set() {
        let present =
            browser_challenge_value("https://example.test/login", ["Verify you are human"], true);
        let absent = browser_challenge_value(
            "https://example.test/",
            ["Example Domain", "Illustrative examples live here."],
            true,
        );
        let unknown = browser_challenge_value("https://example.test/", ["Example Domain"], false);

        let mut present_keys = present.as_object().unwrap().keys().collect::<Vec<_>>();
        let mut absent_keys = absent.as_object().unwrap().keys().collect::<Vec<_>>();
        let mut unknown_keys = unknown.as_object().unwrap().keys().collect::<Vec<_>>();
        present_keys.sort();
        absent_keys.sort();
        unknown_keys.sort();
        assert_eq!(present_keys, absent_keys);
        assert_eq!(present_keys, unknown_keys);
        assert_eq!(absent["detection_status"], "not_detected");
        assert_eq!(unknown["detection_status"], "unknown");
        assert_eq!(unknown["required"], false);
        assert!(unknown["message"].as_str().is_some());
    }

    #[test]
    fn signals_do_not_echo_page_content_or_url_details() {
        let value = browser_challenge_value(
            "https://example.test/login?secret=do-not-copy",
            ["Private account name: Alice. Please verify you are human."],
            true,
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

    #[test]
    fn observes_429_and_numeric_retry_after_without_retaining_url_details() {
        let observation = navigation_response_observation(
            &json!({
                "type": "Document",
                "frameId": "frame-1",
                "loaderId": "loader-1",
                "response": {
                    "url": "https://example.test/private?q=secret",
                    "status": 429,
                    "headers": {"Retry-After": "12"},
                }
            }),
            Some("loader-1"),
            Some("frame-1"),
            OffsetDateTime::UNIX_EPOCH,
        )
        .expect("main-document response");

        assert_eq!(observation.status, 429);
        assert_eq!(observation.origin, "https://example.test");
        assert_eq!(observation.retry_after, Some(Duration::from_secs(12)));
        assert!(!observation.is_redirect);
    }

    #[test]
    fn marks_redirect_only_when_status_and_location_agree() {
        let redirect = json!({
            "type": "Document",
            "frameId": "frame-1",
            "loaderId": "loader-1",
            "response": {
                "url": "https://example.test/start",
                "status": 302,
                "headers": {"location": "https://example.test/final"},
            }
        });
        assert!(
            navigation_response_observation(
                &redirect,
                Some("loader-1"),
                Some("frame-1"),
                OffsetDateTime::UNIX_EPOCH,
            )
            .unwrap()
            .is_redirect
        );

        let mut terminal = redirect;
        terminal["response"]["headers"] = json!({});
        assert!(
            !navigation_response_observation(
                &terminal,
                Some("loader-1"),
                Some("frame-1"),
                OffsetDateTime::UNIX_EPOCH,
            )
            .unwrap()
            .is_redirect
        );
    }

    #[test]
    fn parses_http_date_retry_after_and_ignores_other_loaders() {
        let now = OffsetDateTime::parse("Sun, 06 Nov 1994 08:49:30 GMT", &Rfc2822).unwrap();
        let params = json!({
            "type": "Document",
            "frameId": "frame-1",
            "loaderId": "loader-1",
            "response": {
                "url": "https://example.test/",
                "status": 429,
                "headers": {"retry-after": "Sun, 06 Nov 1994 08:49:37 GMT"},
            }
        });

        let observation =
            navigation_response_observation(&params, Some("loader-1"), Some("frame-1"), now)
                .expect("main-document response");
        assert_eq!(observation.retry_after, Some(Duration::from_secs(7)));
        assert!(navigation_response_observation(
            &params,
            Some("different-loader"),
            Some("frame-1"),
            now,
        )
        .is_none());
    }

    #[test]
    fn ignores_subresource_response_events() {
        assert!(navigation_response_observation(
            &json!({
                "type": "Image",
                "frameId": "frame-1",
                "loaderId": "loader-1",
                "response": {
                    "url": "https://example.test/tracker.png",
                    "status": 429,
                    "headers": {"Retry-After": "120"},
                }
            }),
            Some("loader-1"),
            Some("frame-1"),
            OffsetDateTime::UNIX_EPOCH,
        )
        .is_none());
    }
}
