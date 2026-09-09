//! Conservative CAPTCHA / bot-challenge detection for semantic browser snapshots.
//!
//! The detector reports a bounded classification from known challenge URLs or
//! a visible human-verification label bound directly to a checkbox. It does not
//! copy page text into the report, act on a challenge, or treat page copy as
//! proof.

use serde::Serialize;

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
}

pub(crate) fn browser_challenge_report<'a>(
    url: &str,
    labels: impl IntoIterator<Item = BrowserChallengeLabel<'a>>,
    observation_complete: bool,
) -> BrowserChallengeReport {
    let origin = browser_origin(url);
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
    for label in labels {
        let role = normalize_text(label.role);
        let name = normalize_text(label.name);
        if name.is_empty() {
            continue;
        }

        let names_human_check = [
            "i'm not a robot",
            "i’m not a robot",
            "verify you are human",
            "verify that you are human",
            "prove you are human",
        ]
        .iter()
        .any(|phrase| name.contains(phrase));
        if role == "checkbox" && names_human_check {
            return true;
        }
    }

    // Challenge phrases also occur in documentation, support copy, and
    // articles. Without a challenge-specific control relationship, combining
    // page-wide labels would turn unrelated prose into a positive report.
    false
}

fn browser_origin(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    match parsed.origin() {
        url::Origin::Tuple(_, _, _) => Some(parsed.origin().ascii_serialization()),
        url::Origin::Opaque(_) => None,
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
    fn unrelated_visible_challenge_copy_does_not_classify_the_page() {
        let value = value(
            "https://example.test/login",
            &[
                "Complete the security check",
                "Please verify you are human before continuing.",
            ],
            true,
        );

        assert_eq!(value["status"], "not_detected");
        assert!(value["kind"].is_null());
        assert!(value["source"].is_null());
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
    fn challenge_phrases_in_static_copy_do_not_classify_the_page() {
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
        let present = to_value(browser_challenge_report(
            "https://example.test/login",
            [BrowserChallengeLabel::new(
                "checkbox",
                "Verify you are human",
            )],
            true,
        ))
        .unwrap();
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
        let value = to_value(browser_challenge_report(
            "about:blank",
            [BrowserChallengeLabel::new(
                "checkbox",
                "Verify you are human",
            )],
            true,
        ))
        .unwrap();

        assert_eq!(value["status"], "detected");
        assert!(value["origin"].is_null());
    }

    #[test]
    fn report_does_not_echo_page_content_or_url_details() {
        let value = to_value(browser_challenge_report(
            "https://example.test/login?secret=do-not-copy",
            [BrowserChallengeLabel::new(
                "checkbox",
                "Private account name: Alice. Verify you are human.",
            )],
            true,
        ))
        .unwrap();
        let serialized = serde_json::to_string(&value).unwrap();

        assert!(!serialized.contains("Alice"));
        assert!(!serialized.contains("secret"));
        assert_eq!(value["origin"], "https://example.test");
    }
}
