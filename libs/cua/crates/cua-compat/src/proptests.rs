//! Property tests on the wire translation: Connect envelope framing, the
//! JSON/proto3 mappings, the multipart `/files` reader, path handling and
//! header parsing. Bounded (a fixed case count per property) so they run in
//! every PR; `PROPTEST_CASES` raises it for longer local fuzzing.

use crate::connect::{self, Code, RpcError};
use crate::e2b::{request_user, resolve_path};
use crate::guestfs::{Multipart, multipart_boundary};
use axum::body::Body;
use bytes::{Bytes, BytesMut};
use futures_util::StreamExt;
use http::{HeaderMap, header};
use http_body_util::BodyExt;
use proptest::prelude::*;
use serde_json::{Value, json};

fn cases() -> ProptestConfig {
    let n = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(128);
    ProptestConfig {
        cases: n,
        max_shrink_iters: 200,
        failure_persistence: None,
        ..ProptestConfig::default()
    }
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

/// JSON values a Connect message can carry (objects at the top).
fn json_object() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i32>().prop_map(|n| json!(n)),
        "\\PC{0,24}".prop_map(Value::String),
    ];
    let inner = leaf.prop_recursive(3, 24, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            prop::collection::btree_map("[a-zA-Z]{1,6}", inner, 0..4)
                .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    });
    prop::collection::btree_map("[a-zA-Z]{1,6}", inner, 0..5)
        .prop_map(|m| Value::Object(m.into_iter().collect()))
}

/// Splits `data` at the given fractions (each in 0..=1000 permille).
fn split(data: &[u8], cuts: &[u16]) -> Vec<Bytes> {
    let mut points: Vec<usize> = cuts
        .iter()
        .map(|c| (usize::from(*c) * data.len()) / 1000)
        .collect();
    points.push(0);
    points.push(data.len());
    points.sort_unstable();
    points.dedup();
    points
        .windows(2)
        .map(|w| Bytes::copy_from_slice(&data[w[0]..w[1]]))
        .collect()
}

proptest! {
    #![proptest_config(cases())]

    // ── Connect envelopes ────────────────────────────────────────────────

    /// A framed message comes back out exactly, whatever the flags and
    /// payload, and nothing else is consumed.
    #[test]
    fn envelope_round_trips(flags in any::<u8>(), payload in prop::collection::vec(any::<u8>(), 0..2048), tail in prop::collection::vec(any::<u8>(), 0..16)) {
        let mut buf = BytesMut::from(&connect::envelope(flags, &payload)[..]);
        buf.extend_from_slice(&tail);
        let mut b = buf.freeze();
        let (f, p) = connect::take_envelope(&mut b).unwrap().unwrap();
        prop_assert_eq!(f, flags);
        prop_assert_eq!(&p[..], &payload[..]);
        prop_assert_eq!(&b[..], &tail[..]);
    }

    /// Arbitrary bytes never panic the framer; a result consumes exactly
    /// the envelope's length.
    #[test]
    fn envelope_parser_survives_garbage(data in prop::collection::vec(any::<u8>(), 0..256)) {
        let mut b = Bytes::from(data.clone());
        match connect::take_envelope(&mut b) {
            Ok(Some((_, p))) => prop_assert_eq!(data.len(), 5 + p.len() + b.len()),
            Ok(None) => prop_assert_eq!(b.len(), data.len()),
            Err(e) => prop_assert_eq!(e.code, Code::ResourceExhausted),
        }
    }

    /// A client stream decodes the same messages however the transport
    /// chops the bytes, and stops at the end-of-stream envelope.
    #[test]
    fn client_stream_is_chunking_independent(msgs in prop::collection::vec(json_object(), 0..6), cuts in prop::collection::vec(0u16..=1000, 0..8), trailing_end in any::<bool>()) {
        let mut wire = BytesMut::new();
        for m in &msgs {
            wire.extend_from_slice(&connect::envelope(0, m.to_string().as_bytes()));
        }
        if trailing_end {
            wire.extend_from_slice(&connect::envelope(2, b"{}"));
            // Anything after the end envelope is not a message.
            wire.extend_from_slice(&connect::envelope(0, br#"{"after":"end"}"#));
        }
        let chunks: Vec<Result<Bytes, std::io::Error>> = split(&wire, &cuts).into_iter().map(Ok).collect();
        let got: Vec<Value> = rt().block_on(async {
            connect::client_stream::<Value>(Body::from_stream(futures_util::stream::iter(chunks)))
                .map(|r| r.unwrap())
                .collect()
                .await
        });
        prop_assert_eq!(got, msgs);
    }

    /// A server stream is one envelope per message, then an end-of-stream
    /// envelope (carrying the error, if any), and nothing after it.
    #[test]
    fn server_stream_framing(msgs in prop::collection::vec(json_object(), 0..5), fail in any::<bool>()) {
        let mut items: Vec<Result<Value, RpcError>> = msgs.iter().cloned().map(Ok).collect();
        if fail {
            items.push(Err(RpcError::not_found("gone")));
            items.push(Ok(json!({"never": true})));
        }
        let resp = connect::streaming(futures_util::stream::iter(items));
        let mut body = rt().block_on(async { resp.into_body().collect().await.unwrap().to_bytes() });
        let mut seen = Vec::new();
        let mut end = None;
        while let Some((f, p)) = connect::take_envelope(&mut body).unwrap() {
            if f & 2 != 0 { end = Some(serde_json::from_slice::<Value>(&p).unwrap()); break; }
            seen.push(serde_json::from_slice::<Value>(&p).unwrap());
        }
        prop_assert_eq!(seen, msgs);
        let end = end.expect("an end-of-stream envelope");
        prop_assert_eq!(end.get("error").is_some(), fail);
        prop_assert!(body.is_empty(), "bytes after the end envelope");
    }

    /// `Connect-Timeout-Ms` and the keepalive header parse without panics
    /// and only accept plain non-negative numbers.
    #[test]
    fn timeout_header_parsing(v in "[ -~]{0,24}") {
        let mut h = HeaderMap::new();
        h.insert("connect-timeout-ms", v.parse().unwrap());
        let got = connect::timeout(&h);
        prop_assert_eq!(got.map(|d| d.as_millis() as u64), v.trim().parse::<u64>().ok());
    }

    // ── multipart /files ─────────────────────────────────────────────────

    /// Any set of parts survives any chunking of the request body.
    #[test]
    fn multipart_round_trips(
        boundary in "[A-Za-z0-9'()+_,.:=?-]{1,40}",
        parts in prop::collection::vec(("[ -!#-~]{1,40}", prop::collection::vec(any::<u8>(), 0..600)), 1..5),
        cuts in prop::collection::vec(0u16..=1000, 0..10),
    ) {
        let delim = format!("\r\n--{boundary}");
        // A body that contains the delimiter would legitimately end the part.
        prop_assume!(parts.iter().all(|(_, d)| !d.windows(delim.len()).any(|w| w == delim.as_bytes())));
        let mut wire = Vec::new();
        for (name, data) in &parts {
            wire.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
            wire.extend_from_slice(data);
            wire.extend_from_slice(b"\r\n");
        }
        wire.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        let chunks: Vec<Result<Bytes, std::io::Error>> = split(&wire, &cuts).into_iter().map(Ok).collect();
        let got = rt().block_on(async {
            let mut mp = Multipart::new(futures_util::stream::iter(chunks), boundary.clone());
            let mut out = Vec::new();
            while let Some(p) = mp.next_part().await.unwrap() {
                let mut data = Vec::new();
                while let Some(c) = mp.read_chunk().await.unwrap() { data.extend_from_slice(&c); }
                out.push((p.filename.unwrap(), data));
            }
            out
        });
        prop_assert_eq!(got, parts);
    }

    /// Garbage bodies end in a part list or an error, never a panic or hang.
    #[test]
    fn multipart_survives_garbage(data in prop::collection::vec(any::<u8>(), 0..400), cuts in prop::collection::vec(0u16..=1000, 0..6)) {
        let chunks: Vec<Result<Bytes, std::io::Error>> = split(&data, &cuts).into_iter().map(Ok).collect();
        rt().block_on(async {
            let mut mp = Multipart::new(futures_util::stream::iter(chunks), "XyZ".into());
            for _ in 0..64 {
                match mp.next_part().await {
                    Ok(Some(_)) => { while let Ok(Some(_)) = mp.read_chunk().await {} }
                    _ => break,
                }
            }
        });
    }

    #[test]
    fn boundary_parsing_never_panics(ct in "\\PC{0,60}") {
        let _ = multipart_boundary(&ct);
    }

    #[test]
    fn boundary_is_what_the_header_says(b in "[A-Za-z0-9'()+_,.:=?-]{1,40}", quoted in any::<bool>()) {
        let ct = if quoted { format!("multipart/form-data; boundary=\"{b}\"") } else { format!("multipart/form-data; boundary={b}") };
        prop_assert_eq!(multipart_boundary(&ct), Some(b));
    }

    // ── paths ────────────────────────────────────────────────────────────

    /// A resolved path is absolute, lexically clean, and stable.
    #[test]
    fn resolved_paths_are_clean(path in "([a-z.~ ]{0,4}/){0,6}[a-z.~ ]{0,4}", home in "/(home/[a-z]{1,6}|root)") {
        let r = resolve_path(&path, &home);
        prop_assert!(r.starts_with('/'));
        prop_assert!(!r.contains("//"));
        prop_assert!(r == "/" || !r.ends_with('/'));
        prop_assert!(r.split('/').all(|s| s != "." && s != ".."));
        prop_assert_eq!(resolve_path(&r, "/elsewhere"), r.clone(), "not idempotent");
        if path.starts_with('/') {
            prop_assert_eq!(resolve_path(&path, "/other/home"), r);
        }
    }

    /// Relative paths are the home plus the path (dot segments resolved).
    #[test]
    fn relative_paths_hang_off_home(segs in prop::collection::vec("[a-z]{1,5}", 1..5)) {
        let rel = segs.join("/");
        prop_assert_eq!(resolve_path(&rel, "/home/user"), format!("/home/user/{rel}"));
        prop_assert_eq!(resolve_path(&format!("~/{rel}"), "/home/user"), format!("/home/user/{rel}"));
        prop_assert_eq!(resolve_path(&format!("./x/../{rel}"), "/home/user"), format!("/home/user/{rel}"));
    }

    // ── headers ──────────────────────────────────────────────────────────

    /// The user from `Authorization: Basic base64(user:)` round-trips, and
    /// arbitrary header values fall back to the default without a panic.
    #[test]
    fn basic_auth_user(user in "[a-z_][a-z0-9_-]{0,15}", junk in "[ -~]{0,40}") {
        use base64::Engine;
        let mut h = HeaderMap::new();
        h.insert(header::AUTHORIZATION, format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{user}:"))).parse().unwrap());
        prop_assert_eq!(request_user(&h, "dflt"), user);
        let mut j = HeaderMap::new();
        j.insert(header::AUTHORIZATION, junk.parse().unwrap());
        let _ = request_user(&j, "dflt");
    }
}
