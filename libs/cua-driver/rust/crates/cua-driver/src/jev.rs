// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Jev (TypeSafe System One) transport for the optional policy head.
//!
//! Lives in the `cua-driver` binary crate, not in `cua-driver-core` or a
//! per-platform `tools/` module, for the same reason `check_update_tool`
//! does: the implementation needs an HTTP client, and pulling `ureq` +
//! rustls into `cua-driver-core` would propagate that stack into every
//! platform crate's dependency graph and break cross-target builds. The
//! crate already depends on `ureq` for the telemetry and update-check
//! round-trips, so this module adds no new dependency.
//!
//! The provider is deliberately thin. It knows how to POST one
//! `{ model, state, questions }` envelope, how to classify the failures a
//! caller must react to differently (no key, bad key, unpaid, rate
//! limited, overloaded), and nothing about accessibility trees. Everything
//! element-shaped lives in [`crate::policy_tool`].

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::Value;

/// Production System One endpoint.
pub const DEFAULT_ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
/// Floating model alias. Pinned ids (`jev-1.13.0`) also work.
pub const DEFAULT_MODEL: &str = "jev-latest";

/// Bearer credential. Absent means the policy head is simply not
/// configured, and `suggest_action` is never registered.
pub const API_KEY_ENV: &str = "TYPESAFE_API_KEY";
/// Endpoint override. Used by the tests against a loopback stub, and by
/// anyone pointing the driver at a self-hosted or proxied System One.
pub const ENDPOINT_ENV: &str = "TYPESAFE_BASE_URL";
/// Model override, for pinning a specific published Jev revision.
pub const MODEL_ENV: &str = "TYPESAFE_MODEL";

/// Wall-clock ceiling for one decision. A Jev call is ~200 ms; anything
/// past a couple of seconds means the caller is better off deciding for
/// itself than waiting.
const HTTP_TIMEOUT_SECS: u64 = 10;

/// Why a Jev decision could not be produced.
///
/// Every variant is a reason for the caller to fall back to its own
/// policy rather than to retry blindly, so the rendered message says so
/// explicitly. `code` is the stable machine-readable discriminator that
/// lands in the tool's structured error payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JevError {
    /// `TYPESAFE_API_KEY` is unset or empty.
    MissingKey,
    /// 401/403 — the key is present but rejected.
    Unauthorized(u16),
    /// 402 — the account cannot be billed for this call.
    PaymentRequired,
    /// 429 — rate limited.
    RateLimited,
    /// 5xx, including TypeSafe's 529 "overloaded".
    Upstream(u16),
    /// 422 and any other non-2xx that is none of the above.
    Rejected(u16, String),
    /// Transport failure: DNS, TLS, connect, timeout.
    Transport(String),
    /// 2xx whose body was not the documented envelope.
    Malformed(String),
}

impl JevError {
    /// Stable discriminator for the structured error payload.
    pub fn code(&self) -> &'static str {
        match self {
            Self::MissingKey => "jev_not_configured",
            Self::Unauthorized(_) => "jev_unauthorized",
            Self::PaymentRequired => "jev_payment_required",
            Self::RateLimited => "jev_rate_limited",
            Self::Upstream(_) => "jev_upstream_unavailable",
            Self::Rejected(_, _) => "jev_rejected_request",
            Self::Transport(_) => "jev_unreachable",
            Self::Malformed(_) => "jev_malformed_response",
        }
    }

    /// HTTP status, when the failure came back as one.
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Unauthorized(status) | Self::Upstream(status) | Self::Rejected(status, _) => {
                Some(*status)
            }
            Self::PaymentRequired => Some(402),
            Self::RateLimited => Some(429),
            _ => None,
        }
    }

    /// One sentence of cause, then one sentence of what to do instead.
    /// Every path ends in "decide this step yourself" because none of
    /// these are recoverable inside a single `suggest_action` call.
    pub fn message(&self) -> String {
        let cause = match self {
            Self::MissingKey => {
                format!("the Jev policy head is not configured ({API_KEY_ENV} is unset)")
            }
            Self::Unauthorized(status) => {
                format!("Jev rejected the credential in {API_KEY_ENV} (HTTP {status})")
            }
            Self::PaymentRequired => {
                "the Jev account cannot be billed for this call (HTTP 402)".to_owned()
            }
            Self::RateLimited => "Jev is rate limiting this key (HTTP 429)".to_owned(),
            Self::Upstream(status) => format!("Jev is unavailable (HTTP {status})"),
            Self::Rejected(status, body) => {
                format!("Jev rejected the request (HTTP {status}): {body}")
            }
            Self::Transport(detail) => format!("Jev could not be reached: {detail}"),
            Self::Malformed(detail) => format!("Jev returned an unexpected payload: {detail}"),
        };
        format!(
            "{cause}. suggest_action is an optional accelerator, not a dependency — \
             read the window's accessibility tree and decide this step yourself."
        )
    }
}

/// One `choice` answer: the winning label, its calibrated confidence, and
/// the full distribution over the offered labels.
#[derive(Debug, Clone)]
pub struct ChoiceAnswer {
    pub choice: String,
    pub confidence: f64,
    /// Ordered so the structured payload is byte-stable across calls.
    pub probabilities: BTreeMap<String, f64>,
}

/// One decoded System One response.
#[derive(Debug, Clone)]
pub struct JevResponse {
    /// The concrete model that answered (`jev-1.13.0`), not the alias sent.
    pub model: String,
    answers: Value,
    /// Billed tokens. Output tokens are free on this endpoint and are not
    /// carried: a field that is always zero-cost reads as a cost.
    pub input_tokens: u64,
}

impl JevResponse {
    /// Read a `choice` answer by question name.
    pub fn choice(&self, name: &str) -> Result<ChoiceAnswer, JevError> {
        let answer = self
            .answers
            .get(name)
            .ok_or_else(|| JevError::Malformed(format!("answers.{name} is missing")))?;
        let choice = answer
            .get("choice")
            .and_then(Value::as_str)
            .ok_or_else(|| JevError::Malformed(format!("answers.{name}.choice is not a string")))?
            .to_owned();
        let confidence = answer
            .get("confidence")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let mut probabilities = BTreeMap::new();
        if let Some(map) = answer.get("probabilities").and_then(Value::as_object) {
            for (label, probability) in map {
                if let Some(probability) = probability.as_f64() {
                    probabilities.insert(label.clone(), probability);
                }
            }
        }
        Ok(ChoiceAnswer {
            choice,
            confidence,
            probabilities,
        })
    }

    /// Read a `noul` answer (P(true) in 0..=1) by question name.
    pub fn noul(&self, name: &str) -> Result<f64, JevError> {
        self.answers
            .get(name)
            .and_then(|answer| answer.get("noul"))
            .and_then(Value::as_f64)
            .ok_or_else(|| JevError::Malformed(format!("answers.{name}.noul is not a number")))
    }
}

/// True when a usable credential is configured. The `suggest_action` tool
/// is registered only when this holds, so a driver without a TypeSafe key
/// advertises exactly the tool list it advertised before this feature.
pub fn is_configured() -> bool {
    api_key().is_some()
}

fn api_key() -> Option<String> {
    std::env::var(API_KEY_ENV)
        .ok()
        .map(|key| key.trim().to_owned())
        .filter(|key| !key.is_empty())
}

fn endpoint() -> String {
    std::env::var(ENDPOINT_ENV)
        .ok()
        .map(|url| url.trim().to_owned())
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| DEFAULT_ENDPOINT.to_owned())
}

fn model() -> String {
    std::env::var(MODEL_ENV)
        .ok()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| DEFAULT_MODEL.to_owned())
}

/// POST one System One envelope and decode the answers.
///
/// Blocking on purpose — callers run it on a blocking pool so the MCP
/// server's runtime keeps multiplexing other tool calls during the
/// round-trip, the same way `check_for_update` does.
pub fn ask(state: &Value, questions: &Value) -> Result<JevResponse, JevError> {
    let key = api_key().ok_or(JevError::MissingKey)?;
    ask_at(&endpoint(), &key, &model(), state, questions)
}

/// [`ask`] with the endpoint, credential, and model passed explicitly.
///
/// Separated so the round-trip can be exercised against a loopback stub
/// without mutating process environment from a test thread.
pub fn ask_at(
    endpoint: &str,
    key: &str,
    model: &str,
    state: &Value,
    questions: &Value,
) -> Result<JevResponse, JevError> {
    if key.trim().is_empty() {
        return Err(JevError::MissingKey);
    }
    let body = serde_json::json!({
        "model": model,
        "state": state,
        "questions": questions,
    });
    let raw = post(endpoint, key, &body)?;
    decode(&raw)
}

fn post(url: &str, key: &str, body: &Value) -> Result<String, JevError> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(HTTP_TIMEOUT_SECS)))
        // Classify the status ourselves: a 402 and a 429 need different
        // advice, and ureq's default turns both into one opaque error.
        .http_status_as_error(false)
        .build()
        .new_agent();

    let response = agent
        .post(url)
        .header("Content-Type", "application/json")
        .header("Authorization", &format!("Bearer {key}"))
        .header(
            "User-Agent",
            concat!("cua-driver-rs/", env!("CARGO_PKG_VERSION")),
        )
        .send_json(body)
        .map_err(|error| JevError::Transport(error.to_string()))?;

    let status = response.status().as_u16();
    let text = response
        .into_body()
        .read_to_string()
        .map_err(|error| JevError::Transport(error.to_string()))?;
    classify(status, text)
}

/// Map an HTTP status onto the typed failure the caller reacts to.
/// Split out from the transport so it is testable without a socket.
fn classify(status: u16, body: String) -> Result<String, JevError> {
    match status {
        200..=299 => Ok(body),
        401 | 403 => Err(JevError::Unauthorized(status)),
        402 => Err(JevError::PaymentRequired),
        429 => Err(JevError::RateLimited),
        500..=599 => Err(JevError::Upstream(status)),
        // The body of a 4xx can echo the request, so keep only enough of
        // it to identify the complaint.
        _ => Err(JevError::Rejected(
            status,
            body.chars().take(200).collect::<String>(),
        )),
    }
}

fn decode(raw: &str) -> Result<JevResponse, JevError> {
    let parsed: Value = serde_json::from_str(raw)
        .map_err(|error| JevError::Malformed(format!("body is not JSON: {error}")))?;
    let answers = parsed
        .get("answers")
        .filter(|answers| answers.is_object())
        .ok_or_else(|| JevError::Malformed("`answers` is missing or not an object".to_owned()))?
        .clone();
    Ok(JevResponse {
        model: parsed
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned(),
        answers,
        input_tokens: parsed
            .pointer("/usage/input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    /// A one-shot loopback HTTP server. Serves exactly one request with a
    /// scripted status and body, hands back what it received, then closes.
    ///
    /// Small enough to stay in-file, and it proves the whole transport —
    /// URL, method, headers, JSON body, status classification — rather
    /// than the decoder alone.
    struct StubServer {
        url: String,
        handle: std::thread::JoinHandle<(String, String)>,
    }

    impl StubServer {
        fn serve(status: u16, reason: &str, body: &str) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
            let url = format!("http://{}/v1/systemone", listener.local_addr().unwrap());
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let handle = std::thread::spawn(move || {
                let (stream, _) = listener.accept().expect("accept");
                let mut reader = BufReader::new(stream);
                let mut headers = String::new();
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).expect("read header") == 0 {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap_or(0);
                    }
                    if line == "\r\n" {
                        break;
                    }
                    headers.push_str(&line);
                }
                let mut payload = vec![0u8; length];
                reader.read_exact(&mut payload).expect("read body");
                let mut stream = reader.into_inner();
                stream.write_all(response.as_bytes()).expect("write");
                stream.flush().ok();
                (headers, String::from_utf8_lossy(&payload).into_owned())
            });
            Self { url, handle }
        }

        /// The request headers and body the client actually sent.
        fn received(self) -> (String, String) {
            self.handle.join().expect("stub thread")
        }
    }

    #[test]
    fn a_successful_round_trip_sends_the_documented_envelope_and_decodes_the_answers() {
        let stub = StubServer::serve(200, "OK", SAMPLE);
        let response = ask_at(
            &stub.url,
            "test-key",
            "jev-latest",
            &serde_json::json!({ "goal": "open search" }),
            &serde_json::json!({ "done": { "type": "noul" } }),
        )
        .expect("stubbed call succeeds");
        assert_eq!(response.model, "jev-1.13.0");
        assert_eq!(response.choice("next").expect("next").choice, "12");

        let (headers, body) = stub.received();
        let headers = headers.to_ascii_lowercase();
        assert!(headers.contains("authorization: bearer test-key"));
        assert!(headers.contains("content-type: application/json"));
        assert!(headers.contains("cua-driver-rs/"));

        let sent: Value = serde_json::from_str(&body).expect("body is JSON");
        assert_eq!(sent["model"], serde_json::json!("jev-latest"));
        assert_eq!(sent["state"]["goal"], serde_json::json!("open search"));
        assert!(sent["questions"]["done"].is_object());
    }

    #[test]
    fn a_throttled_round_trip_becomes_a_typed_fallback_error() {
        let stub = StubServer::serve(429, "Too Many Requests", r#"{"error":"slow down"}"#);
        let error = ask_at(
            &stub.url,
            "test-key",
            "jev-latest",
            &serde_json::json!({}),
            &serde_json::json!({}),
        )
        .expect_err("429 is an error");
        assert_eq!(error, JevError::RateLimited);
        assert_eq!(error.code(), "jev_rate_limited");
        assert_eq!(error.status(), Some(429));
        assert!(error.message().contains("decide this step yourself"));
        stub.received();
    }

    #[test]
    fn an_unreachable_endpoint_is_a_transport_error_not_a_panic() {
        // Bind and immediately drop, so the port is closed but plausible.
        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            listener.local_addr().unwrap().port()
        };
        let error = ask_at(
            &format!("http://127.0.0.1:{port}/v1/systemone"),
            "test-key",
            "jev-latest",
            &serde_json::json!({}),
            &serde_json::json!({}),
        )
        .expect_err("closed port cannot answer");
        assert_eq!(error.code(), "jev_unreachable");
    }

    #[test]
    fn an_empty_credential_never_reaches_the_network() {
        let error = ask_at(
            "http://127.0.0.1:1/v1/systemone",
            "   ",
            "jev-latest",
            &serde_json::json!({}),
            &serde_json::json!({}),
        )
        .expect_err("blank key is not a credential");
        assert_eq!(error, JevError::MissingKey);
    }

    const SAMPLE: &str = r#"{
        "model": "jev-1.13.0",
        "answers": {
            "next": {
                "type": "choice",
                "choice": "12",
                "confidence": 0.97,
                "probabilities": {"12": 0.97, "4": 0.02, "__none__": 0.01}
            },
            "done": {"type": "noul", "noul": 0.04},
            "blocked": {"type": "noul", "noul": 0.11}
        },
        "usage": {"input_tokens": 441, "output_tokens": 67}
    }"#;

    #[test]
    fn decodes_the_documented_envelope() {
        let response = decode(SAMPLE).expect("sample decodes");
        assert_eq!(response.model, "jev-1.13.0");
        assert_eq!(response.input_tokens, 441);

        let next = response.choice("next").expect("next is a choice answer");
        assert_eq!(next.choice, "12");
        assert!((next.confidence - 0.97).abs() < f64::EPSILON);
        assert_eq!(next.probabilities.len(), 3);
        assert_eq!(next.probabilities.get("__none__"), Some(&0.01));

        assert!((response.noul("done").expect("done") - 0.04).abs() < f64::EPSILON);
        assert!((response.noul("blocked").expect("blocked") - 0.11).abs() < f64::EPSILON);
    }

    #[test]
    fn a_missing_answer_is_malformed_not_a_default() {
        let response = decode(SAMPLE).expect("sample decodes");
        let error = response.choice("absent").expect_err("absent has no answer");
        assert_eq!(error.code(), "jev_malformed_response");
    }

    #[test]
    fn a_body_without_answers_is_rejected() {
        let error = decode(r#"{"model": "jev-1.13.0"}"#).expect_err("no answers");
        assert_eq!(error.code(), "jev_malformed_response");
    }

    #[test]
    fn billing_and_throttling_statuses_stay_distinguishable() {
        assert_eq!(
            classify(401, String::new()).unwrap_err(),
            JevError::Unauthorized(401)
        );
        assert_eq!(
            classify(403, String::new()).unwrap_err(),
            JevError::Unauthorized(403)
        );
        assert_eq!(
            classify(402, String::new()).unwrap_err(),
            JevError::PaymentRequired
        );
        assert_eq!(
            classify(429, String::new()).unwrap_err(),
            JevError::RateLimited
        );
        assert_eq!(
            classify(529, String::new()).unwrap_err(),
            JevError::Upstream(529)
        );
        assert_eq!(classify(200, "{}".to_owned()).unwrap(), "{}");
    }

    #[test]
    fn every_failure_tells_the_caller_to_fall_back() {
        for error in [
            JevError::MissingKey,
            JevError::Unauthorized(401),
            JevError::PaymentRequired,
            JevError::RateLimited,
            JevError::Upstream(529),
            JevError::Rejected(422, "bad question".to_owned()),
            JevError::Transport("connection refused".to_owned()),
            JevError::Malformed("not JSON".to_owned()),
        ] {
            let message = error.message();
            assert!(
                message.contains("decide this step yourself"),
                "{} must advise a fallback, said: {message}",
                error.code()
            );
        }
    }

    #[test]
    fn a_rejected_body_is_truncated_so_an_echoed_request_cannot_flood_the_error() {
        let error = classify(422, "x".repeat(5_000)).unwrap_err();
        let JevError::Rejected(_, body) = &error else {
            panic!("422 must classify as rejected, got {error:?}");
        };
        assert_eq!(body.len(), 200);
    }
}
