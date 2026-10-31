//! Parsing, classification, and cursor bookkeeping shared by the platform
//! browser adapters.
//!
//! Every platform adapter reads Chromium's `DevToolsActivePort` file,
//! re-proves that a discovered websocket stays on the attested loopback
//! listener, reads the `/json/version` discovery surface, recognizes Firefox,
//! and keeps one visible browser cursor per native window. Those semantics are platform-independent, so they live here
//! and the adapters stay thin.

use std::collections::HashMap;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Parse Chromium's `DevToolsActivePort` file.
///
/// The file must hold exactly two non-empty lines: the listener port and one
/// `/devtools/browser/<id>` path whose id is ASCII alphanumeric, `-`, or `_`.
/// Anything else, including page paths and trailing lines, is refused so the
/// caller never attaches to an endpoint the file does not name exactly.
pub fn parse_devtools_active_port(text: &str) -> Option<(u16, &str)> {
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let port = lines.next()?.parse::<u16>().ok()?;
    let path = lines.next()?;
    if lines.next().is_some() {
        return None;
    }
    let instance = path.strip_prefix("/devtools/browser/")?;
    (!instance.is_empty()
        && instance
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'))
    .then_some((port, path))
}

/// Return the port of a loopback DevTools websocket URL.
///
/// Accepts `127.0.0.1`, `localhost`, and `[::1]` hosts only, so a
/// discovered URL that points anywhere else can never match the attested
/// listener port.
pub fn loopback_websocket_port(url: &str) -> Option<u16> {
    ["ws://127.0.0.1:", "ws://localhost:", "ws://[::1]:"]
        .iter()
        .find_map(|prefix| {
            url.strip_prefix(prefix)?
                .split('/')
                .next()?
                .parse::<u16>()
                .ok()
        })
}

/// Read `webSocketDebuggerUrl` from `GET /json/version` on `127.0.0.1:<port>`.
///
/// Uses a raw HTTP/1.1 request under a 2 second timeout and refuses responses
/// over 256 KiB. Returns `None` on any failure. The URL is returned exactly as
/// the browser reported it: callers must re-prove it against the attested
/// listener port (and canonicalize it if they need to) before trusting it.
pub async fn fetch_json_version_ws_url(port: u16) -> Option<String> {
    tokio::time::timeout(Duration::from_secs(2), async move {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .ok()?;
        let request = format!(
            "GET /json/version HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).await.ok()?;
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let read = stream.read(&mut chunk).await.ok()?;
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..read]);
            if bytes.len() > 256 * 1024 {
                return None;
            }
            if let Some(header_end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..header_end]);
                if let Some(length) = headers.lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                }) {
                    if bytes.len() >= header_end + 4 + length {
                        break;
                    }
                }
            }
        }
        let body_start = bytes.windows(4).position(|part| part == b"\r\n\r\n")? + 4;
        let value: serde_json::Value = serde_json::from_slice(&bytes[body_start..]).ok()?;
        Some(value.get("webSocketDebuggerUrl")?.as_str()?.to_owned())
    })
    .await
    .ok()
    .flatten()
}

/// Whether a process identity names Firefox.
///
/// `identity` is whatever the adapter has for the process: an executable
/// name, an executable path, or an app name joined with its bundle id. The
/// identity is split on every non-alphanumeric character and matches when one
/// token is exactly `firefox`, so `firefox.exe`, `/usr/lib/firefox-esr/firefox-esr`
/// and `org.mozilla.firefox` match while `FirefoxHelper` and `waterfox` do
/// not.
pub fn is_firefox(identity: &str) -> bool {
    identity
        .to_ascii_lowercase()
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|token| token == "firefox")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BrowserCursorBinding {
    window_id: u64,
    cdp_target_id: String,
}

/// Session-to-tab bindings for browser cursors.
///
/// An active tab owns the sole visible browser cursor in its native window.
/// An inactive tab hides only its own cursor and never disturbs the cursor for
/// the selected tab.
#[derive(Debug, Default)]
pub struct BrowserCursorTracker {
    bindings: HashMap<String, BrowserCursorBinding>,
}

impl BrowserCursorTracker {
    /// Record the session-to-tab binding and return the exact overlay
    /// visibility changes, as `(session, visible)` pairs, needed for this
    /// action.
    pub fn update(
        &mut self,
        session: &str,
        window_id: u64,
        cdp_target_id: &str,
        tab_is_active: bool,
    ) -> Vec<(String, bool)> {
        self.bindings.insert(
            session.to_owned(),
            BrowserCursorBinding {
                window_id,
                cdp_target_id: cdp_target_id.to_owned(),
            },
        );

        if !tab_is_active {
            return vec![(session.to_owned(), false)];
        }

        self.bindings
            .iter()
            .filter(|(_, binding)| binding.window_id == window_id)
            .map(|(key, binding)| {
                (
                    key.clone(),
                    key == session && binding.cdp_target_id == cdp_target_id,
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn active_port_parser_requires_one_exact_browser_path() {
        assert_eq!(
            parse_devtools_active_port(
                "9222\n/devtools/browser/f1d991b4-2694-4b28-b63a-1f2a8da3a435\n"
            ),
            Some((
                9222,
                "/devtools/browser/f1d991b4-2694-4b28-b63a-1f2a8da3a435"
            ))
        );
        assert_eq!(
            parse_devtools_active_port("9222\n/devtools/browser/abc_123\n"),
            Some((9222, "/devtools/browser/abc_123"))
        );
        for refused in [
            "9222\n/devtools/browser\n",
            "9222\n/devtools/browser/\n",
            "9222\n/devtools/page/id\n",
            "9222\n/devtools/browser/id\nextra\n",
            "9222\n/devtools/browser/../page\n",
            "not-a-port\n/devtools/browser/id\n",
            "9222\n",
        ] {
            assert_eq!(parse_devtools_active_port(refused), None, "{refused:?}");
        }
    }

    #[test]
    fn websocket_url_must_keep_the_attested_listener_port() {
        for (url, port) in [
            ("ws://127.0.0.1:9222/devtools/browser/id", Some(9222)),
            ("ws://localhost:9333/devtools/browser/id", Some(9333)),
            ("ws://[::1]:9444/devtools/browser/id", Some(9444)),
            ("ws://0.0.0.0:9222/devtools", None),
            ("ws://192.0.2.1:9222/devtools", None),
            ("wss://127.0.0.1:9222/devtools", None),
        ] {
            assert_eq!(loopback_websocket_port(url), port, "{url}");
        }
    }

    #[test]
    fn firefox_classifier_uses_product_tokens() {
        for identity in [
            "firefox.exe",
            "Mozilla Firefox.exe",
            "Firefox org.mozilla.firefox",
            "/usr/lib/firefox/firefox",
            "/usr/lib/firefox-esr/firefox-esr",
        ] {
            assert!(is_firefox(identity), "{identity}");
        }
        for identity in [
            "FirefoxHelper.exe",
            "FirefoxHelper com.example.FirefoxHelper",
            "waterfox",
            "Waterfox net.waterfox.current",
        ] {
            assert!(!is_firefox(identity), "{identity}");
        }
    }

    #[test]
    fn browser_cursor_tracker_shows_only_the_active_tabs_session_per_window() {
        let mut tracker = BrowserCursorTracker::default();
        assert_eq!(
            tracker.update("session-red", 77, "tab-A", false),
            vec![("session-red".to_owned(), false)]
        );

        let first_active = tracker.update("session-red", 77, "tab-A", true);
        assert_eq!(first_active, vec![("session-red".to_owned(), true)]);

        let second_active = tracker
            .update("session-blue", 77, "tab-B", true)
            .into_iter()
            .collect::<HashMap<_, _>>();
        assert_eq!(second_active.get("session-red"), Some(&false));
        assert_eq!(second_active.get("session-blue"), Some(&true));

        let mut back_to_red = tracker.update("session-red", 77, "tab-A", true);
        back_to_red.sort();
        assert_eq!(
            back_to_red,
            vec![
                ("session-blue".to_owned(), false),
                ("session-red".to_owned(), true)
            ]
        );

        let other_window = tracker.update("session-green", 88, "tab-C", true);
        assert_eq!(
            other_window,
            vec![("session-green".to_owned(), true)],
            "an active tab in another native window must not hide this window"
        );
    }

    /// Serve one canned response per accepted connection and report each
    /// request. `hold_open` keeps the socket open after the response.
    async fn serve_once(
        response: Vec<u8>,
        hold_open: bool,
    ) -> (u16, tokio::sync::oneshot::Receiver<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0u8; 4096];
            let read = stream.read(&mut request).await.unwrap();
            let _ = tx.send(String::from_utf8_lossy(&request[..read]).into_owned());
            let _ = stream.write_all(&response).await;
            if hold_open {
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        });
        (port, rx)
    }

    fn http_ok(body: &str, with_length: bool) -> Vec<u8> {
        let length = if with_length {
            format!("Content-Length: {}\r\n", body.len())
        } else {
            String::new()
        };
        format!("HTTP/1.1 200 OK\r\n{length}Connection: close\r\n\r\n{body}").into_bytes()
    }

    const CAP: usize = 256 * 1024;

    /// A valid response of exactly `total` bytes, headers included.
    fn response_of_total_size(total: usize) -> Vec<u8> {
        let build = |pad: usize| {
            let body = format!(
                "{{\"webSocketDebuggerUrl\":\"ws://127.0.0.1:1/devtools/browser/id\",\"pad\":\"{}\"}}",
                "a".repeat(pad)
            );
            http_ok(&body, true)
        };
        let base = build(0).len();
        // The Content-Length digit count can change with the pad; solve by retry.
        for pad in total.saturating_sub(base + 8)..=total {
            let bytes = build(pad);
            if bytes.len() == total {
                return bytes;
            }
        }
        panic!("no response of {total} bytes");
    }

    #[tokio::test]
    async fn json_version_probe_sends_the_exact_request_and_returns_the_raw_url() {
        let body = r#"{"webSocketDebuggerUrl":"ws://localhost:1/devtools/browser/id"}"#;
        let (port, request) = serve_once(http_ok(body, true), true).await;
        let url = fetch_json_version_ws_url(port).await;
        assert_eq!(
            request.await.unwrap(),
            format!(
                "GET /json/version HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
            )
        );
        // Raw URL: neither host nor port is canonicalised or checked here, and
        // the open socket proves the read stopped at Content-Length.
        assert_eq!(url.as_deref(), Some("ws://localhost:1/devtools/browser/id"));
    }

    #[tokio::test]
    async fn json_version_probe_reads_to_eof_without_content_length() {
        let body = r#"{"webSocketDebuggerUrl":"ws://127.0.0.1:1/devtools/browser/id"}"#;
        let (port, _) = serve_once(http_ok(body, false), false).await;
        assert_eq!(
            fetch_json_version_ws_url(port).await.as_deref(),
            Some("ws://127.0.0.1:1/devtools/browser/id")
        );
    }

    #[tokio::test]
    async fn json_version_probe_refuses_oversized_missing_or_malformed_bodies() {
        let oversized = format!("{{\"pad\":\"{}\"}}", "a".repeat(CAP));
        for body in [
            oversized.as_str(),
            "{}",
            r#"{"webSocketDebuggerUrl":7}"#,
            "not json",
        ] {
            let (port, _) = serve_once(http_ok(body, true), false).await;
            assert_eq!(fetch_json_version_ws_url(port).await, None, "{body:.40}");
        }
        let closed = {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            listener.local_addr().unwrap().port()
        };
        assert_eq!(fetch_json_version_ws_url(closed).await, None);
    }

    #[tokio::test]
    async fn json_version_probe_cap_boundary_is_exactly_256_kib() {
        let at_cap = response_of_total_size(CAP);
        assert_eq!(at_cap.len(), CAP);
        let (port, _) = serve_once(at_cap, false).await;
        assert_eq!(
            fetch_json_version_ws_url(port).await.as_deref(),
            Some("ws://127.0.0.1:1/devtools/browser/id")
        );

        let over_cap = response_of_total_size(CAP + 1);
        assert_eq!(over_cap.len(), CAP + 1);
        let (port, _) = serve_once(over_cap, false).await;
        assert_eq!(fetch_json_version_ws_url(port).await, None);
    }

    #[tokio::test]
    async fn json_version_probe_times_out_after_two_seconds() {
        let (port, _) = serve_once(Vec::new(), true).await;
        let started = std::time::Instant::now();
        let url = tokio::time::timeout(Duration::from_secs(5), fetch_json_version_ws_url(port))
            .await
            .expect("probe must give up on its own timeout");
        assert_eq!(url, None);
        assert!(started.elapsed() >= Duration::from_secs(2));
    }
}
