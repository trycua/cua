//! Resolves "what is the latest Cua Driver release?" without depending on the
//! rate-limited GitHub REST API.
//!
//! The installer (`https://cua.ai/driver/install.sh`, which delegates to
//! `_install-rust.sh`) resolves the stable version from a constant it carries,
//! `CUA_DRIVER_RS_BAKED_VERSION`. The release workflow advances that constant
//! only after every release asset is public, so it names the newest release a
//! user can actually install. Update discovery reads the same constant, which
//! keeps `check-update` and the installer in agreement by construction.
//!
//! Source order:
//!
//! | channel | order |
//! |---------|-------|
//! | stable  | installer's baked version, then the releases API, then the releases feed |
//! | nightly | releases API, then the releases feed |
//!
//! The releases API call carries `GH_TOKEN` / `GITHUB_TOKEN` when one is set
//! (same precedence as the installer) and only ever to `api.github.com`.
//! The `releases.atom` feed lives on github.com and is not subject to the REST
//! API quota, but it only lists the newest ten releases across every product in
//! the monorepo, so it is a last resort rather than a primary source.
//!
//! Failures are classified so callers can tell the user the truth: a REST rate
//! limit ([`FetchError::RateLimited`], with the reset time) is not a network
//! failure ([`FetchError::Network`]).

use std::fmt;
use std::time::Duration;

use crate::release_channel::ReleaseChannel;

/// Installer script that carries the baked stable version.
pub(crate) const INSTALLER_SCRIPT_URL: &str = "https://cua.ai/driver/_install-rust.sh";
/// REST endpoint listing releases, newest first.
pub(crate) const RELEASES_API_URL: &str = "https://api.github.com/repos/trycua/cua/releases";
/// Atom feed of the newest releases. Served from github.com, not the REST API.
pub(crate) const RELEASES_FEED_URL: &str = "https://github.com/trycua/cua/releases.atom";

/// Pages of 100 releases to walk before giving up. The monorepo interleaves
/// several products, so the newest `cua-driver-rs-v*` tag is not always on the
/// first page. The installer uses the same bound.
const MAX_API_PAGES: usize = 10;
const API_PAGE_SIZE: usize = 100;

const HTTP_TIMEOUT_SECONDS: u64 = 4;

const BAKED_VERSION_KEY: &str = "CUA_DRIVER_RS_BAKED_VERSION=";
const WITHDRAWN_VERSIONS_KEY: &str = "CUA_DRIVER_RS_WITHDRAWN_VERSIONS=";

// ── Errors ───────────────────────────────────────────────────────────────

/// Why a release lookup failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// The GitHub REST API refused the request because its quota is spent.
    RateLimited {
        /// Quota per hour (`x-ratelimit-limit`), when reported.
        limit: Option<u64>,
        /// Unix time at which the quota resets (`x-ratelimit-reset`, or
        /// `retry-after` converted to an absolute time).
        reset_unix: Option<u64>,
        /// Whether the rejected request carried a token.
        authenticated: bool,
    },
    /// A transport failure: DNS, connect, TLS or timeout.
    Network { host: String, detail: String },
    /// The server answered but the answer was unusable.
    Other(String),
}

impl FetchError {
    /// Human-readable description relative to `now` (unix seconds).
    pub fn describe(&self, now: u64) -> String {
        match self {
            FetchError::RateLimited {
                limit,
                reset_unix,
                authenticated,
            } => {
                let who = if *authenticated {
                    "authenticated"
                } else {
                    "unauthenticated"
                };
                let quota = match limit {
                    Some(limit) => format!("{limit} requests/hour, {who}"),
                    None => who.to_owned(),
                };
                let reset = match reset_unix {
                    Some(reset) => format!(
                        "Resets at {} ({}).",
                        crate::version_check::iso8601(*reset),
                        relative_time(*reset, now)
                    ),
                    None => "The reset time was not reported.".to_owned(),
                };
                let advice = if *authenticated {
                    "Wait for the reset, or run the installer directly: \
                     curl -fsSL https://cua.ai/driver/install.sh | bash"
                } else {
                    "Set GITHUB_TOKEN or GH_TOKEN to use your own quota."
                };
                format!("GitHub API rate limit exceeded ({quota}). {reset} {advice}")
            }
            FetchError::Network { host, detail } => format!(
                "Could not reach {host}: {detail}. Check your network, DNS, proxy or TLS \
                 settings and try again."
            ),
            FetchError::Other(message) => message.clone(),
        }
    }
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.describe(crate::version_check::unix_now()))
    }
}

impl std::error::Error for FetchError {}

fn relative_time(target: u64, now: u64) -> String {
    if target <= now {
        return "now".to_owned();
    }
    let secs = target - now;
    if secs < 90 {
        format!("in {secs} s")
    } else if secs < 90 * 60 {
        format!("in {} min", secs.div_ceil(60))
    } else {
        let minutes = secs.div_ceil(60);
        format!("in {} h {} min", minutes / 60, minutes % 60)
    }
}

// ── HTTP seam ────────────────────────────────────────────────────────────

/// A completed HTTP exchange. Any status code is a response, not an error;
/// only transport failures are `Err`.
#[derive(Debug, Clone)]
pub(crate) struct HttpResponse {
    pub status: u16,
    /// Header names are lowercased.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl HttpResponse {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// Transport failure detail, host-tagged by the caller.
pub(crate) type TransportError = String;

pub(crate) trait HttpClient {
    /// GET `url`. `bearer` is sent as `Authorization: Bearer ...` when set.
    fn get(
        &self,
        url: &str,
        accept: &str,
        bearer: Option<&str>,
    ) -> Result<HttpResponse, TransportError>;
}

/// Production client backed by `ureq`.
pub(crate) struct UreqClient {
    agent: ureq::Agent,
}

impl UreqClient {
    pub(crate) fn new() -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(HTTP_TIMEOUT_SECONDS)))
            // 4xx/5xx are responses we classify ourselves (rate limits above all).
            .http_status_as_error(false)
            .build()
            .new_agent();
        Self { agent }
    }
}

impl HttpClient for UreqClient {
    fn get(
        &self,
        url: &str,
        accept: &str,
        bearer: Option<&str>,
    ) -> Result<HttpResponse, TransportError> {
        let mut request = self.agent.get(url).header("Accept", accept).header(
            "User-Agent",
            concat!("cua-driver-rs/", env!("CARGO_PKG_VERSION")),
        );
        if let Some(token) = bearer {
            request = request.header("Authorization", &format!("Bearer {token}"));
        }
        let response = request.call().map_err(|error| error.to_string())?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                Some((
                    name.as_str().to_ascii_lowercase(),
                    value.to_str().ok()?.to_owned(),
                ))
            })
            .collect();
        let body = response
            .into_body()
            .read_to_string()
            .map_err(|error| error.to_string())?;
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

/// Endpoints, overridable so tests can point at a local server.
#[derive(Debug, Clone)]
pub(crate) struct Endpoints {
    pub installer_script: String,
    pub releases_api: String,
    pub releases_feed: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            installer_script: INSTALLER_SCRIPT_URL.to_owned(),
            releases_api: RELEASES_API_URL.to_owned(),
            releases_feed: RELEASES_FEED_URL.to_owned(),
        }
    }
}

/// `GH_TOKEN` wins over `GITHUB_TOKEN`, matching the GitHub CLI and the
/// installer. Empty values count as unset.
pub(crate) fn token_from_env() -> Option<String> {
    ["GH_TOKEN", "GITHUB_TOKEN"].iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    })
}

// ── Resolution ───────────────────────────────────────────────────────────

/// Resolve the latest release version for `channel`, bare (no tag prefix).
pub(crate) fn resolve_latest(
    http: &dyn HttpClient,
    endpoints: &Endpoints,
    channel: ReleaseChannel,
    token: Option<&str>,
) -> Result<String, FetchError> {
    let mut errors: Vec<FetchError> = Vec::new();
    let mut withdrawn: Vec<String> = Vec::new();

    if channel == ReleaseChannel::Stable {
        match fetch_installer_pin(http, endpoints) {
            Ok(pin) => {
                withdrawn = pin.withdrawn;
                // Same rule as the installer: a baked version that is not
                // withdrawn is the latest stable release.
                if let Some(baked) = pin.baked.filter(|baked| !withdrawn.contains(baked)) {
                    return Ok(baked);
                }
            }
            Err(error) => {
                tracing::debug!(target: "cua_driver::release_source",
                                "installer lookup failed: {error:?}");
                errors.push(error);
            }
        }
    }

    match fetch_via_api(http, endpoints, channel, token, &withdrawn) {
        Ok(Some(version)) => return Ok(version),
        Ok(None) => errors.push(no_match_error(channel)),
        Err(error) => {
            tracing::debug!(target: "cua_driver::release_source",
                            "releases API lookup failed: {error:?}");
            errors.push(error);
        }
    }

    match fetch_via_feed(http, endpoints, channel, &withdrawn) {
        Ok(Some(version)) => return Ok(version),
        Ok(None) => errors.push(no_match_error(channel)),
        Err(error) => {
            tracing::debug!(target: "cua_driver::release_source",
                            "releases feed lookup failed: {error:?}");
            errors.push(error);
        }
    }

    Err(most_informative(errors, channel))
}

fn no_match_error(channel: ReleaseChannel) -> FetchError {
    FetchError::Other(format!(
        "no matching {}* release found",
        crate::version_check::tag_prefix(channel)
    ))
}

/// Pick the error that explains the failure best: a rate limit beats
/// everything, a real answer ("no such release") beats a transport failure
/// elsewhere, and a network error is reported only when nothing was reachable.
fn most_informative(errors: Vec<FetchError>, channel: ReleaseChannel) -> FetchError {
    if let Some(limited) = errors
        .iter()
        .find(|error| matches!(error, FetchError::RateLimited { .. }))
    {
        return limited.clone();
    }
    if let Some(other) = errors
        .iter()
        .find(|error| matches!(error, FetchError::Other(_)))
    {
        return other.clone();
    }
    errors
        .into_iter()
        .next()
        .unwrap_or_else(|| no_match_error(channel))
}

fn host_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split(['/', '?'])
        .next()
        .unwrap_or(url)
        .to_owned()
}

fn transport(url: &str, detail: TransportError) -> FetchError {
    FetchError::Network {
        host: host_of(url),
        detail,
    }
}

// ── Installer pin ────────────────────────────────────────────────────────

#[derive(Debug, Default, PartialEq, Eq)]
struct InstallerPin {
    baked: Option<String>,
    withdrawn: Vec<String>,
}

fn fetch_installer_pin(
    http: &dyn HttpClient,
    endpoints: &Endpoints,
) -> Result<InstallerPin, FetchError> {
    let url = &endpoints.installer_script;
    let response = http
        .get(url, "text/plain", None)
        .map_err(|detail| transport(url, detail))?;
    if !(200..300).contains(&response.status) {
        return Err(FetchError::Other(format!(
            "{} returned HTTP {}",
            host_of(url),
            response.status
        )));
    }
    Ok(parse_installer_pin(&response.body))
}

/// Read the two sentinel assignments out of `_install-rust.sh`:
///
/// ```text
/// CUA_DRIVER_RS_BAKED_VERSION="0.34.0" # published-installer-version
/// CUA_DRIVER_RS_WITHDRAWN_VERSIONS="0.28.3" # withdrawn-installer-versions
/// ```
fn parse_installer_pin(script: &str) -> InstallerPin {
    let mut pin = InstallerPin::default();
    for line in script.lines() {
        // Only a bare assignment at the start of a line: uses such as
        // `"${CUA_DRIVER_RS_BAKED_VERSION#v}"` and comments never match.
        if let Some(rest) = line.strip_prefix(BAKED_VERSION_KEY) {
            pin.baked = quoted_value(rest)
                .map(|value| value.trim_start_matches('v').to_owned())
                .filter(|value| is_plain_release(value));
        } else if let Some(rest) = line.strip_prefix(WITHDRAWN_VERSIONS_KEY) {
            pin.withdrawn = quoted_value(rest)
                .map(|value| value.split_whitespace().map(str::to_owned).collect())
                .unwrap_or_default();
        }
    }
    pin
}

fn quoted_value(rest: &str) -> Option<&str> {
    let rest = rest.strip_prefix('"')?;
    Some(&rest[..rest.find('"')?])
}

/// Exact `x.y.z` with no pre-release or build suffix.
fn is_plain_release(value: &str) -> bool {
    semver::Version::parse(value)
        .map(|version| version.pre.is_empty() && version.build.is_empty())
        .unwrap_or(false)
        && value.split('.').count() == 3
}

// ── Releases API ─────────────────────────────────────────────────────────

fn fetch_via_api(
    http: &dyn HttpClient,
    endpoints: &Endpoints,
    channel: ReleaseChannel,
    token: Option<&str>,
    withdrawn: &[String],
) -> Result<Option<String>, FetchError> {
    let mut token = token;
    for page in 1..=MAX_API_PAGES {
        let url = format!(
            "{}?per_page={API_PAGE_SIZE}&page={page}",
            endpoints.releases_api
        );
        let response = api_get(http, &url, &mut token)?;
        let body: serde_json::Value = serde_json::from_str(&response.body)
            .map_err(|error| FetchError::Other(format!("JSON parse error: {error}")))?;
        let entries = body.as_array().map(Vec::len).unwrap_or(0);
        if let Some(version) =
            crate::version_check::pick_latest_release_excluding(&body, channel, withdrawn)
        {
            return Ok(Some(version));
        }
        if entries < API_PAGE_SIZE {
            break;
        }
    }
    Ok(None)
}

/// GET against the REST API, classifying rate limits. A rejected token (401)
/// is retried once without it: a stale `GITHUB_TOKEN` in the environment must
/// not make discovery worse than having none.
fn api_get(
    http: &dyn HttpClient,
    url: &str,
    token: &mut Option<&str>,
) -> Result<HttpResponse, FetchError> {
    const ACCEPT: &str = "application/vnd.github+json";
    let mut response = http
        .get(url, ACCEPT, *token)
        .map_err(|detail| transport(url, detail))?;
    if response.status == 401 && token.is_some() {
        tracing::debug!(target: "cua_driver::release_source",
                        "GitHub rejected the token; retrying unauthenticated");
        *token = None;
        response = http
            .get(url, ACCEPT, None)
            .map_err(|detail| transport(url, detail))?;
    }
    if (200..300).contains(&response.status) {
        return Ok(response);
    }
    if let Some(limited) = classify_rate_limit(&response, token.is_some()) {
        return Err(limited);
    }
    Err(FetchError::Other(format!(
        "GitHub API returned HTTP {}",
        response.status
    )))
}

/// A 429, or a 403 that says the quota is spent (`x-ratelimit-remaining: 0`,
/// a `retry-after` for secondary limits, or a rate-limit message body).
fn classify_rate_limit(response: &HttpResponse, authenticated: bool) -> Option<FetchError> {
    let remaining_zero = response
        .header("x-ratelimit-remaining")
        .map(|value| value.trim() == "0")
        .unwrap_or(false);
    let retry_after = response
        .header("retry-after")
        .and_then(|value| value.trim().parse::<u64>().ok());
    let limited = match response.status {
        429 => true,
        403 => {
            remaining_zero
                || retry_after.is_some()
                || response.body.to_ascii_lowercase().contains("rate limit")
        }
        _ => false,
    };
    if !limited {
        return None;
    }
    let reset_unix = response
        .header("x-ratelimit-reset")
        .and_then(|value| value.trim().parse::<u64>().ok())
        .or_else(|| retry_after.map(|secs| crate::version_check::unix_now() + secs));
    Some(FetchError::RateLimited {
        limit: response
            .header("x-ratelimit-limit")
            .and_then(|value| value.trim().parse().ok()),
        reset_unix,
        authenticated,
    })
}

// ── Releases feed ────────────────────────────────────────────────────────

fn fetch_via_feed(
    http: &dyn HttpClient,
    endpoints: &Endpoints,
    channel: ReleaseChannel,
    withdrawn: &[String],
) -> Result<Option<String>, FetchError> {
    let url = &endpoints.releases_feed;
    let response = http
        .get(url, "application/atom+xml", None)
        .map_err(|detail| transport(url, detail))?;
    if !(200..300).contains(&response.status) {
        return Err(FetchError::Other(format!(
            "{} returned HTTP {}",
            host_of(url),
            response.status
        )));
    }
    let releases: Vec<serde_json::Value> = feed_tags(&response.body)
        .into_iter()
        .map(|tag| serde_json::json!({ "tag_name": tag, "draft": false }))
        .collect();
    Ok(crate::version_check::pick_latest_release_excluding(
        &serde_json::Value::Array(releases),
        channel,
        withdrawn,
    ))
}

/// Tag names from `<link ... href=".../releases/tag/<tag>"/>` entries.
fn feed_tags(feed: &str) -> Vec<String> {
    const MARKER: &str = "/releases/tag/";
    let mut tags = Vec::new();
    let mut rest = feed;
    while let Some(at) = rest.find(MARKER) {
        let after = &rest[at + MARKER.len()..];
        let end = after.find(['"', '\'', '<', '&']).unwrap_or(after.len());
        let tag = &after[..end];
        if !tag.is_empty() {
            tags.push(tag.to_owned());
        }
        rest = &after[end..];
    }
    tags
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    /// Scripted HTTP client. Responses are keyed by URL prefix; every call is
    /// recorded with the bearer token it carried.
    #[derive(Default)]
    struct MockHttp {
        routes: Vec<(String, Result<HttpResponse, TransportError>)>,
        calls: RefCell<Vec<(String, Option<String>)>>,
    }

    impl MockHttp {
        fn route(mut self, prefix: &str, response: Result<HttpResponse, TransportError>) -> Self {
            self.routes.push((prefix.to_owned(), response));
            self
        }
        fn calls_to(&self, prefix: &str) -> Vec<Option<String>> {
            self.calls
                .borrow()
                .iter()
                .filter(|(url, _)| url.starts_with(prefix))
                .map(|(_, token)| token.clone())
                .collect()
        }
    }

    impl HttpClient for MockHttp {
        fn get(
            &self,
            url: &str,
            _accept: &str,
            bearer: Option<&str>,
        ) -> Result<HttpResponse, TransportError> {
            self.calls
                .borrow_mut()
                .push((url.to_owned(), bearer.map(str::to_owned)));
            self.routes
                .iter()
                .find(|(prefix, _)| url.starts_with(prefix))
                .map(|(_, response)| response.clone())
                .unwrap_or_else(|| Err(format!("no route for {url}")))
        }
    }

    const SCRIPT: &str = "https://scripts.test/_install-rust.sh";
    const API: &str = "https://api.test/repos/trycua/cua/releases";
    const FEED: &str = "https://feed.test/releases.atom";

    fn endpoints() -> Endpoints {
        Endpoints {
            installer_script: SCRIPT.to_owned(),
            releases_api: API.to_owned(),
            releases_feed: FEED.to_owned(),
        }
    }

    fn ok(body: &str) -> Result<HttpResponse, TransportError> {
        Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: body.to_owned(),
        })
    }

    fn status(
        code: u16,
        headers: &[(&str, &str)],
        body: &str,
    ) -> Result<HttpResponse, TransportError> {
        Ok(HttpResponse {
            status: code,
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body: body.to_owned(),
        })
    }

    fn rate_limited() -> Result<HttpResponse, TransportError> {
        status(
            403,
            &[
                ("x-ratelimit-limit", "60"),
                ("x-ratelimit-remaining", "0"),
                ("x-ratelimit-reset", "1791298445"),
            ],
            r#"{"message":"API rate limit exceeded for 1.2.3.4."}"#,
        )
    }

    fn installer_script(baked: &str, withdrawn: &str) -> String {
        format!(
            "#!/bin/bash\n# prose mentioning CUA_DRIVER_RS_BAKED_VERSION=\"9.9.9\"\n\
             # ~~~ BAKED_VERSION ~~~\n\
             CUA_DRIVER_RS_BAKED_VERSION=\"{baked}\" # published-installer-version\n\
             # ~~~ END ~~~\n\
             CUA_DRIVER_RS_WITHDRAWN_VERSIONS=\"{withdrawn}\" # withdrawn-installer-versions\n\
             TAG=\"${{TAG_PREFIX}}${{CUA_DRIVER_RS_BAKED_VERSION#v}}\"\n"
        )
    }

    fn releases_json(tags: &[&str]) -> String {
        serde_json::Value::Array(
            tags.iter()
                .map(|tag| serde_json::json!({"tag_name": tag, "draft": false, "prerelease": true}))
                .collect(),
        )
        .to_string()
    }

    fn feed_xml(tags: &[&str]) -> String {
        let entries: String = tags
            .iter()
            .map(|tag| {
                format!(
                    "<entry><link rel=\"alternate\" type=\"text/html\" \
                     href=\"https://github.com/trycua/cua/releases/tag/{tag}\"/>\
                     <title>{tag}</title></entry>"
                )
            })
            .collect();
        format!("<feed>{entries}</feed>")
    }

    #[test]
    fn stable_uses_the_installers_baked_version_without_touching_the_api() {
        let http = MockHttp::default()
            .route(SCRIPT, ok(&installer_script("0.34.0", "0.28.3")))
            .route(API, rate_limited());
        let latest = resolve_latest(&http, &endpoints(), ReleaseChannel::Stable, None).unwrap();
        assert_eq!(latest, "0.34.0");
        assert!(http.calls_to(API).is_empty(), "API must not be consulted");
    }

    #[test]
    fn baked_version_wins_over_a_newer_api_release() {
        // The installer would install 0.34.0; so must update discovery, even if
        // the API already lists a tag whose assets are not all public yet.
        let http = MockHttp::default()
            .route(SCRIPT, ok(&installer_script("0.34.0", "")))
            .route(API, ok(&releases_json(&["cua-driver-rs-v0.35.0"])));
        let latest = resolve_latest(&http, &endpoints(), ReleaseChannel::Stable, None).unwrap();
        assert_eq!(latest, "0.34.0");
    }

    #[test]
    fn withdrawn_baked_version_falls_back_to_the_api_and_skips_withdrawn_tags() {
        let http = MockHttp::default()
            .route(SCRIPT, ok(&installer_script("0.28.3", "0.28.3 0.30.0")))
            .route(
                API,
                ok(&releases_json(&[
                    "cua-driver-rs-v0.30.0",
                    "cua-driver-rs-v0.29.0",
                    "lume-v1.0.0",
                ])),
            );
        let latest = resolve_latest(&http, &endpoints(), ReleaseChannel::Stable, None).unwrap();
        assert_eq!(latest, "0.29.0");
    }

    #[test]
    fn rate_limited_api_is_reported_with_the_reset_time_not_as_a_network_error() {
        let http = MockHttp::default()
            .route(SCRIPT, Err("connection refused".into()))
            .route(API, rate_limited())
            .route(FEED, Err("connection refused".into()));
        let error = resolve_latest(&http, &endpoints(), ReleaseChannel::Stable, None).unwrap_err();
        assert_eq!(
            error,
            FetchError::RateLimited {
                limit: Some(60),
                reset_unix: Some(1_791_298_445),
                authenticated: false,
            }
        );
        let message = error.describe(1_791_298_445 - 600);
        assert!(message.contains("rate limit exceeded"), "{message}");
        assert!(
            message.contains("60 requests/hour, unauthenticated"),
            "{message}"
        );
        assert!(
            message.contains(&crate::version_check::iso8601(1_791_298_445)),
            "{message}"
        );
        assert!(message.contains("in 10 min"), "{message}");
        assert!(message.contains("GITHUB_TOKEN"), "{message}");
        assert!(!message.contains("Could not reach"), "{message}");
    }

    #[test]
    fn http_429_is_a_rate_limit_too() {
        let http =
            MockHttp::default().route(API, status(429, &[("retry-after", "30")], "slow down"));
        let error =
            fetch_via_api(&http, &endpoints(), ReleaseChannel::Nightly, None, &[]).unwrap_err();
        assert!(
            matches!(
                error,
                FetchError::RateLimited {
                    reset_unix: Some(_),
                    ..
                }
            ),
            "{error:?}"
        );
    }

    #[test]
    fn plain_403_is_not_misreported_as_a_rate_limit() {
        let http = MockHttp::default().route(
            API,
            status(403, &[("x-ratelimit-remaining", "42")], "forbidden"),
        );
        let error =
            fetch_via_api(&http, &endpoints(), ReleaseChannel::Nightly, None, &[]).unwrap_err();
        assert_eq!(
            error,
            FetchError::Other("GitHub API returned HTTP 403".into())
        );
    }

    #[test]
    fn real_network_failure_is_reported_as_could_not_reach() {
        let http = MockHttp::default()
            .route(SCRIPT, Err("dns error: no such host".into()))
            .route(API, Err("dns error: no such host".into()))
            .route(FEED, Err("tls handshake failed".into()));
        let error = resolve_latest(&http, &endpoints(), ReleaseChannel::Stable, None).unwrap_err();
        assert!(matches!(error, FetchError::Network { .. }), "{error:?}");
        let message = error.describe(0);
        assert!(message.starts_with("Could not reach "), "{message}");
        assert!(message.contains("DNS"), "{message}");
        assert!(!message.contains("rate limit"), "{message}");
    }

    #[test]
    fn token_is_sent_to_the_api_only_and_authenticated_limits_say_so() {
        let http = MockHttp::default()
            .route(SCRIPT, ok(&installer_script("0.28.3", "0.28.3")))
            .route(API, ok(&releases_json(&["cua-driver-rs-v0.34.0"])));
        let latest = resolve_latest(
            &http,
            &endpoints(),
            ReleaseChannel::Stable,
            Some("ghp_secret"),
        )
        .unwrap();
        assert_eq!(latest, "0.34.0");
        assert_eq!(http.calls_to(API), vec![Some("ghp_secret".to_owned())]);
        assert_eq!(
            http.calls_to(SCRIPT),
            vec![None],
            "token must never leave api host"
        );

        let limited = MockHttp::default().route(API, rate_limited());
        let error = fetch_via_api(
            &limited,
            &endpoints(),
            ReleaseChannel::Nightly,
            Some("ghp_secret"),
            &[],
        )
        .unwrap_err();
        assert!(
            matches!(
                error,
                FetchError::RateLimited {
                    authenticated: true,
                    ..
                }
            ),
            "{error:?}"
        );
        assert!(error.describe(0).contains("authenticated"));
    }

    #[test]
    fn rejected_token_is_retried_without_it() {
        let calls = RefCell::new(0);
        struct Flaky<'a>(&'a RefCell<i32>, String);
        impl HttpClient for Flaky<'_> {
            fn get(
                &self,
                _url: &str,
                _accept: &str,
                bearer: Option<&str>,
            ) -> Result<HttpResponse, TransportError> {
                *self.0.borrow_mut() += 1;
                if bearer.is_some() {
                    status(401, &[], "Bad credentials")
                } else {
                    ok(&self.1)
                }
            }
        }
        let http = Flaky(&calls, releases_json(&["cua-driver-rs-v0.34.0"]));
        let found = fetch_via_api(
            &http,
            &endpoints(),
            ReleaseChannel::Stable,
            Some("stale"),
            &[],
        )
        .unwrap();
        assert_eq!(found.as_deref(), Some("0.34.0"));
        assert_eq!(*calls.borrow(), 2);
    }

    #[test]
    fn feed_is_the_fallback_when_installer_and_api_are_unavailable() {
        let http = MockHttp::default()
            .route(SCRIPT, status(503, &[], ""))
            .route(API, rate_limited())
            .route(
                FEED,
                ok(&feed_xml(&[
                    "nightly-lume-v0.6.2-nightly.20261006.37415690828",
                    "cua-driver-rs-v0.34.0",
                    "cua-driver-rs-v0.33.4",
                ])),
            );
        let latest = resolve_latest(&http, &endpoints(), ReleaseChannel::Stable, None).unwrap();
        assert_eq!(latest, "0.34.0");
    }

    #[test]
    fn nightly_never_reads_the_installer_pin() {
        let http = MockHttp::default()
            .route(SCRIPT, ok(&installer_script("0.34.0", "")))
            .route(
                API,
                ok(&releases_json(&[
                    "nightly-cua-driver-rs-v0.34.1-nightly.20261006.7",
                    "cua-driver-rs-v0.34.0",
                ])),
            );
        let latest = resolve_latest(&http, &endpoints(), ReleaseChannel::Nightly, None).unwrap();
        assert_eq!(latest, "0.34.1-nightly.20261006.7");
        assert!(http.calls_to(SCRIPT).is_empty());
    }

    #[test]
    fn api_walks_further_pages_until_a_match_appears() {
        let first_page: Vec<String> = (0..API_PAGE_SIZE)
            .map(|i| format!("lume-v1.0.{i}"))
            .collect();
        let first_page: Vec<&str> = first_page.iter().map(String::as_str).collect();
        let http = MockHttp::default()
            .route(
                &format!("{API}?per_page=100&page=1"),
                ok(&releases_json(&first_page)),
            )
            .route(
                &format!("{API}?per_page=100&page=2"),
                ok(&releases_json(&["cua-driver-rs-v0.20.0"])),
            );
        let found = fetch_via_api(&http, &endpoints(), ReleaseChannel::Stable, None, &[]).unwrap();
        assert_eq!(found.as_deref(), Some("0.20.0"));
    }

    #[test]
    fn installer_pin_parser_ignores_comments_and_expansions() {
        let pin = parse_installer_pin(&installer_script("0.34.0", "0.28.3 0.30.1"));
        assert_eq!(pin.baked.as_deref(), Some("0.34.0"));
        assert_eq!(pin.withdrawn, vec!["0.28.3", "0.30.1"]);
        assert_eq!(
            parse_installer_pin(&installer_script("0.34.0-rc.1", "")).baked,
            None
        );
        assert_eq!(parse_installer_pin(&installer_script("", "")).baked, None);
        assert_eq!(
            parse_installer_pin("no sentinels here"),
            InstallerPin::default()
        );
    }

    /// Serve one canned HTTP response on localhost and return the base URL.
    fn serve_once(response: &'static str) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(response.as_bytes());
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn ureq_client_surfaces_a_403_with_its_rate_limit_headers() {
        let base = serve_once(
            "HTTP/1.1 403 Forbidden\r\ncontent-type: application/json\r\n\
             x-ratelimit-limit: 60\r\nx-ratelimit-remaining: 0\r\n\
             x-ratelimit-reset: 1791298445\r\ncontent-length: 44\r\n\r\n\
             {\"message\":\"API rate limit exceeded for ip\"}",
        );
        let endpoints = Endpoints {
            releases_api: format!("{base}/repos/trycua/cua/releases"),
            ..endpoints()
        };
        let error = fetch_via_api(
            &UreqClient::new(),
            &endpoints,
            ReleaseChannel::Nightly,
            None,
            &[],
        )
        .unwrap_err();
        assert_eq!(
            error,
            FetchError::RateLimited {
                limit: Some(60),
                reset_unix: Some(1_791_298_445),
                authenticated: false,
            }
        );
    }

    #[test]
    fn ureq_client_reports_a_refused_connection_as_a_network_failure() {
        // Bind then drop to get a port nothing listens on.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let endpoints = Endpoints {
            installer_script: format!("http://127.0.0.1:{port}/_install-rust.sh"),
            releases_api: format!("http://127.0.0.1:{port}/releases"),
            releases_feed: format!("http://127.0.0.1:{port}/releases.atom"),
        };
        let error = resolve_latest(&UreqClient::new(), &endpoints, ReleaseChannel::Stable, None)
            .unwrap_err();
        assert!(matches!(error, FetchError::Network { .. }), "{error:?}");
        assert!(error.describe(0).starts_with("Could not reach 127.0.0.1:"));
    }

    #[test]
    fn relative_time_is_readable() {
        assert_eq!(relative_time(100, 200), "now");
        assert_eq!(relative_time(230, 200), "in 30 s");
        assert_eq!(relative_time(200 + 600, 200), "in 10 min");
        assert_eq!(relative_time(200 + 3 * 3600 + 60, 200), "in 3 h 1 min");
        assert_eq!(relative_time(200 + 4 * 3600 - 10, 200), "in 4 h 0 min");
    }

    #[test]
    fn token_env_prefers_gh_token_and_ignores_blank_values() {
        let _guard = crate::version_check::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let saved: HashMap<&str, Option<std::ffi::OsString>> = ["GH_TOKEN", "GITHUB_TOKEN"]
            .iter()
            .map(|name| (*name, std::env::var_os(name)))
            .collect();
        unsafe {
            std::env::set_var("GH_TOKEN", "  ");
            std::env::set_var("GITHUB_TOKEN", "from-github-token");
        }
        assert_eq!(token_from_env().as_deref(), Some("from-github-token"));
        unsafe {
            std::env::set_var("GH_TOKEN", "from-gh-token");
        }
        assert_eq!(token_from_env().as_deref(), Some("from-gh-token"));
        unsafe {
            std::env::remove_var("GH_TOKEN");
            std::env::remove_var("GITHUB_TOKEN");
        }
        assert_eq!(token_from_env(), None);
        for (name, value) in saved {
            match value {
                Some(value) => unsafe { std::env::set_var(name, value) },
                None => unsafe { std::env::remove_var(name) },
            }
        }
    }
}
