//! Conservative CAPTCHA / bot-challenge detection for semantic browser snapshots.
//!
//! The detector reports a bounded classification from known challenge URLs or
//! corroborating visible semantic labels. It does not copy page text into the
//! report, act on a challenge, or treat a lone stock phrase as proof.
use std::collections::HashSet;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use time::{format_description::well_known::Rfc2822, OffsetDateTime};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BrowserChallengeStatus {
    Detected,
    NotDetected,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BrowserChallengeKind {
    AntiBotChallenge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BrowserChallengeSource {
    Url,
    Semantic,
}

impl BrowserChallengeSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Url => "url",
            Self::Semantic => "semantic",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BrowserChallengeConfidence {
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BrowserChallengeLabel<'a> {
    role: &'a str,
    name: &'a str,
}

impl<'a> BrowserChallengeLabel<'a> {
    pub(crate) fn new(role: &'a str, name: &'a str) -> Self {
        Self { role, name }
    }
}

/// Bounded public classification attached to `semantic_v2` snapshots.
///
/// Optional fields remain present as `null` so callers can handle one stable
/// object shape. `origin` is `null` for opaque URLs such as `about:blank`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct BrowserChallengeReport {
    pub(crate) status: BrowserChallengeStatus,
    pub(crate) kind: Option<BrowserChallengeKind>,
    pub(crate) origin: Option<String>,
    pub(crate) source: Option<BrowserChallengeSource>,
    pub(crate) confidence: Option<BrowserChallengeConfidence>,
}

impl BrowserChallengeReport {
    fn detected(
        origin: Option<String>,
        source: BrowserChallengeSource,
        confidence: BrowserChallengeConfidence,
    ) -> Self {
        Self {
            status: BrowserChallengeStatus::Detected,
            kind: Some(BrowserChallengeKind::AntiBotChallenge),
            origin,
            source: Some(source),
            confidence: Some(confidence),
        }
    }

    fn absent(origin: Option<String>, observation_complete: bool) -> Self {
        Self {
            status: if observation_complete {
                BrowserChallengeStatus::NotDetected
            } else {
                BrowserChallengeStatus::Unknown
            },
            kind: None,
            origin,
            source: None,
            confidence: None,
        }
    }

    pub(crate) fn containment_target(&self) -> Option<(&str, BrowserChallengeSource)> {
        (self.status == BrowserChallengeStatus::Detected).then(|| {
            (
                self.origin.as_deref().unwrap_or_default(),
                self.source
                    .expect("a detected challenge always has a bounded source"),
            )
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ChallengeCopyKind {
    RobotPrompt,
    HumanVerification,
    SecurityCheck,
    BrowserCheck,
}

pub(crate) fn browser_challenge_report<'a>(
    url: &str,
    labels: impl IntoIterator<Item = BrowserChallengeLabel<'a>>,
    observation_complete: bool,
) -> BrowserChallengeReport {
    let origin = nonopaque_browser_origin(url);
    if url_indicates_challenge(url) {
        return BrowserChallengeReport::detected(
            origin,
            BrowserChallengeSource::Url,
            BrowserChallengeConfidence::High,
        );
    }
    if visible_labels_indicate_challenge(labels) {
        return BrowserChallengeReport::detected(
            origin,
            BrowserChallengeSource::Semantic,
            BrowserChallengeConfidence::Medium,
        );
    }
    BrowserChallengeReport::absent(origin, observation_complete)
}

fn url_indicates_challenge(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    let path = parsed.path().to_ascii_lowercase();

    if host == "challenges.cloudflare.com" || path.starts_with("/cdn-cgi/challenge-platform/") {
        return true;
    }
    if (host == "www.google.com" || host == "www.recaptcha.net")
        && is_recaptcha_challenge_path(&path)
    {
        return true;
    }
    if host == "www.google.com" && path.starts_with("/sorry/") {
        return true;
    }
    if (host == "hcaptcha.com" || host.ends_with(".hcaptcha.com"))
        && (path.starts_with("/captcha/")
            || path.starts_with("/checksiteconfig")
            || path.starts_with("/getcaptcha/"))
    {
        return true;
    }
    (host == "funcaptcha.com"
        || host.ends_with(".funcaptcha.com")
        || host == "arkoselabs.com"
        || host.ends_with(".arkoselabs.com"))
        && path.starts_with("/fc/")
}

fn is_recaptcha_challenge_path(path: &str) -> bool {
    matches!(
        path.trim_end_matches('/'),
        "/recaptcha/api/fallback"
            | "/recaptcha/api2/anchor"
            | "/recaptcha/api2/bframe"
            | "/recaptcha/enterprise/anchor"
            | "/recaptcha/enterprise/bframe"
    )
}

fn visible_labels_indicate_challenge<'a>(
    labels: impl IntoIterator<Item = BrowserChallengeLabel<'a>>,
) -> bool {
    let mut copy_kinds = HashSet::new();
    let mut matching_labels = 0_usize;
    let mut seen_labels = HashSet::new();

    for label in labels {
        let role = normalize_text(label.role);
        let name = normalize_text(label.name);
        if name.is_empty() {
            continue;
        }

        let mut label_kinds = HashSet::new();
        for (kind, phrases) in [
            (
                ChallengeCopyKind::RobotPrompt,
                &["i'm not a robot", "i’m not a robot"][..],
            ),
            (
                ChallengeCopyKind::HumanVerification,
                &[
                    "verify you are human",
                    "verify that you are human",
                    "prove you are human",
                ][..],
            ),
            (
                ChallengeCopyKind::SecurityCheck,
                &[
                    "complete the security check",
                    "complete this security check",
                ][..],
            ),
            (
                ChallengeCopyKind::BrowserCheck,
                &[
                    "checking your browser",
                    "checking if the site connection is secure",
                    "review the security of your connection",
                ][..],
            ),
        ] {
            if phrases.iter().any(|phrase| name.contains(phrase)) {
                label_kinds.insert(kind);
            }
        }
        if role == "checkbox"
            && (label_kinds.contains(&ChallengeCopyKind::RobotPrompt)
                || label_kinds.contains(&ChallengeCopyKind::HumanVerification))
        {
            return true;
        }
        if !label_kinds.is_empty() && seen_labels.insert(name) {
            matching_labels += 1;
            copy_kinds.extend(label_kinds);
        }
    }

    // One quoted stock phrase is common in articles, messages, and control
    // values. Require distinct labels that corroborate distinct copy classes.
    copy_kinds.len() >= 2 && matching_labels >= 2
}

fn nonopaque_browser_origin(url: &str) -> Option<String> {
    let origin = browser_origin(url);
    (!origin.is_empty()).then_some(origin)
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
    use serde_json::{json, to_value};

    use super::*;

    fn value(url: &str, labels: &[&str], complete: bool) -> serde_json::Value {
        to_value(browser_challenge_report(
            url,
            labels
                .iter()
                .map(|label| BrowserChallengeLabel::new("statictext", label)),
            complete,
        ))
        .unwrap()
    }

    #[test]
    fn detects_cloudflare_challenge_infrastructure_without_guessing_a_provider() {
        let value = value(
            "https://example.test/cdn-cgi/challenge-platform/h/b/orchestrate/managed/v1",
            &[],
            true,
        );

        assert_eq!(
            value,
            json!({
                "status": "detected",
                "kind": "anti_bot_challenge",
                "origin": "https://example.test",
                "source": "url",
                "confidence": "high",
            })
        );
    }

    #[test]
    fn detects_known_recaptcha_hcaptcha_and_arkose_challenge_paths() {
        for url in [
            "https://www.google.com/recaptcha/api2/anchor?k=site-key",
            "https://www.recaptcha.net/recaptcha/enterprise/bframe?k=site-key",
            "https://www.google.com/recaptcha/api/fallback?k=site-key",
            "https://newassets.hcaptcha.com/captcha/v1/example/static/hcaptcha.html",
            "https://client-api.arkoselabs.com/fc/gc/",
        ] {
            let report = browser_challenge_report(url, std::iter::empty(), true);
            assert_eq!(report.status, BrowserChallengeStatus::Detected, "{url}");
            assert_eq!(report.source, Some(BrowserChallengeSource::Url), "{url}");
        }
    }

    #[test]
    fn recaptcha_product_and_administration_paths_are_not_challenge_pages() {
        for url in [
            "https://www.google.com/recaptcha/admin",
            "https://www.google.com/recaptcha/api.js",
            "https://www.google.com/recaptcha/about/",
            "https://www.recaptcha.net/recaptcha/docs/",
            "https://www.google.com/recaptcha/api2/reload?k=site-key",
        ] {
            let report = browser_challenge_report(url, std::iter::empty(), true);
            assert_eq!(report.status, BrowserChallengeStatus::NotDetected, "{url}");
        }
    }

    #[test]
    fn detects_correlated_visible_challenge_copy() {
        let value = value(
            "https://example.test/login",
            &[
                "Complete the security check",
                "Please verify you are human before continuing.",
            ],
            true,
        );

        assert_eq!(value["status"], "detected");
        assert_eq!(value["kind"], "anti_bot_challenge");
        assert_eq!(value["origin"], "https://example.test");
        assert_eq!(value["source"], "semantic");
        assert_eq!(value["confidence"], "medium");
    }

    #[test]
    fn detects_a_challenge_phrase_bound_to_an_interactive_control() {
        let value = to_value(browser_challenge_report(
            "https://example.test/login",
            [
                BrowserChallengeLabel::new("statictext", "I'm not a robot"),
                BrowserChallengeLabel::new("checkbox", "I'm not a robot"),
            ],
            true,
        ))
        .unwrap();

        assert_eq!(value["status"], "detected");
        assert_eq!(value["source"], "semantic");
        assert_eq!(value["confidence"], "medium");
    }

    #[test]
    fn ordinary_button_or_dialog_copy_does_not_classify_as_a_challenge() {
        for role in ["button", "dialog", "alert", "alertdialog"] {
            let value = to_value(browser_challenge_report(
                "https://example.test/account",
                [BrowserChallengeLabel::new(role, "Verify you are human")],
                true,
            ))
            .unwrap();

            assert_eq!(value["status"], "not_detected", "{role}");
        }
    }

    #[test]
    fn detects_google_unusual_traffic_interstitial_without_page_copy() {
        let value = value(
            "https://www.google.com/sorry/index?continue=https%3A%2F%2Fwww.google.com%2Fsearch",
            &[],
            true,
        );

        assert_eq!(value["status"], "detected");
        assert_eq!(value["origin"], "https://www.google.com");
        assert_eq!(value["source"], "url");
        assert_eq!(value["confidence"], "high");
    }

    #[test]
    fn one_stock_phrase_is_not_a_challenge() {
        for label in [
            "An article quotes the phrase: I'm not a robot.",
            "A chat message says: please verify you are human.",
        ] {
            let value = value("https://example.test/ordinary", &[label], true);
            assert_eq!(value["status"], "not_detected", "{label}");
        }
    }

    #[test]
    fn challenge_copy_must_span_distinct_semantic_labels() {
        let value = value(
            "https://example.test/ordinary",
            &["This article quotes 'verify you are human' and 'I'm not a robot'."],
            true,
        );

        assert_eq!(value["status"], "not_detected");
    }

    #[test]
    fn arbitrary_challenge_query_names_do_not_classify_a_page() {
        for url in [
            "https://example.test/?cf_chl_token=documentation",
            "https://example.test/?cf-chl-example=true",
        ] {
            let value = value(url, &["Documentation"], true);
            assert_eq!(value["status"], "not_detected", "{url}");
        }
    }

    #[test]
    fn challenge_markers_in_query_values_do_not_classify_a_page() {
        let value = value(
            "https://docs.example/search?q=%2Fcdn-cgi%2Fchallenge-platform%2F+cf_chl_token",
            &["Search results", "Documentation search results"],
            true,
        );

        assert_eq!(value["status"], "not_detected");
    }

    #[test]
    fn challenge_path_fragments_outside_the_reserved_prefix_are_not_classified() {
        let value = value(
            "https://docs.example/reference/cdn-cgi/challenge-platform/",
            &["Cloud service documentation"],
            true,
        );

        assert_eq!(value["status"], "not_detected");
    }

    #[test]
    fn provider_homepages_and_docs_are_not_challenge_infrastructure() {
        for url in [
            "https://www.hcaptcha.com/",
            "https://docs.hcaptcha.com/configuration/",
            "https://www.arkoselabs.com/resources/",
            "https://developer.funcaptcha.com/docs/",
        ] {
            let value = value(url, &["Documentation", "Product documentation"], true);
            assert_eq!(value["status"], "not_detected", "{url}: {value}");
        }
    }

    #[test]
    fn report_has_a_stable_closed_shape() {
        let present = value(
            "https://example.test/login",
            &["Complete the security check", "Verify you are human"],
            true,
        );
        let absent = value("https://example.test/", &["Example Domain"], true);
        let unknown = value("https://example.test/", &["Example Domain"], false);

        let mut present_keys = present.as_object().unwrap().keys().collect::<Vec<_>>();
        let mut absent_keys = absent.as_object().unwrap().keys().collect::<Vec<_>>();
        let mut unknown_keys = unknown.as_object().unwrap().keys().collect::<Vec<_>>();
        present_keys.sort();
        absent_keys.sort();
        unknown_keys.sort();
        assert_eq!(present_keys, absent_keys);
        assert_eq!(present_keys, unknown_keys);
        assert_eq!(
            present_keys,
            vec!["confidence", "kind", "origin", "source", "status"]
        );
        assert_eq!(absent["status"], "not_detected");
        assert_eq!(unknown["status"], "unknown");
        assert!(unknown["kind"].is_null());
        assert!(unknown["source"].is_null());
    }

    #[test]
    fn opaque_urls_report_an_unknown_origin() {
        let report = browser_challenge_report(
            "about:blank",
            [
                BrowserChallengeLabel::new("statictext", "Complete the security check"),
                BrowserChallengeLabel::new("statictext", "Verify you are human"),
            ],
            true,
        );
        assert_eq!(
            report.containment_target(),
            Some(("", BrowserChallengeSource::Semantic))
        );
        let value = to_value(report).unwrap();

        assert_eq!(value["status"], "detected");
        assert!(value["origin"].is_null());
    }

    #[test]
    fn report_does_not_echo_page_content_or_url_details() {
        let value = value(
            "https://example.test/login?secret=do-not-copy",
            &[
                "Private account name: Alice. Complete the security check.",
                "Verify you are human.",
            ],
            true,
        );
        let serialized = serde_json::to_string(&value).unwrap();

        assert!(!serialized.contains("Alice"));
        assert!(!serialized.contains("secret"));
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
