// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! cua-spacesd conformance suite, run over native gRPC and gRPC-Web with
//! the cua SDK client (`cua-spacesd-client`). See `common/mod.rs` for targets and knobs.

mod common;

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use cua_proto::env::v1::process_data::Output;
use cua_proto::env::v1::process_event::Event;
use cua_proto::env::v1::process_input::Input;
use cua_proto::env::v1::process_selector::Selector;
use cua_proto::env::v1::read_file_response::Message as ReadMessage;
use cua_proto::env::v1::watch_dir_response::Message as WatchMessage;
use cua_proto::env::v1::*;
use cua_spacesd_client::TransportPreference;
use futures_util::{SinkExt, StreamExt};
use sha2::Digest;
use tonic::Code;

use common::*;

fn reason(status: &tonic::Status) -> i32 {
    cua_spacesd_server::error::error_info(status)
        .map(|i| i.reason)
        .unwrap_or_default()
}

fn sh(script: &str) -> ProcessConfig {
    ProcessConfig {
        command: SH.into(),
        args: vec!["-c".into(), script.into()],
        ..Default::default()
    }
}

fn tag(tag: &str) -> Option<ProcessSelector> {
    Some(ProcessSelector {
        selector: Some(Selector::Tag(tag.into())),
    })
}

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::new_v4().simple())
}

// ---------------------------------------------------------------------------
// Auth, capabilities, routes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn auth_is_required_and_either_header_works() {
    let t = target().await;
    for transport in TRANSPORTS {
        let anonymous = t.client_with_token(transport, None).await;
        let error = anonymous
            .system()
            .get_capabilities(GetCapabilitiesRequest {})
            .await
            .unwrap_err();
        assert_eq!(error.code(), Code::Unauthenticated, "{transport:?}");
        assert_eq!(reason(&error), ErrorReason::Unauthenticated as i32);

        let wrong = t.client_with_token(transport, Some("wrong".into())).await;
        assert_eq!(
            wrong
                .system()
                .health(HealthRequest {})
                .await
                .unwrap_err()
                .code(),
            Code::Unauthenticated
        );

        // The Fleet gateway strips `authorization`; the env token rides in
        // x-cua-env-authorization instead.
        let mut request = tonic::Request::new(GetCapabilitiesRequest {});
        request.metadata_mut().insert(
            cua_proto::metadata::ENV_AUTHORIZATION,
            format!("Bearer {}", t.token).parse().unwrap(),
        );
        request
            .metadata_mut()
            .insert("authorization", "Bearer some-gateway-jwt".parse().unwrap());
        anonymous
            .system()
            .get_capabilities(request)
            .await
            .unwrap_or_else(|e| panic!("{transport:?} x-cua-env-authorization: {e}"));
    }
    // /health is unauthenticated plain HTTP.
    let response = http_client()
        .get(t.endpoint().http_url("/health").parse().unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), http::StatusCode::NO_CONTENT);
    t.cleanup().await;
}

#[tokio::test]
async fn capabilities_report_contract_and_limitations() {
    let t = target().await;
    for transport in TRANSPORTS {
        let c = t.client(transport).await;
        let caps = c
            .system()
            .get_capabilities(GetCapabilitiesRequest {})
            .await
            .unwrap()
            .into_inner();
        assert_eq!(caps.protocol_version, cua_proto::ENV_PROTOCOL_VERSION);
        assert_eq!(caps.protocol_revision, cua_proto::ENV_PROTOCOL_REVISION);
        let names: BTreeSet<_> = caps.features.iter().map(|f| f.name.as_str()).collect();
        for required in [
            "pty",
            "fs_watch",
            "tunnel.forward",
            "hotspot",
            "driver",
            "a11y",
            "desktop_stream",
            "audio.uplink",
        ] {
            assert!(names.contains(required), "missing feature {required}");
        }
        for feature in &caps.features {
            if !feature.supported {
                assert!(
                    !feature.limitation.is_empty(),
                    "{} unsupported without limitation",
                    feature.name
                );
            }
        }
        let limits = caps.limits.unwrap();
        assert_eq!(limits.max_chunk_bytes, 4 * 1024 * 1024);
        let side = caps.side_channels.unwrap();
        assert_eq!(side.files_http_path, "/files");
        assert_eq!(side.tunnel_ws_path, "/tunnel");
    }
    if t.local.is_some() {
        // Without a desktop provider the desktop services answer with a
        // typed FEATURE_UNSUPPORTED, and the matching features say so.
        let c = t.client(TransportPreference::GrpcWeb).await;
        let error = c
            .computer()
            .list_displays(ListDisplaysRequest::default())
            .await
            .unwrap_err();
        assert_eq!(error.code(), Code::FailedPrecondition);
        let info = cua_spacesd_server::error::error_info(&error).unwrap();
        assert_eq!(info.reason, ErrorReason::FeatureUnsupported as i32);
        assert_eq!(info.feature, "background_input");
        let caps = c
            .system()
            .get_capabilities(GetCapabilitiesRequest {})
            .await
            .unwrap()
            .into_inner();
        let a11y = caps.features.iter().find(|f| f.name == "a11y").unwrap();
        assert!(!a11y.supported);
    }
    t.cleanup().await;
}

/// Every HTTP route is in the spec, every spec path is served, every
/// contract service is registered, and no contract method is UNIMPLEMENTED.
#[tokio::test]
async fn route_spec_consistency() {
    let t = target().await;
    let Some(local) = &t.local else { return };
    let spec_paths: BTreeSet<String> = [
        cua_proto::metadata::HEALTH_PATH,
        cua_proto::metadata::MEDIA_WS_PATH,
        cua_proto::metadata::TUNNEL_WS_PATH,
        cua_proto::metadata::HOTSPOT_WS_PATH,
        cua_proto::metadata::VOLUME_WS_PATH,
        cua_proto::metadata::FILES_PATH,
        cua_proto::metadata::MCP_PATH,
        cua_proto::metadata::VIEWER_PATH,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(
        local.manifest.http_paths, spec_paths,
        "HTTP routes must equal the spec"
    );

    let set =
        <prost_types::FileDescriptorSet as prost::Message>::decode(cua_proto::FILE_DESCRIPTOR_SET)
            .unwrap();
    let mut methods = Vec::new();
    let mut services = BTreeSet::new();
    for file in &set.file {
        if file.package() != "cua.env.v1" {
            continue;
        }
        for service in &file.service {
            let name = format!("{}.{}", file.package(), service.name());
            for method in &service.method {
                methods.push((
                    format!("/{name}/{}", method.name()),
                    method.client_streaming(),
                ));
            }
            services.insert(name);
        }
    }
    let registered: BTreeSet<String> = local
        .manifest
        .grpc_services
        .union(&local.manifest.stubbed_services)
        .cloned()
        .collect();
    for service in &services {
        assert!(registered.contains(service), "{service} is not registered");
    }
    assert!(
        methods.len() > 60,
        "descriptor has {} methods",
        methods.len()
    );

    // Probe each method with an empty message over raw h2c: a registered
    // method never answers UNIMPLEMENTED.
    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .http2_only(true)
        .build_http::<http_body_util::Full<bytes::Bytes>>();
    for (path, _client_streaming) in methods {
        if path.ends_with("/Shutdown") || path.ends_with("/Init") {
            continue; // effects; covered elsewhere
        }
        let request = http::Request::post(format!("{}{path}", t.url))
            .header("content-type", "application/grpc")
            .header("te", "trailers")
            .header("authorization", format!("Bearer {}", t.token))
            .body(http_body_util::Full::new(bytes::Bytes::from_static(&[
                0, 0, 0, 0, 0,
            ])))
            .unwrap();
        let response = tokio::time::timeout(Duration::from_secs(10), client.request(request))
            .await
            .unwrap_or_else(|_| panic!("{path}: no response headers"))
            .unwrap();
        assert_eq!(response.status(), http::StatusCode::OK, "{path}");
        let header_status = response
            .headers()
            .get("grpc-status")
            .map(|v| v.to_str().unwrap().to_owned());
        let status = match header_status {
            Some(s) => s,
            None => {
                // Streaming response: read trailers (bounded).
                use http_body_util::BodyExt;
                let mut body = response.into_body();
                let mut found = None;
                for _ in 0..64 {
                    match tokio::time::timeout(Duration::from_secs(2), body.frame()).await {
                        Ok(Some(Ok(frame))) => {
                            if let Some(trailers) = frame.trailers_ref() {
                                found = trailers
                                    .get("grpc-status")
                                    .map(|v| v.to_str().unwrap().to_owned());
                                break;
                            }
                        }
                        _ => break,
                    }
                }
                // A stream that is still open is served, not unimplemented.
                found.unwrap_or_else(|| "open".into())
            }
        };
        assert_ne!(status, "12", "{path} is UNIMPLEMENTED");
    }
}

#[tokio::test]
async fn init_is_idempotent_and_rotates_the_token() {
    let t = target().await;
    let Some(local) = &t.local else { return };
    let c = t.client(TransportPreference::Native).await;
    let init = InitRequest {
        env: [("CUA_CONFORMANCE".to_owned(), "yes".to_owned())].into(),
        audio_uplink: Some(AudioUplinkAccess {
            mode: AudioUplinkMode::Allowlist as i32,
            principal_ids: vec!["alice".into()],
        }),
        ..Default::default()
    };
    let first = c.system().init(init.clone()).await.unwrap().into_inner();
    let second = c.system().init(init).await.unwrap().into_inner();
    assert!(!first.token_changed && !second.token_changed);
    assert!(local.ctx.audio_uplink_allowed(Some(&Principal {
        id: "alice".into(),
        ..Default::default()
    })));
    assert!(!local.ctx.audio_uplink_allowed(Some(&Principal {
        id: "bob".into(),
        ..Default::default()
    })));
    let (code, out) = run_sh(
        &c,
        "printf %s \"$CUA_CONFORMANCE\"; printf %s \"${CUA_ENV_TOKEN:-scrubbed}\"",
    )
    .await;
    assert_eq!(code, Some(0));
    assert_eq!(out, b"yesscrubbed");
    let caps = c
        .system()
        .get_capabilities(GetCapabilitiesRequest {})
        .await
        .unwrap()
        .into_inner();
    assert!(caps.initialized);

    let rotated = c
        .system()
        .init(InitRequest {
            token: "rotated".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    assert!(rotated.token_changed);
    assert_eq!(
        c.system()
            .health(HealthRequest {})
            .await
            .unwrap_err()
            .code(),
        Code::Unauthenticated
    );
    let fresh = t
        .client_with_token(TransportPreference::GrpcWeb, Some("rotated".into()))
        .await;
    fresh.system().health(HealthRequest {}).await.unwrap();
    let metrics = fresh
        .system()
        .metrics(MetricsRequest::default())
        .await
        .unwrap()
        .into_inner();
    assert!(
        metrics.cpu_count > 0 && metrics.memory_total_bytes > 0 && metrics.disk_total_bytes > 0
    );
}

// ---------------------------------------------------------------------------
// Process
// ---------------------------------------------------------------------------

#[tokio::test]
async fn process_basics_on_both_transports() {
    let t = target().await;
    for transport in TRANSPORTS {
        let c = t.client(transport).await;
        let (code, out) = run_sh(&c, "echo hello; echo oops >&2; exit 3").await;
        assert_eq!(code, Some(3));
        assert_eq!(out, b"hello\n");

        // Executable not found is a ProcessEnd with an error, not an RPC error.
        let mut stream = c
            .process()
            .start_process(StartProcessRequest {
                config: Some(ProcessConfig {
                    command: "/definitely/not/here".into(),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .await
            .unwrap()
            .into_inner();
        let mut error = String::new();
        for _ in 0..4 {
            match stream.message().await.unwrap() {
                Some(m) => {
                    if let Some(Event::End(end)) = m.event.and_then(|e| e.event) {
                        error = end.error;
                    }
                }
                None => break,
            }
        }
        assert!(error.contains("not found"), "{error:?}");
    }
    t.cleanup().await;
}

#[tokio::test]
async fn signals_stdin_and_sequenced_input() {
    let t = target().await;
    let c = t.client(TransportPreference::GrpcWeb).await;
    let name = unique("cat");
    let mut stream = c
        .process()
        .start_process(StartProcessRequest {
            config: Some(sh("cat")),
            tag: name.clone(),
            stdin: true,
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    for (seq, text) in [(5, "a"), (6, "b"), (6, "b"), (7, "c")] {
        let r = c
            .process()
            .send_input(SendInputRequest {
                process: tag(&name),
                input: Some(ProcessInput {
                    input: Some(Input::Stdin(text.as_bytes().to_vec())),
                }),
                sequence: seq,
                writer_id: "w".into(),
            })
            .await
            .unwrap()
            .into_inner();
        assert_eq!(r.applied_sequence, seq);
    }
    let gap = c
        .process()
        .send_input(SendInputRequest {
            process: tag(&name),
            input: Some(ProcessInput {
                input: Some(Input::Stdin(b"z".to_vec())),
            }),
            sequence: 9,
            writer_id: "w".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(reason(&gap), ErrorReason::SequenceGap as i32);
    c.process()
        .close_stdin(CloseStdinRequest {
            process: tag(&name),
        })
        .await
        .unwrap();
    let mut out = Vec::new();
    let mut code = None;
    for _ in 0..1000 {
        let Some(m) = stream.message().await.unwrap() else {
            break;
        };
        match m.event.and_then(|e| e.event) {
            Some(Event::Data(d)) => {
                if let Some(Output::Stdout(b)) = d.output {
                    out.extend(b)
                }
            }
            Some(Event::End(e)) => {
                code = e.exit_code;
                break;
            }
            _ => {}
        }
    }
    assert_eq!(out, b"abc", "duplicates are not re-applied");
    assert_eq!(code, Some(0));

    // Signals.
    let name = unique("sleep");
    let mut stream = c
        .process()
        .start_process(StartProcessRequest {
            config: Some(sh("exec sleep 100")),
            tag: name.clone(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    stream.message().await.unwrap(); // ProcessStart
    c.process()
        .signal_process(SignalProcessRequest {
            process: tag(&name),
            signal: Signal::Term as i32,
            process_group: true,
        })
        .await
        .unwrap();
    let mut end = None;
    for _ in 0..100 {
        let Some(m) = tokio::time::timeout(Duration::from_secs(10), stream.message())
            .await
            .unwrap()
            .unwrap()
        else {
            break;
        };
        if let Some(Event::End(e)) = m.event.and_then(|e| e.event) {
            end = Some(e);
            break;
        }
    }
    let end = end.expect("the signalled process ends");
    // Windows has no signals: SIGTERM terminates the process, and the end
    // carries its exit code instead of a signal.
    if cfg!(windows) {
        assert!(end.exit_code.is_some() && end.error.is_empty(), "{end:?}");
    } else {
        assert_eq!(end.signal, Signal::Term as i32);
    }

    // Per-process timeout.
    let started = Instant::now();
    let mut stream = c
        .process()
        .start_process(StartProcessRequest {
            config: Some(ProcessConfig {
                timeout: Some(cua_proto::wkt::Duration {
                    seconds: 1,
                    nanos: 0,
                }),
                ..sh("exec sleep 100")
            }),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    let mut timed_out = false;
    for _ in 0..100 {
        let Some(m) = tokio::time::timeout(Duration::from_secs(15), stream.message())
            .await
            .unwrap()
            .unwrap()
        else {
            break;
        };
        if let Some(Event::End(e)) = m.event.and_then(|e| e.event) {
            timed_out = e.timed_out;
            break;
        }
    }
    assert!(timed_out && started.elapsed() < Duration::from_secs(10));
    t.cleanup().await;
}

#[tokio::test]
async fn sixty_four_mib_of_stdout_round_trips() {
    let t = target().await;
    for transport in TRANSPORTS {
        let c = t.client(transport).await;
        // 64 MiB from a pattern the test can reproduce: the 16-byte line
        // "0123456789abcde\n" repeated.
        let total: u64 = 64 * 1024 * 1024;
        let mut stream = c
            .process()
            .start_process(StartProcessRequest {
                config: Some(sh(&format!("yes 0123456789abcde | head -c {total}"))),
                scrollback_bytes: total + 1024,
                ..Default::default()
            })
            .await
            .unwrap()
            .into_inner();
        let mut hasher = sha2::Sha256::new();
        let mut next = 0u64;
        let mut ended = false;
        for _ in 0..1_000_000 {
            let Some(m) = tokio::time::timeout(Duration::from_secs(60), stream.message())
                .await
                .unwrap()
                .unwrap()
            else {
                break;
            };
            match m.event.and_then(|e| e.event) {
                Some(Event::Data(d)) => {
                    assert_eq!(d.offset, next, "{transport:?}: output gap");
                    if let Some(Output::Stdout(b)) = d.output {
                        next += b.len() as u64;
                        hasher.update(&b);
                    }
                }
                Some(Event::End(e)) => {
                    assert_eq!(e.exit_code, Some(0));
                    ended = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(ended);
        assert_eq!(next, total);
        let mut expected = sha2::Sha256::new();
        let line = b"0123456789abcde\n";
        let block: Vec<u8> = line.iter().copied().cycle().take(1024 * 1024).collect();
        for _ in 0..64 {
            expected.update(&block);
        }
        assert_eq!(hasher.finalize(), expected.finalize(), "{transport:?}");
    }
    t.cleanup().await;
}

// Unix only: on Windows the test shell is Git for Windows' sh under ConPTY,
// whose `stty size` never answered here (the read timed out on the
// windows-latest runner), so it cannot observe a resize.
#[cfg(unix)]
#[tokio::test]
async fn pty_resize_is_observed_by_stty() {
    let t = target().await;
    let c = t.client(TransportPreference::Native).await;
    let name = unique("pty");
    let mut stream = c
        .process()
        .start_process(StartProcessRequest {
            config: Some(sh("stty size; read line; stty size")),
            pty: Some(PtyConfig {
                size: Some(PtySize {
                    cols: 100,
                    rows: 30,
                    ..Default::default()
                }),
                term: String::new(),
            }),
            tag: name.clone(),
            stdin: true,
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    let mut text = String::new();
    let mut resized = false;
    for _ in 0..10_000 {
        let Some(m) = tokio::time::timeout(Duration::from_secs(20), stream.message())
            .await
            .unwrap()
            .unwrap()
        else {
            break;
        };
        match m.event.and_then(|e| e.event) {
            Some(Event::Data(d)) => {
                if let Some(Output::Pty(b)) = d.output {
                    text.push_str(&String::from_utf8_lossy(&b));
                }
                if !resized && text.contains("30 100") {
                    resized = true;
                    c.process()
                        .resize_pty(ResizePtyRequest {
                            process: tag(&name),
                            size: Some(PtySize {
                                cols: 120,
                                rows: 40,
                                ..Default::default()
                            }),
                        })
                        .await
                        .unwrap();
                    c.process()
                        .send_input(SendInputRequest {
                            process: tag(&name),
                            input: Some(ProcessInput {
                                input: Some(Input::Pty(b"go\n".to_vec())),
                            }),
                            ..Default::default()
                        })
                        .await
                        .unwrap();
                }
            }
            Some(Event::End(_)) => break,
            _ => {}
        }
    }
    assert!(text.contains("30 100"), "{text:?}");
    assert!(text.contains("40 120"), "{text:?}");
    t.cleanup().await;
}

/// A long-running command streamed through gRPC-Web with keepalives: the
/// client disconnects mid-stream, reattaches by tag over native gRPC and
/// replays scrollback, and the stream runs to completion.
#[tokio::test]
async fn long_running_stream_keepalive_and_reattach_by_tag() {
    let t = target().await;
    let secs = long_secs().max(4);
    let name = unique("long");
    let web = t.client(TransportPreference::GrpcWeb).await;
    let mut stream = web
        .process()
        .start_process(StartProcessRequest {
            config: Some(sh(&format!(
                "i=0; while [ $i -lt {secs} ]; do echo tick $i; i=$((i+1)); sleep 1; done"
            ))),
            tag: name.clone(),
            keepalive_interval: Some(cua_proto::wkt::Duration {
                seconds: 0,
                nanos: 400_000_000,
            }),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    let mut keepalives = 0;
    let mut first_half = String::new();
    let cut = Instant::now() + Duration::from_secs(secs / 2);
    while Instant::now() < cut {
        let m = tokio::time::timeout(Duration::from_secs(5), stream.message())
            .await
            .expect("keepalive interval exceeded")
            .unwrap()
            .expect("stream ended early");
        match m.event.and_then(|e| e.event) {
            Some(Event::Keepalive(_)) => keepalives += 1,
            Some(Event::Data(d)) => {
                if let Some(Output::Stdout(b)) = d.output {
                    first_half.push_str(&String::from_utf8_lossy(&b));
                }
            }
            Some(Event::End(_)) => panic!("ended before the cut"),
            _ => {}
        }
    }
    assert!(keepalives >= 1, "no keepalives in {}s", secs / 2);
    assert!(first_half.contains("tick 0"));
    drop(stream); // client goes away; the process keeps running

    let native = t.client(TransportPreference::Native).await;
    let list = native
        .process()
        .list_processes(ListProcessesRequest {
            include_exited: false,
            tag_prefix: name.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(list.processes.len(), 1, "detached process still running");
    let mut stream = native
        .process()
        .connect_process(ConnectProcessRequest {
            process: tag(&name),
            replay_from_offset: Some(0),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    let mut all = String::new();
    let mut exit = None;
    for _ in 0..100_000 {
        let Some(m) = tokio::time::timeout(Duration::from_secs(secs + 30), stream.message())
            .await
            .unwrap()
            .unwrap()
        else {
            break;
        };
        match m.event.and_then(|e| e.event) {
            Some(Event::Start(s)) => assert_eq!(s.scrollback_start_offset, 0),
            Some(Event::Data(d)) => {
                if let Some(Output::Stdout(b)) = d.output {
                    all.push_str(&String::from_utf8_lossy(&b));
                }
            }
            Some(Event::End(e)) => {
                exit = e.exit_code;
                break;
            }
            _ => {}
        }
    }
    assert_eq!(exit, Some(0));
    for i in 0..secs {
        assert!(
            all.contains(&format!("tick {i}\n")),
            "missing tick {i} after reattach"
        );
    }
    // Exit retention: a late ConnectProcess still sees the end.
    let mut late = native
        .process()
        .connect_process(ConnectProcessRequest {
            process: tag(&name),
            replay_bytes: 8,
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    let mut saw_end = false;
    for _ in 0..10 {
        let Some(m) = late.message().await.unwrap() else {
            break;
        };
        if let Some(Event::End(_)) = m.event.and_then(|e| e.event) {
            saw_end = true;
        }
    }
    assert!(saw_end);
    t.cleanup().await;
}

#[tokio::test]
async fn thirty_two_parallel_streams() {
    let t = target().await;
    let mut clients = Vec::new();
    for i in 0..32u32 {
        let transport = if i % 2 == 0 {
            TransportPreference::Native
        } else {
            TransportPreference::GrpcWeb
        };
        clients.push((i, t.client(transport).await));
    }
    // All 32 streams are in flight at once (polled concurrently).
    let runs = clients.iter().map(|(i, c)| async move {
        let (code, out) = run_sh(c, &format!("yes stream-{i} | head -c 1048576")).await;
        assert_eq!(code, Some(0));
        assert_eq!(out.len(), 1024 * 1024, "stream {i}");
        assert!(out.starts_with(format!("stream-{i}\n").as_bytes()));
    });
    tokio::time::timeout(
        Duration::from_secs(180),
        futures_util::future::join_all(runs),
    )
    .await
    .expect("32 parallel streams");
    t.cleanup().await;
}

// ---------------------------------------------------------------------------
// Filesystem
// ---------------------------------------------------------------------------

async fn read_sha(c: &cua_spacesd_client::SpacesdClient, path: &str) -> (u64, String) {
    let mut stream = c
        .filesystem()
        .read_file(ReadFileRequest {
            path: path.into(),
            compute_sha256: true,
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    let mut hasher = sha2::Sha256::new();
    let mut total = 0u64;
    for _ in 0..10_000_000 {
        let Some(m) = stream.message().await.unwrap() else {
            break;
        };
        match m.message {
            Some(ReadMessage::Chunk(chunk)) => {
                assert_eq!(chunk.offset, total);
                total += chunk.data.len() as u64;
                hasher.update(&chunk.data);
            }
            Some(ReadMessage::End(end)) => {
                let local = hex::encode(hasher.finalize());
                assert_eq!(end.sha256, local, "server and client digests agree");
                assert_eq!(end.bytes_read, total);
                return (total, local);
            }
            _ => {}
        }
    }
    panic!("ReadFile ended without ReadFileEnd");
}

/// Large transfer over the native client stream, the resumable unary chunk
/// protocol (with a forced resume) and signed URLs, verified by sha256.
#[tokio::test]
async fn large_transfers_via_chunks_and_signed_urls() {
    let t = target().await;
    let size = big_bytes();
    let chunk = 1024 * 1024usize;
    let expected = pattern_sha(7, size);

    // 1) Native WriteFile stream, then gRPC-Web ReadFile.
    let native = t.client(TransportPreference::Native).await;
    let web = t.client(TransportPreference::GrpcWeb).await;
    let path_a = t.path("big-a.bin");
    let header = WriteFileHeader {
        path: path_a.clone(),
        expected_size: size,
        expected_sha256: expected.clone(),
        ..Default::default()
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<WriteFileRequest>(4);
    let producer = tokio::spawn(async move {
        tx.send(WriteFileRequest {
            message: Some(write_file_request::Message::Header(header)),
        })
        .await
        .unwrap();
        let mut p = Pattern::new(7);
        let mut left = size;
        while left > 0 {
            let n = left.min(chunk as u64) as usize;
            let mut buf = vec![0u8; n];
            p.fill(&mut buf);
            tx.send(WriteFileRequest {
                message: Some(write_file_request::Message::Data(buf)),
            })
            .await
            .unwrap();
            left -= n as u64;
        }
    });
    let written = native
        .filesystem()
        .write_file(tokio_stream::wrappers::ReceiverStream::new(rx))
        .await
        .unwrap()
        .into_inner();
    producer.await.unwrap();
    assert_eq!(written.sha256, expected);
    assert_eq!(read_sha(&web, &path_a).await, (size, expected.clone()));

    // 2) gRPC-Web resumable upload with a forced "disconnect" halfway: the
    //    client re-begins with the same upload id and resumes.
    let path_b = t.path("big-b.bin");
    let upload_id = unique("upload");
    let begin = |id: String, path: String| BeginUploadRequest {
        header: Some(WriteFileHeader {
            path,
            expected_size: size,
            expected_sha256: expected.clone(),
            ..Default::default()
        }),
        upload_id: id,
        ttl: None,
    };
    let first = web
        .filesystem()
        .begin_upload(begin(upload_id.clone(), path_b.clone()))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(first.received_bytes, 0);
    let mut p = Pattern::new(7);
    let mut offset = 0u64;
    let mut resumed = false;
    let mut buf = vec![0u8; chunk];
    while offset < size {
        if !resumed && offset >= size / 2 {
            resumed = true;
            let again = t.client(TransportPreference::GrpcWeb).await;
            let r = again
                .filesystem()
                .begin_upload(begin(upload_id.clone(), path_b.clone()))
                .await
                .unwrap()
                .into_inner();
            assert_eq!(r.received_bytes, offset, "resume point");
            // A retried (duplicate) chunk is acknowledged, not re-written.
            if offset >= chunk as u64 {
                let mut q = Pattern::new(7);
                let mut skip = vec![0u8; (offset - chunk as u64) as usize];
                q.fill(&mut skip);
                let mut dup = vec![0u8; chunk];
                q.fill(&mut dup);
                let d = again
                    .filesystem()
                    .upload_chunk(UploadChunkRequest {
                        upload_id: upload_id.clone(),
                        offset: offset - chunk as u64,
                        data: dup,
                    })
                    .await
                    .unwrap()
                    .into_inner();
                assert!(d.duplicate);
            }
            // A chunk from the future is refused with the expected offset.
            let e = again
                .filesystem()
                .upload_chunk(UploadChunkRequest {
                    upload_id: upload_id.clone(),
                    offset: offset + 8,
                    data: vec![0; 8],
                })
                .await
                .unwrap_err();
            assert_eq!(reason(&e), ErrorReason::OffsetMismatch as i32);
        }
        let n = (size - offset).min(chunk as u64) as usize;
        p.fill(&mut buf[..n]);
        web.filesystem()
            .upload_chunk(UploadChunkRequest {
                upload_id: upload_id.clone(),
                offset,
                data: buf[..n].to_vec(),
            })
            .await
            .unwrap();
        offset += n as u64;
    }
    let committed = web
        .filesystem()
        .commit_upload(CommitUploadRequest {
            upload_id,
            sha256: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(committed.sha256, expected);

    // 3) Signed-URL GET of the uploaded file.
    let signed = web
        .filesystem()
        .create_signed_url(CreateSignedUrlRequest {
            path: path_b.clone(),
            method: SignedUrlMethod::Get as i32,
            ttl: Some(cua_proto::wkt::Duration {
                seconds: 300,
                nanos: 0,
            }),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    let http = http_client();
    let response = http
        .get(t.endpoint().http_url(&signed.url_path).parse().unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), http::StatusCode::OK);
    {
        use http_body_util::BodyExt;
        let mut body = response.into_body();
        let mut hasher = sha2::Sha256::new();
        let mut total = 0u64;
        while let Some(frame) = body.frame().await {
            if let Ok(data) = frame.unwrap().into_data() {
                total += data.len() as u64;
                assert!(total <= size);
                hasher.update(&data);
            }
        }
        assert_eq!(
            (total, hex::encode(hasher.finalize())),
            (size, expected.clone())
        );
    }
    // Range request.
    let ranged = http
        .request(
            http::Request::get(t.endpoint().http_url(&signed.url_path))
                .header("range", "bytes=8-15")
                .body(full(bytes::Bytes::new()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ranged.status(), http::StatusCode::PARTIAL_CONTENT);
    let mut expect_range = vec![0u8; 16];
    Pattern::new(7).fill(&mut expect_range);
    assert_eq!(body_bytes(ranged, 64).await, expect_range[8..16]);
    // Tampered signature.
    let tampered = signed.url_path.replace("big-b", "big-a");
    let refused = http
        .get(t.endpoint().http_url(&tampered).parse().unwrap())
        .await
        .unwrap();
    assert_eq!(refused.status(), http::StatusCode::FORBIDDEN);

    // 4) Signed-URL PUT, then native ReadFile.
    let path_c = t.path("big-c.bin");
    let put = native
        .filesystem()
        .create_signed_url(CreateSignedUrlRequest {
            path: path_c.clone(),
            method: SignedUrlMethod::Put as i32,
            ttl: Some(cua_proto::wkt::Duration {
                seconds: 300,
                nanos: 0,
            }),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    let body = {
        use http_body_util::BodyExt;
        let stream = futures_util::stream::unfold(
            (Pattern::new(7), size),
            move |(mut p, left)| async move {
                if left == 0 {
                    return None;
                }
                let n = left.min(chunk as u64) as usize;
                let mut buf = vec![0u8; n];
                p.fill(&mut buf);
                Some((
                    Ok::<_, std::io::Error>(hyper::body::Frame::data(bytes::Bytes::from(buf))),
                    (p, left - n as u64),
                ))
            },
        );
        BodyExt::boxed(http_body_util::StreamBody::new(stream))
    };
    let response = http
        .request(
            http::Request::put(t.endpoint().http_url(&put.url_path))
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), http::StatusCode::CREATED);
    assert_eq!(read_sha(&native, &path_c).await, (size, expected.clone()));

    // 5) A checksum mismatch discards the upload.
    let path_d = t.path("bad.bin");
    let b = web
        .filesystem()
        .begin_upload(BeginUploadRequest {
            header: Some(WriteFileHeader {
                path: path_d.clone(),
                expected_sha256: "0".repeat(64),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    web.filesystem()
        .upload_chunk(UploadChunkRequest {
            upload_id: b.upload_id.clone(),
            offset: 0,
            data: b"data".to_vec(),
        })
        .await
        .unwrap();
    let e = web
        .filesystem()
        .commit_upload(CommitUploadRequest {
            upload_id: b.upload_id,
            sha256: String::new(),
        })
        .await
        .unwrap_err();
    assert_eq!(reason(&e), ErrorReason::ChecksumMismatch as i32);
    let missing = web
        .filesystem()
        .stat(StatRequest {
            path: path_d,
            no_follow_symlinks: false,
        })
        .await
        .unwrap_err();
    assert_eq!(missing.code(), Code::NotFound);

    for p in [path_a, path_b, path_c] {
        native
            .filesystem()
            .remove(RemoveRequest {
                path: p,
                recursive: false,
                missing_ok: true,
            })
            .await
            .unwrap();
    }
    t.cleanup().await;
}

#[tokio::test]
async fn directory_operations() {
    let t = target().await;
    let c = t.client(TransportPreference::GrpcWeb).await;
    let fs = || c.filesystem();
    let dir = t.path("d/e");
    let made = fs()
        .make_dir(MakeDirRequest {
            path: dir.clone(),
            parents: true,
            mode: 0,
        })
        .await
        .unwrap()
        .into_inner();
    assert!(made.created);
    assert!(
        !fs()
            .make_dir(MakeDirRequest {
                path: dir.clone(),
                parents: true,
                mode: 0
            })
            .await
            .unwrap()
            .into_inner()
            .created
    );
    for name in ["b.txt", "a.txt", ".hidden"] {
        let (code, _) = run_sh(&c, &format!("printf x > '{dir}/{name}'")).await;
        assert_eq!(code, Some(0));
    }
    let listed = fs()
        .list_dir(ListDirRequest {
            path: t.path("d"),
            depth: 2,
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    let names: Vec<_> = listed.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        ["e", "a.txt", "b.txt"],
        "depth-first, sorted, hidden excluded"
    );
    let page = fs()
        .list_dir(ListDirRequest {
            path: t.path("d"),
            depth: 2,
            page_size: 2,
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(page.entries.len(), 2);
    let rest = fs()
        .list_dir(ListDirRequest {
            path: t.path("d"),
            depth: 2,
            page_size: 2,
            page_token: page.next_page_token,
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(rest.entries.len(), 1);
    assert!(rest.next_page_token.is_empty());
    let moved = format!("{dir}/c.txt");
    fs().r#move(MoveRequest {
        source: format!("{dir}/a.txt"),
        destination: moved.clone(),
        ..Default::default()
    })
    .await
    .unwrap();
    let exists = fs()
        .r#move(MoveRequest {
            source: format!("{dir}/b.txt"),
            destination: moved.clone(),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(exists.code(), Code::AlreadyExists);
    let not_empty = fs()
        .remove(RemoveRequest {
            path: t.path("d"),
            recursive: false,
            missing_ok: false,
        })
        .await
        .unwrap_err();
    assert_eq!(not_empty.code(), Code::FailedPrecondition);
    fs().remove(RemoveRequest {
        path: t.path("d"),
        recursive: true,
        missing_ok: false,
    })
    .await
    .unwrap();
    fs().remove(RemoveRequest {
        path: t.path("d"),
        recursive: true,
        missing_ok: true,
    })
    .await
    .unwrap();
    let gone = fs()
        .stat(StatRequest {
            path: t.path("d"),
            no_follow_symlinks: false,
        })
        .await
        .unwrap_err();
    assert_eq!(reason(&gone), ErrorReason::PathNotFound as i32);
    t.cleanup().await;
}

#[tokio::test]
async fn fs_watch_stream_and_polling_api() {
    let t = target().await;
    let c = t.client(TransportPreference::GrpcWeb).await;
    let dir = t.path("watched");
    c.filesystem()
        .make_dir(MakeDirRequest {
            path: dir.clone(),
            parents: true,
            mode: 0,
        })
        .await
        .unwrap();
    let mut stream = c
        .filesystem()
        .watch_dir(WatchDirRequest {
            path: dir.clone(),
            recursive: true,
            keepalive_interval: None,
        })
        .await
        .unwrap()
        .into_inner();
    let first = stream.message().await.unwrap().unwrap();
    assert!(matches!(first.message, Some(WatchMessage::Started(_))));
    let watcher = c
        .filesystem()
        .create_watcher(CreateWatcherRequest {
            path: dir.clone(),
            recursive: true,
        })
        .await
        .unwrap()
        .into_inner()
        .watcher_id;
    let file = format!("{dir}/f.txt");
    run_sh(
        &c,
        &format!("printf one > '{file}'; sleep 1; printf two >> '{file}'; sleep 1; rm '{file}'"),
    )
    .await;
    let mut kinds = BTreeSet::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline
        && !(kinds.contains(&(FsEventType::Remove as i32)) && kinds.len() >= 2)
    {
        let Ok(Ok(Some(m))) = tokio::time::timeout(Duration::from_secs(5), stream.message()).await
        else {
            break;
        };
        if let Some(WatchMessage::Event(e)) = m.message {
            if e.path.ends_with("f.txt") {
                kinds.insert(e.r#type);
            }
        }
    }
    assert!(
        kinds.contains(&(FsEventType::Remove as i32))
            || kinds.contains(&(FsEventType::Rename as i32)),
        "{kinds:?}"
    );
    assert!(
        kinds.contains(&(FsEventType::Create as i32))
            || kinds.contains(&(FsEventType::Write as i32)),
        "{kinds:?}"
    );
    let mut polled = Vec::new();
    for _ in 0..40 {
        let r = c
            .filesystem()
            .get_watcher_events(GetWatcherEventsRequest {
                watcher_id: watcher.clone(),
                max_events: 0,
            })
            .await
            .unwrap()
            .into_inner();
        polled.extend(r.events);
        if polled.iter().any(|e| {
            e.r#type == FsEventType::Remove as i32 || e.r#type == FsEventType::Rename as i32
        }) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(!polled.is_empty(), "polling watcher saw nothing");
    c.filesystem()
        .remove_watcher(RemoveWatcherRequest {
            watcher_id: watcher.clone(),
        })
        .await
        .unwrap();
    let e = c
        .filesystem()
        .get_watcher_events(GetWatcherEventsRequest {
            watcher_id: watcher,
            max_events: 0,
        })
        .await
        .unwrap_err();
    assert_eq!(reason(&e), ErrorReason::SessionNotFound as i32);
    t.cleanup().await;
}

// ---------------------------------------------------------------------------
// Driver / MCP
// ---------------------------------------------------------------------------

#[tokio::test]
async fn driver_passthrough_and_mcp() {
    let t = target().await;
    let c = t.client(TransportPreference::Native).await;
    let caps = c
        .system()
        .get_capabilities(GetCapabilitiesRequest {})
        .await
        .unwrap()
        .into_inner();
    let driver = caps.features.iter().find(|f| f.name == "driver").unwrap();
    if !driver.supported {
        let e = c
            .driver()
            .list_tools(ListToolsRequest {})
            .await
            .unwrap_err();
        assert_eq!(reason(&e), ErrorReason::FeatureUnsupported as i32);
        return;
    }
    let tools = c
        .driver()
        .list_tools(ListToolsRequest {})
        .await
        .unwrap()
        .into_inner()
        .tools;
    assert!(!tools.is_empty());
    let unknown = c
        .driver()
        .call_tool(CallToolRequest {
            name: "no_such_tool".into(),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(unknown.code(), Code::NotFound);
    if t.local.is_some() {
        let r = c
            .driver()
            .call_tool(CallToolRequest {
                name: "get_screen_size".into(),
                arguments_json: String::new(),
                timeout: None,
            })
            .await
            .unwrap()
            .into_inner();
        assert!(!r.is_error, "{r:?}");
        assert!(
            matches!(&r.content[0].content, Some(tool_content::Content::Text(s)) if s == "1280x800")
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&r.structured_json).unwrap()["width"],
            1280
        );
        // cua-driver's own authorization runs on this path: an unreviewed
        // tool is refused as a tool-level error.
        let refused = c
            .driver()
            .call_tool(CallToolRequest {
                name: "echo".into(),
                arguments_json: r#"{"text":"hi"}"#.into(),
                timeout: None,
            })
            .await
            .unwrap()
            .into_inner();
        assert!(
            refused.is_error && refused.structured_json.contains("permission_denied"),
            "{refused:?}"
        );
    }

    // Streamable-HTTP MCP on the same port.
    let http = http_client();
    let post = |body: serde_json::Value, token: Option<&str>, session: Option<String>| {
        let mut request = http::Request::post(t.endpoint().http_url("/mcp"))
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        if let Some(token) = token {
            request = request.header(
                cua_proto::metadata::ENV_AUTHORIZATION,
                format!("Bearer {token}"),
            );
        }
        if let Some(session) = session {
            request = request.header("mcp-session-id", session);
        }
        request
            .body(full(serde_json::to_vec(&body).unwrap()))
            .unwrap()
    };
    let denied = http
        .request(post(
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(denied.status(), http::StatusCode::UNAUTHORIZED);
    let init = http
        .request(post(
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"conformance","version":"1"}}}),
            Some(&t.token),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(init.status(), http::StatusCode::OK);
    let session = init
        .headers()
        .get("mcp-session-id")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let init: serde_json::Value = serde_json::from_slice(&body_bytes(init, 1 << 20).await).unwrap();
    assert!(init["result"]["serverInfo"].is_object(), "{init}");
    let note = http
        .request(post(
            serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            Some(&t.token),
            Some(session.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(note.status(), http::StatusCode::ACCEPTED);
    let list = http
        .request(post(
            serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
            Some(&t.token),
            Some(session),
        ))
        .await
        .unwrap();
    let list: serde_json::Value =
        serde_json::from_slice(&body_bytes(list, 16 << 20).await).unwrap();
    assert!(
        list["result"]["tools"]
            .as_array()
            .is_some_and(|a| !a.is_empty()),
        "{list}"
    );
    t.cleanup().await;
}

// ---------------------------------------------------------------------------
// Tunnel and hotspot
// ---------------------------------------------------------------------------

async fn free_guest_port(c: &cua_spacesd_client::SpacesdClient) -> u16 {
    let (_, out) = run_sh(
        c,
        &format!(
            "{PYTHON} -c 'import socket; s=socket.socket(); s.bind((\"127.0.0.1\",0)); print(s.getsockname()[1])'"
        ),
    )
    .await;
    String::from_utf8(out).unwrap().trim().parse().unwrap()
}

#[tokio::test]
async fn tunnel_forward_reaches_a_guest_http_server() {
    let t = target().await;
    let c = t.client(TransportPreference::GrpcWeb).await;
    let root = t.path("www");
    c.filesystem()
        .make_dir(MakeDirRequest {
            path: root.clone(),
            parents: true,
            mode: 0,
        })
        .await
        .unwrap();
    run_sh(&c, &format!("printf tunnel-ok > '{root}/index.txt'")).await;
    let port = free_guest_port(&c).await;
    let name = unique("http");
    let _server = c
        .process()
        .start_process(StartProcessRequest {
            config: Some(ProcessConfig {
                command: PYTHON.into(),
                args: vec![
                    "-m".into(),
                    "http.server".into(),
                    "--bind".into(),
                    "127.0.0.1".into(),
                    port.to_string(),
                ],
                cwd: root.clone(),
                ..Default::default()
            }),
            tag: name.clone(),
            kill_on_disconnect: true,
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    let forward = c
        .tunnel()
        .forward(ForwardRequest {
            port: port as u32,
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    let url = t.endpoint().ws_url(&forward.ws_path);
    let mut response = String::new();
    for _ in 0..50 {
        let Ok((mut ws, _)) = ws_connect(&url).await else {
            tokio::time::sleep(Duration::from_millis(200)).await;
            continue;
        };
        ws.send(tokio_tungstenite::tungstenite::Message::Binary(
            b"GET /index.txt HTTP/1.0\r\nHost: x\r\n\r\n".to_vec(),
        ))
        .await
        .unwrap();
        response.clear();
        for _ in 0..100 {
            match tokio::time::timeout(Duration::from_secs(5), ws.next()).await {
                Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Binary(b)))) => {
                    response.push_str(&String::from_utf8_lossy(&b))
                }
                _ => break,
            }
        }
        if response.contains("tunnel-ok") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(
        response.contains("200") && response.contains("tunnel-ok"),
        "{response:?}"
    );
    let list = c
        .tunnel()
        .list_forwards(ListForwardsRequest {})
        .await
        .unwrap()
        .into_inner();
    let info = list
        .forwards
        .iter()
        .find(|f| f.forward_id == forward.forward_id)
        .unwrap();
    assert!(info.bytes_out > 0);
    // Tickets are scope-bound: a tunnel ticket does not open the hotspot.
    let wrong = t
        .endpoint()
        .ws_url(&forward.ws_path.replace("/tunnel", "/hotspot"));
    assert!(ws_connect(&wrong).await.is_err());
    // Forwarding to non-local hosts is refused.
    let e = c
        .tunnel()
        .forward(ForwardRequest {
            port: 80,
            host: "8.8.8.8".into(),
            ttl: None,
        })
        .await
        .unwrap_err();
    assert_eq!(e.code(), Code::PermissionDenied);
    c.tunnel()
        .close_forward(CloseForwardRequest {
            forward_id: forward.forward_id,
        })
        .await
        .unwrap();
    assert!(ws_connect(&url).await.is_err(), "closed forward refuses");
    c.process()
        .signal_process(SignalProcessRequest {
            process: tag(&name),
            signal: Signal::Kill as i32,
            process_group: true,
        })
        .await
        .unwrap();
    t.cleanup().await;
}

#[tokio::test]
async fn hotspot_egress_goes_through_the_peer() {
    let t = target().await;
    let c = t.client(TransportPreference::Native).await;
    // Client-side HTTP server the guest can only reach through the peer
    // (in-process both are loopback; the oracle is the peer's dial log).
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client_port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        while let Ok((mut s, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf).await;
                let _ = s
                    .write_all(b"HTTP/1.0 200 OK\r\nContent-Length: 10\r\n\r\nhotspot-ok")
                    .await;
            });
        }
    });
    let socks_port = free_guest_port(&c).await;
    let hotspot = c
        .tunnel()
        .start_hotspot(StartHotspotRequest {
            socks_port: socks_port as u32,
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    // Unauthenticated / tampered tickets are refused.
    let bad = t.endpoint().ws_url("/hotspot?ticket=v1.bad.sig");
    assert!(ws_connect(&bad).await.is_err());
    let (ws, _) = ws_connect(t.endpoint().ws_url(&hotspot.ws_path))
        .await
        .unwrap();
    let (sink, source) = ws.split();
    let source = Box::pin(source.filter_map(|m| async move {
        match m {
            Ok(tokio_tungstenite::tungstenite::Message::Binary(b)) => Some(b),
            _ => None,
        }
    }));
    let sink = Box::pin(sink.with(|b: Vec<u8>| async move {
        Ok::<_, tokio_tungstenite::tungstenite::Error>(
            tokio_tungstenite::tungstenite::Message::Binary(b),
        )
    }));
    let dials = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = dials.clone();
    tokio::spawn(cua_spacesd_socks::peer::serve_egress(
        source,
        sink,
        move |host: String, port| {
            log.lock().unwrap().push(format!("{host}:{port}"));
            async move { tokio::net::TcpStream::connect((host.as_str(), port)).await }
        },
    ));
    let mut body = Vec::new();
    for _ in 0..20 {
        let (code, out) = run_sh(
            &c,
            &format!("curl -s --max-time 5 --socks5-hostname 127.0.0.1:{socks_port} http://127.0.0.1:{client_port}/"),
        )
        .await;
        body = out;
        if code == Some(0) && body == b"hotspot-ok" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert_eq!(body, b"hotspot-ok");
    assert!(
        dials
            .lock()
            .unwrap()
            .contains(&format!("127.0.0.1:{client_port}")),
        "egress went through the peer"
    );
    let status = c
        .tunnel()
        .get_hotspot_status(GetHotspotStatusRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(status.state, HotspotState::Active as i32);
    assert!(status.bytes_in >= 10);
    c.tunnel()
        .stop_hotspot(StopHotspotRequest {
            hotspot_id: hotspot.hotspot_id,
        })
        .await
        .unwrap();
    let status = c
        .tunnel()
        .get_hotspot_status(GetHotspotStatusRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(status.state, HotspotState::Stopped as i32);
    t.cleanup().await;
}

// ---------------------------------------------------------------------------
// Teleport
// ---------------------------------------------------------------------------

#[tokio::test]
async fn teleport_receive_files_honors_ignore_rules() {
    let t = target().await;
    let Some(local) = &t.local else { return };
    let c = t.client(TransportPreference::GrpcWeb).await;
    let files: Vec<(&str, &[u8])> = vec![
        ("proj/.gitignore", b"secret.env\n"),
        ("proj/src/main.rs", b"fn main() {}\n"),
        ("proj/secret.env", b"TOKEN=1\n"),
        ("proj/debug.log", b"noise\n"),
        ("proj/empty.txt", b""),
    ];
    let entries: Vec<TransferEntry> = files
        .iter()
        .map(|(p, d)| TransferEntry {
            relative_path: (*p).into(),
            size: d.len() as u64,
            sha256: hex::encode(sha2::Sha256::digest(d)),
            ..Default::default()
        })
        .collect();
    let id = unique("xfer");
    let begin = c
        .teleport()
        .begin_receive_files(BeginReceiveFilesRequest {
            transfer_id: id.clone(),
            entries,
            ignore_patterns: vec!["*.log".into()],
            honor_gitignore: true,
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(begin.ignored, vec!["proj/debug.log"]);
    for (index, entry) in begin.accepted.iter().enumerate() {
        let data = files
            .iter()
            .find(|(p, _)| *p == entry.relative_path)
            .unwrap()
            .1;
        if data.is_empty() {
            continue;
        }
        c.teleport()
            .receive_files_chunk(ReceiveFilesChunkRequest {
                transfer_id: id.clone(),
                index: index as u32,
                offset: 0,
                data: data.to_vec(),
            })
            .await
            .unwrap();
    }
    let done = c
        .teleport()
        .commit_receive_files(CommitReceiveFilesRequest { transfer_id: id })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(done.skipped, vec!["proj/secret.env"]);
    let placed: BTreeSet<_> = done.files.iter().map(|f| f.path.clone()).collect();
    assert!(placed.contains(
        &local
            .downloads
            .join("proj/src/main.rs")
            .display()
            .to_string()
    ));
    assert_eq!(
        std::fs::read(local.downloads.join("proj/src/main.rs")).unwrap(),
        b"fn main() {}\n"
    );
    assert!(!local.downloads.join("proj/secret.env").exists());
    assert!(!local.downloads.join("proj/debug.log").exists());
    // Resending the same tree renames on conflict.
    let id2 = unique("xfer");
    let data = b"fn main() {}\n";
    let begin = c
        .teleport()
        .begin_receive_files(BeginReceiveFilesRequest {
            transfer_id: id2.clone(),
            entries: vec![TransferEntry {
                relative_path: "proj/src/main.rs".into(),
                size: data.len() as u64,
                sha256: hex::encode(sha2::Sha256::digest(data)),
                ..Default::default()
            }],
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(begin.accepted.len(), 1);
    c.teleport()
        .receive_files_chunk(ReceiveFilesChunkRequest {
            transfer_id: id2.clone(),
            index: 0,
            offset: 0,
            data: data.to_vec(),
        })
        .await
        .unwrap();
    let done = c
        .teleport()
        .commit_receive_files(CommitReceiveFilesRequest { transfer_id: id2 })
        .await
        .unwrap()
        .into_inner();
    assert!(
        done.files[0].path.ends_with("main (1).rs"),
        "{:?}",
        done.files
    );
    // Path traversal is refused.
    let e = c
        .teleport()
        .begin_receive_files(BeginReceiveFilesRequest {
            transfer_id: unique("xfer"),
            entries: vec![TransferEntry {
                relative_path: "../escape".into(),
                size: 0,
                sha256: "0".repeat(64),
                ..Default::default()
            }],
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(e.code(), Code::InvalidArgument);
}

#[cfg(unix)]
#[tokio::test]
async fn teleport_receive_files_drops_special_mode_bits() {
    use std::os::unix::fs::PermissionsExt as _;
    let t = target().await;
    let Some(local) = &t.local else { return };
    let c = t.client(TransportPreference::GrpcWeb).await;
    let data = b"#!/bin/sh\n";
    let name = format!("{}/tool.sh", unique("modes"));
    let id = unique("xfer");
    let begin = c
        .teleport()
        .begin_receive_files(BeginReceiveFilesRequest {
            transfer_id: id.clone(),
            entries: vec![TransferEntry {
                relative_path: name.clone(),
                size: data.len() as u64,
                sha256: hex::encode(sha2::Sha256::digest(data)),
                mode: 0o6755,
                ..Default::default()
            }],
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(begin.accepted.len(), 1);
    c.teleport()
        .receive_files_chunk(ReceiveFilesChunkRequest {
            transfer_id: id.clone(),
            index: 0,
            offset: 0,
            data: data.to_vec(),
        })
        .await
        .unwrap();
    c.teleport()
        .commit_receive_files(CommitReceiveFilesRequest { transfer_id: id })
        .await
        .unwrap();
    let mode = std::fs::metadata(local.downloads.join(&name))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o7777, 0o755, "setuid/setgid bits are dropped");
}

#[tokio::test]
async fn teleport_import_session_in_chunks() {
    use cua_spacesd_teleport::bundle::BundleWriter;
    use cua_spacesd_teleport::TransferScope;
    let t = target().await;
    let Some(local) = &t.local else { return };
    // A Chrome bundle in the sender's canonical layout, written with the
    // shared contract crate (the driver never links the sender).
    let mut writer = BundleWriter::new(Vec::new(), "chrome", "Chrome", TransferScope::FullProfile);
    writer.add_bytes("tabs.json", 0o644, b"[]").unwrap();
    writer
        .add_bytes(
            ".config/google-chrome/Default/Cookies",
            0o600,
            b"cookie-bytes",
        )
        .unwrap();
    writer
        .add_bytes(".config/google-chrome/Default/Preferences", 0o600, b"{}")
        .unwrap();
    let bundle = writer.finish().unwrap();
    let sha = hex::encode(sha2::Sha256::digest(&bundle));
    let c = t.client(TransportPreference::GrpcWeb).await;
    let manifest = c
        .teleport()
        .get_manifest(GetManifestRequest {
            app: "chrome".into(),
            scope: 0,
        })
        .await
        .unwrap()
        .into_inner();
    assert!(manifest.supported && manifest.supported_apps.contains(&"chrome".to_owned()));
    let id = unique("import");
    let chunks: Vec<&[u8]> = bundle.chunks(4096).collect();
    let mut result = None;
    for (i, chunk) in chunks.iter().enumerate() {
        let last = i + 1 == chunks.len();
        let r = c
            .teleport()
            .import_session(ImportSessionRequest {
                import_id: id.clone(),
                app: "chrome".into(),
                offset: (i * 4096) as u64,
                data: chunk.to_vec(),
                commit: last,
                sha256: if last { sha.clone() } else { String::new() },
                options: Some(ImportOptions {
                    launch_after: true,
                    ..Default::default()
                }),
                ..Default::default()
            })
            .await
            .unwrap()
            .into_inner();
        if i == 0 && chunks.len() > 1 {
            // Retrying the first chunk is a duplicate.
            let dup = c
                .teleport()
                .import_session(ImportSessionRequest {
                    import_id: id.clone(),
                    app: "chrome".into(),
                    offset: 0,
                    data: chunk.to_vec(),
                    ..Default::default()
                })
                .await
                .unwrap()
                .into_inner();
            assert!(dup.duplicate);
        }
        result = r.result;
    }
    let result = result.expect("import result on the final chunk");
    assert!(result.launched, "launched through the fake host");
    assert_eq!(
        local
            .host
            .calls_of(cua_spacesd_teleport::EffectKind::AppLaunch)
            .len(),
        1
    );
    // Chrome's user-data dir on the receiving host (the importer remaps the
    // bundle's canonical Linux layout).
    let user_data = if cfg!(target_os = "macos") {
        "Library/Application Support/Google/Chrome"
    } else if cfg!(windows) {
        "AppData/Local/Google/Chrome/User Data"
    } else {
        ".config/google-chrome"
    };
    assert_eq!(
        std::fs::read(local.teleport_home.join(user_data).join("Default/Cookies")).unwrap(),
        b"cookie-bytes"
    );
}

// ---------------------------------------------------------------------------
// Scripted relay-kill lane (docker): CUA_ENV_TEST_PHASE=start|reattach
// ---------------------------------------------------------------------------

/// Phase `start` launches a tagged process that prints one line every 500 ms
/// for `CUA_ENV_TEST_LONG_SECS` and reads a few lines; the lane script then
/// kills and restarts the relay; phase `reattach` reconnects by tag, replays
/// the scrollback from offset 0 and checks every line arrived. Skipped
/// unless `CUA_ENV_TEST_PHASE` is set.
#[tokio::test]
async fn phased_relay_kill_reattach() {
    let Ok(phase) = std::env::var("CUA_ENV_TEST_PHASE") else {
        return;
    };
    let name = std::env::var("CUA_ENV_TEST_PHASE_TAG").expect("CUA_ENV_TEST_PHASE_TAG");
    let t = target().await;
    let lines = long_secs() * 2;
    match phase.as_str() {
        "start" => {
            let c = t.client(TransportPreference::GrpcWeb).await;
            let mut stream = c
                .process()
                .start_process(StartProcessRequest {
                    config: Some(sh(&format!(
                        "i=0; while [ $i -lt {lines} ]; do echo line $i; i=$((i+1)); sleep 0.5; done"
                    ))),
                    tag: name,
                    ..Default::default()
                })
                .await
                .unwrap()
                .into_inner();
            let mut seen = String::new();
            for _ in 0..1000 {
                let m = tokio::time::timeout(Duration::from_secs(10), stream.message())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                if let Some(Event::Data(d)) = m.event.and_then(|e| e.event) {
                    if let Some(Output::Stdout(b)) = d.output {
                        seen.push_str(&String::from_utf8_lossy(&b));
                    }
                }
                if seen.contains("line 2\n") {
                    return;
                }
            }
            panic!("no output before the relay kill");
        }
        "reattach" => {
            // The machine may still be rejoining.
            let mut client = None;
            for _ in 0..120 {
                let c = t.client(TransportPreference::Native).await;
                if c.system().health(HealthRequest {}).await.is_ok() {
                    client = Some(c);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            let c = client.expect("machine rejoined the relay");
            let mut stream = c
                .process()
                .connect_process(ConnectProcessRequest {
                    process: tag(&name),
                    replay_from_offset: Some(0),
                    ..Default::default()
                })
                .await
                .unwrap()
                .into_inner();
            let mut all = String::new();
            let mut exit = None;
            for _ in 0..100_000 {
                let Some(m) =
                    tokio::time::timeout(Duration::from_secs(lines + 60), stream.message())
                        .await
                        .unwrap()
                        .unwrap()
                else {
                    break;
                };
                match m.event.and_then(|e| e.event) {
                    Some(Event::Data(d)) => {
                        if let Some(Output::Stdout(b)) = d.output {
                            all.push_str(&String::from_utf8_lossy(&b));
                        }
                    }
                    Some(Event::End(e)) => {
                        exit = e.exit_code;
                        break;
                    }
                    _ => {}
                }
            }
            assert_eq!(exit, Some(0));
            for i in 0..lines {
                assert!(
                    all.contains(&format!("line {i}\n")),
                    "line {i} missing after reattach"
                );
            }
        }
        other => panic!("unknown phase {other}"),
    }
}

// ---------------------------------------------------------------------------
// Users, disk-full, shutdown
// ---------------------------------------------------------------------------

/// When the driver runs as root, `ProcessConfig.user` switches users and a
/// non-root driver refuses a different user.
#[tokio::test]
async fn process_user_switching() {
    let t = target().await;
    let c = t.client(TransportPreference::Native).await;
    let (_, uid) = run_sh(&c, "id -u").await;
    let root = String::from_utf8_lossy(&uid).trim() == "0";
    let started = c
        .process()
        .start_process(StartProcessRequest {
            config: Some(ProcessConfig {
                user: "nobody".into(),
                cwd: "/".into(),
                ..sh("id -un")
            }),
            ..Default::default()
        })
        .await;
    if cfg!(windows) {
        // Off Unix a process runs only as the driver's own user.
        let Err(error) = started else {
            panic!("switching users is refused off Unix");
        };
        assert_eq!(error.code(), Code::InvalidArgument);
        return;
    }
    if !root {
        let Err(error) = started else {
            panic!("non-root driver refuses to switch users");
        };
        assert_eq!(error.code(), Code::PermissionDenied);
        return;
    }
    let mut stream = started.unwrap().into_inner();
    let mut out = String::new();
    for _ in 0..100 {
        let Some(m) = stream.message().await.unwrap() else {
            break;
        };
        match m.event.and_then(|e| e.event) {
            Some(Event::Data(d)) => {
                if let Some(Output::Stdout(b)) = d.output {
                    out.push_str(&String::from_utf8_lossy(&b));
                }
            }
            Some(Event::End(_)) => break,
            _ => {}
        }
    }
    assert_eq!(out.trim(), "nobody");
}

/// A full disk answers 507 on `/files` and DISK_FULL on uploads. Needs a
/// tiny filesystem: `CUA_ENV_TEST_TINY_FS=/tiny` with `--tmpfs /tiny:size=1m`.
#[tokio::test]
async fn disk_full_is_507_and_disk_full_reason() {
    let Ok(tiny) = std::env::var("CUA_ENV_TEST_TINY_FS") else {
        return;
    };
    let t = target().await;
    let c = t.client(TransportPreference::GrpcWeb).await;
    let put = c
        .filesystem()
        .create_signed_url(CreateSignedUrlRequest {
            path: format!("{tiny}/big.bin"),
            method: SignedUrlMethod::Put as i32,
            ttl: Some(cua_proto::wkt::Duration {
                seconds: 60,
                nanos: 0,
            }),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    let response = http_client()
        .request(
            http::Request::put(t.endpoint().http_url(&put.url_path))
                .body(full(vec![7u8; 4 * 1024 * 1024]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), http::StatusCode::INSUFFICIENT_STORAGE);
    let begin = c
        .filesystem()
        .begin_upload(BeginUploadRequest {
            header: Some(WriteFileHeader {
                path: format!("{tiny}/up.bin"),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    let mut error = None;
    for i in 0..8u64 {
        match c
            .filesystem()
            .upload_chunk(UploadChunkRequest {
                upload_id: begin.upload_id.clone(),
                offset: i * 1024 * 1024,
                data: vec![1u8; 1024 * 1024],
            })
            .await
        {
            Ok(_) => {}
            Err(e) => {
                error = Some(e);
                break;
            }
        }
    }
    let error = match error {
        Some(e) => e,
        None => c
            .filesystem()
            .commit_upload(CommitUploadRequest {
                upload_id: begin.upload_id,
                sha256: String::new(),
            })
            .await
            .unwrap_err(),
    };
    assert_eq!(error.code(), Code::ResourceExhausted);
    assert_eq!(reason(&error), ErrorReason::DiskFull as i32);
}

#[tokio::test]
async fn shutdown_driver_exit_drains_and_stops() {
    let t = target().await;
    if t.local.is_none() {
        return; // never shut down a shared target
    }
    let c = t.client(TransportPreference::Native).await;
    let e = c
        .system()
        .shutdown(ShutdownRequest::default())
        .await
        .unwrap_err();
    assert_eq!(e.code(), Code::InvalidArgument, "UNSPECIFIED is rejected");
    let e = c
        .system()
        .shutdown(ShutdownRequest {
            mode: ShutdownMode::GuestPoweroff as i32,
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(
        e.code(),
        Code::FailedPrecondition,
        "guest power is off by default"
    );
    c.system()
        .shutdown(ShutdownRequest {
            mode: ShutdownMode::DriverExit as i32,
            ..Default::default()
        })
        .await
        .unwrap();
    let mut stopped = false;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let fresh = t.client(TransportPreference::GrpcWeb).await;
        if fresh.system().health(HealthRequest {}).await.is_err() {
            stopped = true;
            break;
        }
    }
    assert!(stopped, "driver stopped serving after DRIVER_EXIT");
}
