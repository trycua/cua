//! Wire round-trips of representative messages, including oneofs, maps,
//! enums, optional fields and well-known types.

use cua_proto::daemon::v1 as daemon;
use cua_proto::env::v1::*;
use cua_proto::wkt;
use prost::Message;

fn roundtrip<M: Message + Default + PartialEq + std::fmt::Debug>(msg: &M) -> Vec<u8> {
    let bytes = msg.encode_to_vec();
    let decoded = M::decode(bytes.as_slice()).expect("decodes");
    assert_eq!(&decoded, msg);
    bytes
}

fn ts(seconds: i64, nanos: i32) -> wkt::Timestamp {
    wkt::Timestamp { seconds, nanos }
}

#[test]
fn process_events_roundtrip() {
    let events = [
        ProcessEvent {
            event: Some(process_event::Event::Start(ProcessStart {
                pid: 4242,
                tag: "build".into(),
                started_at: Some(ts(1_767_225_600, 5)),
                scrollback_start_offset: 0,
                output_end_offset: 17,
            })),
        },
        ProcessEvent {
            event: Some(process_event::Event::Data(ProcessData {
                offset: 17,
                output: Some(process_data::Output::Stderr(
                    b"warning: \xff\x00 raw".to_vec(),
                )),
            })),
        },
        ProcessEvent {
            event: Some(process_event::Event::Keepalive(KeepAlive {})),
        },
        ProcessEvent {
            event: Some(process_event::Event::End(ProcessEnd {
                exit_code: Some(0),
                signal: Signal::Unspecified as i32,
                timed_out: false,
                error: String::new(),
                ended_at: Some(ts(1_767_225_700, 0)),
            })),
        },
    ];
    for event in &events {
        roundtrip(&StartProcessResponse {
            event: Some(event.clone()),
        });
        roundtrip(&ConnectProcessResponse {
            event: Some(event.clone()),
        });
    }
}

#[test]
fn optional_exit_code_distinguishes_zero_from_unset() {
    let killed = ProcessEnd {
        exit_code: None,
        signal: Signal::Kill as i32,
        ..Default::default()
    };
    let decoded = ProcessEnd::decode(killed.encode_to_vec().as_slice()).unwrap();
    assert_eq!(decoded.exit_code, None);
    assert_eq!(decoded.signal(), Signal::Kill);

    let ok = ProcessEnd {
        exit_code: Some(0),
        ..Default::default()
    };
    assert_eq!(
        ProcessEnd::decode(ok.encode_to_vec().as_slice())
            .unwrap()
            .exit_code,
        Some(0)
    );
}

#[test]
fn start_process_request_roundtrip() {
    let req = StartProcessRequest {
        config: Some(ProcessConfig {
            command: "/bin/bash".into(),
            args: vec!["-lc".into(), "sleep 600".into()],
            env: [("FOO".to_string(), "bar".to_string())].into(),
            cwd: "/tmp".into(),
            user: "cua".into(),
            timeout: Some(wkt::Duration {
                seconds: 900,
                nanos: 0,
            }),
        }),
        pty: Some(PtyConfig {
            size: Some(PtySize {
                cols: 120,
                rows: 40,
                ..Default::default()
            }),
            term: String::new(),
        }),
        tag: "long".into(),
        stdin: true,
        keepalive_interval: Some(wkt::Duration {
            seconds: 30,
            nanos: 0,
        }),
        scrollback_bytes: 1 << 20,
        kill_on_disconnect: false,
    };
    roundtrip(&req);
    roundtrip(&SendInputRequest {
        process: Some(ProcessSelector {
            selector: Some(process_selector::Selector::Tag("long".into())),
        }),
        input: Some(ProcessInput {
            input: Some(process_input::Input::Pty(b"\x03".to_vec())),
        }),
        sequence: 7,
        writer_id: "sdk-1".into(),
    });
}

#[test]
fn filesystem_messages_roundtrip() {
    roundtrip(&ReadFileResponse {
        message: Some(read_file_response::Message::Chunk(FileChunk {
            offset: 1 << 33,
            data: vec![0u8; 1024],
        })),
    });
    roundtrip(&WriteFileRequest {
        message: Some(write_file_request::Message::Header(WriteFileHeader {
            path: "~/big.bin".into(),
            mode: WriteMode::CreateNew as i32,
            permissions: 0o600,
            create_parents: true,
            expected_size: 2 << 30,
            expected_sha256: "ab".repeat(32),
        })),
    });
    roundtrip(&UploadChunkRequest {
        upload_id: "u1".into(),
        offset: 3 << 20,
        data: b"chunk".to_vec(),
    });
    roundtrip(&WatchDirResponse {
        message: Some(watch_dir_response::Message::Event(FsEvent {
            path: "/home/cua/a.txt".into(),
            r#type: FsEventType::Rename as i32,
            old_path: "/home/cua/b.txt".into(),
            observed_at: Some(ts(1, 2)),
        })),
    });
}

#[test]
fn computer_messages_roundtrip() {
    roundtrip(&ScreenshotRequest {
        source: Some(screenshot_request::Source::Window(WindowRef {
            id: "w-1".into(),
            epoch: 3,
        })),
        region: Some(Rect {
            x: 10.0,
            y: 20.0,
            width: 300.5,
            height: 200.25,
        }),
        format: ImageFormat::Webp as i32,
        quality: 70,
        max_dimension: 1280,
        include_cursor: true,
    });
    roundtrip(&ScreenshotResponse {
        image: vec![0x89, b'P', b'N', b'G'],
        format: ImageFormat::Png as i32,
        image_size: Some(PixelSize {
            width: 2048,
            height: 1362,
        }),
        native_size: Some(PixelSize {
            width: 2048,
            height: 1362,
        }),
        scale: 2.0,
        logical_bounds: Some(Rect {
            x: 0.0,
            y: 0.0,
            width: 1024.0,
            height: 681.0,
        }),
        screenshot_id: "s-9".into(),
        display_id: "primary".into(),
        captured_at: Some(ts(5, 0)),
    });
    roundtrip(&PointerRequest {
        target: Some(InputTarget {
            window: Some(WindowRef {
                id: "w-1".into(),
                epoch: 3,
            }),
            display_id: String::new(),
            delivery: Delivery::Background as i32,
            space: CoordinateSpace::Screenshot as i32,
            screenshot_id: "s-9".into(),
        }),
        action: Some(pointer_request::Action::Scroll(PointerScroll {
            position: Some(Point { x: 100.0, y: 200.0 }),
            delta_x: 0.0,
            delta_y: -3.0,
            unit: ScrollUnit::Line as i32,
        })),
    });
    roundtrip(&KeyboardRequest {
        target: None,
        action: Some(keyboard_request::Action::Hotkey(KeyboardHotkey {
            keys: vec![
                KeyInput {
                    key: Some(key_input::Key::Named(Key::Meta as i32)),
                },
                KeyInput {
                    key: Some(key_input::Key::Named(Key::ShiftLeft as i32)),
                },
                KeyInput {
                    key: Some(key_input::Key::Character("é".into())),
                },
            ],
        })),
    });
    roundtrip(&SetClipboardRequest {
        content: Some(ClipboardContent {
            text: Some(String::new()),
            file_paths: vec!["/tmp/a.png".into()],
            image_png: Some(vec![0x89, b'P', b'N', b'G']),
        }),
    });
}

#[test]
fn unknown_enum_values_survive_decoding() {
    // A newer peer may send enum values this build does not know; prost keeps
    // the raw i32 and the typed getter falls back to the default.
    let mut req = KeyboardPress::default();
    req.modifiers.push(9999);
    let decoded = KeyboardPress::decode(req.encode_to_vec().as_slice()).unwrap();
    assert_eq!(decoded.modifiers, vec![9999]);
    assert_eq!(Key::try_from(9999).ok(), None);
}

#[test]
fn stream_and_tunnel_messages_roundtrip() {
    roundtrip(&OpenMediaResponse {
        media_session_id: "m1".into(),
        ticket: "t".repeat(43),
        ticket_expires_at: Some(ts(100, 0)),
        ws_path: "/media?ticket=abc".into(),
        quic: Some(QuicEndpoint {
            port: 3212,
            certificate_sha256: "00".repeat(32),
            alpn: "rcdp/2".into(),
        }),
        codec: MediaCodec::H264 as i32,
        max_fps: 30,
        max_dimension: 1920,
        bitrate_kbps: 4000,
        policy: SessionPolicy::BackgroundOnly as i32,
        geometry_control: GeometryControl::ObserveOnly as i32,
        geometry: Some(MediaGeometry {
            frame_size: Some(PixelSize {
                width: 1920,
                height: 1080,
            }),
            scale: 1.0,
            logical_bounds: Some(Rect {
                x: 0.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0,
            }),
            geometry_epoch: 1,
        }),
        wire_version: cua_proto::MEDIA_WIRE_VERSION,
        audio: None,
    });
    roundtrip(&JoinResponse {
        event: Some(join_response::Event::CursorMoved(CursorMoved {
            participant_id: "p2".into(),
            cursor: Some(CursorPosition {
                display_id: "primary".into(),
                window: None,
                position: Some(Point { x: 0.25, y: 0.75 }),
                visible: true,
                shape: CursorShape::Text as i32,
                shape_source: CursorShapeSource::HitTest as i32,
            }),
            at: Some(ts(7, 0)),
        })),
    });
    roundtrip(&ForwardResponse {
        forward_id: "f1".into(),
        ticket: "tk".into(),
        ws_path: "/tunnel?ticket=tk".into(),
        expires_at: Some(ts(9, 0)),
    });
    roundtrip(&ImportSessionRequest {
        import_id: "i1".into(),
        app: "firefox".into(),
        scope: TeleportScope::Session as i32,
        offset: 0,
        data: vec![1, 2, 3],
        commit: true,
        sha256: "ff".repeat(32),
        options: Some(ImportOptions {
            replace_existing: true,
            close_running_app: true,
            launch_after: false,
            expires_at_ms: 1_700_000_000_000,
            broker_grant: String::new(),
        }),
    });
}

#[test]
fn capabilities_and_error_info_roundtrip() {
    roundtrip(&GetCapabilitiesResponse {
        version: "0.1.0".into(),
        protocol_version: cua_proto::ENV_PROTOCOL_VERSION,
        protocol_revision: cua_proto::ENV_PROTOCOL_REVISION,
        os: Some(OperatingSystem {
            family: OsFamily::Linux as i32,
            name: "Ubuntu".into(),
            version: "24.04".into(),
            kernel: "6.8.0".into(),
            pretty_name: "Ubuntu 24.04.3 LTS".into(),
        }),
        runtime: Runtime::Gvisor as i32,
        runtime_detail: "runsc".into(),
        arch: Architecture::Arm64 as i32,
        display_server: DisplayServer::X11 as i32,
        displays: vec![Display {
            id: "0".into(),
            name: "XVFB-0".into(),
            primary: true,
            bounds: Some(Rect {
                x: 0.0,
                y: 0.0,
                width: 1280.0,
                height: 800.0,
            }),
            native_size: Some(PixelSize {
                width: 1280,
                height: 800,
            }),
            scale_factor: 1.0,
            refresh_rate_hz: 0,
            rotation_degrees: 0,
        }],
        features: vec![
            Feature {
                name: "pty".into(),
                supported: true,
                ..Default::default()
            },
            Feature {
                name: "h264_hw".into(),
                supported: false,
                limitation: "no /dev/dri under gVisor".into(),
                attributes: [("fallback".to_string(), "openh264".to_string())].into(),
            },
        ],
        hostname: "sbx".into(),
        initialized: true,
        boot_time: Some(ts(0, 0)),
        side_channels: Some(SideChannels {
            media_ws_path: cua_proto::metadata::MEDIA_WS_PATH.into(),
            media_quic_port: cua_proto::SPACESD_DEFAULT_QUIC_PORT as u32,
            tunnel_ws_path: cua_proto::metadata::TUNNEL_WS_PATH.into(),
            files_http_path: cua_proto::metadata::FILES_PATH.into(),
            mcp_http_path: cua_proto::metadata::MCP_PATH.into(),
        }),
        limits: Some(Limits {
            max_chunk_bytes: 4 << 20,
            max_message_bytes: 8 << 20,
            preferred_chunk_bytes: 1 << 20,
            default_scrollback_bytes: 1 << 20,
        }),
        machine_seal_public_key: vec![7; 32],
    });
    roundtrip(&ErrorInfo {
        reason: ErrorReason::OffsetMismatch as i32,
        message: "expected offset 1048576".into(),
        feature: String::new(),
        metadata: [("expected_offset".to_string(), "1048576".to_string())].into(),
    });
}

#[test]
fn daemon_messages_roundtrip() {
    roundtrip(&daemon::CreateSandboxResponse {
        sandbox: Some(daemon::Sandbox {
            name: "dev".into(),
            provider: daemon::SandboxProvider::Local as i32,
            state: daemon::SandboxState::Running as i32,
            image: "ghcr.io/trycua/linux:24.04".into(),
            endpoints: [("env".to_string(), "http://127.0.0.1:3211".to_string())].into(),
            labels: Default::default(),
            created_at: Some(ts(1, 0)),
            error: String::new(),
            runtime_type: "docker".into(),
            ephemeral: false,
            services: [("env".to_string(), 3211u32)].into(),
            location: "local".into(),
            expires_at: Some(ts(901, 0)),
            provider_details: [("backend".to_string(), "container".to_string())].into(),
            id: "local:dev".into(),
            kind: "container".into(),
            runtime: "gvisor".into(),
        }),
    });
    roundtrip(&daemon::CreateSpaceRequest {
        image: "ghcr.io/trycua/linux:24.04".into(),
        location: "cloud".into(),
        kind: "vm".into(),
        runtime: "kubevirt".into(),
        name: "dev".into(),
        wait: Some(false),
        reuse: true,
        ..Default::default()
    });
    roundtrip(&daemon::CreatePublicUrlResponse {
        public_url: Some(daemon::PublicUrl {
            id: "share-1".into(),
            url: "http://127.0.0.1:5/s/tok/".into(),
            expires_at: Some(ts(3600, 0)),
            sandbox: "dev".into(),
            service: "mcp".into(),
            provider_details: Default::default(),
        }),
    });
    roundtrip(&daemon::GetInfoResponse {
        version: "0.1.0".into(),
        pid: 42,
        socket_path: "/home/u/.cua/cua.sock".into(),
        loopback_url: "http://127.0.0.1:52011".into(),
        loopback_token: "t".into(),
        features: vec!["env".into(), "fleet".into()],
        started_at: Some(ts(1, 0)),
        executable: "/Applications/Cua Spaces.app/Contents/MacOS/cua".into(),
        build_id: "0.2.0-7a1c-18f2".into(),
    });
}

#[test]
fn wkt_types_are_wire_compatible_with_prost_types() {
    let ours = wkt::Timestamp {
        seconds: 1_767_225_600,
        nanos: 123,
    };
    let theirs = prost_types::Timestamp {
        seconds: 1_767_225_600,
        nanos: 123,
    };
    assert_eq!(ours.encode_to_vec(), theirs.encode_to_vec());
}

#[cfg(feature = "serde")]
mod json {
    use super::*;

    #[test]
    fn proto3_json_uses_camel_case_and_enum_names() {
        let req = PointerRequest {
            target: Some(InputTarget {
                delivery: Delivery::Foreground as i32,
                space: CoordinateSpace::Screen as i32,
                ..Default::default()
            }),
            action: Some(pointer_request::Action::Click(PointerClick {
                position: Some(Point { x: 1.0, y: 2.0 }),
                button: MouseButton::Right as i32,
                count: 2,
                modifiers: vec![Key::ControlLeft as i32],
            })),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["target"]["delivery"], "DELIVERY_FOREGROUND");
        assert_eq!(json["click"]["button"], "MOUSE_BUTTON_RIGHT");
        assert_eq!(json["click"]["modifiers"][0], "KEY_CONTROL_LEFT");
        let back: PointerRequest = serde_json::from_value(json).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn json_int64_bytes_and_timestamps_follow_proto3_json() {
        let chunk = FileChunk {
            offset: 5,
            data: b"hi".to_vec(),
        };
        let json = serde_json::to_value(&chunk).unwrap();
        assert_eq!(json["offset"], "5"); // 64-bit ints are strings in proto3 JSON
        assert_eq!(json["data"], "aGk=");
        let start = ProcessStart {
            started_at: Some(ts(0, 5)),
            ..Default::default()
        };
        let json = serde_json::to_value(&start).unwrap();
        assert!(
            json["startedAt"]
                .as_str()
                .unwrap()
                .starts_with("1970-01-01T00:00:00.000000005")
        );
        let back: ProcessStart = serde_json::from_value(json).unwrap();
        assert_eq!(back, start);
    }
}

#[test]
fn audio_media_messages_roundtrip() {
    let opus = AudioEncoding {
        codecs: vec![AudioCodec::Opus as i32, AudioCodec::PcmS16le as i32],
        sample_rate_hz: 48_000,
        channels: 2,
        bitrate_kbps: 64,
        fec: Some(true),
        dtx: Some(false),
        frame_ms: 10,
        expected_loss_percent: 10,
    };
    let req = OpenMediaRequest {
        target: Some(MediaTarget {
            target: Some(media_target::Target::Window(WindowRef {
                id: "w-1".into(),
                epoch: 1,
            })),
        }),
        codecs: vec![MediaCodec::H264 as i32],
        audio: Some(AudioOptions {
            enabled: true,
            source_ids: vec!["desktop".into(), "app-firefox".into()],
            encoding: Some(opus.clone()),
            uplink: Some(AudioUplinkOptions {
                enabled: true,
                encoding: Some(AudioEncoding {
                    channels: 1,
                    ..Default::default()
                }),
                virtual_source_name: String::new(),
                set_default_input: true,
            }),
        }),
        disable_video: false,
        ..Default::default()
    };
    roundtrip(&req);
    // Explicit `false` survives the wire, distinct from unset (= default on).
    let decoded = OpenMediaRequest::decode(req.encode_to_vec().as_slice()).unwrap();
    let enc = decoded.audio.unwrap().encoding.unwrap();
    assert_eq!(enc.dtx, Some(false));
    assert_eq!(enc.fec, Some(true));

    let negotiated = NegotiatedAudioEncoding {
        codec: AudioCodec::Opus as i32,
        sample_rate_hz: 48_000,
        channels: 2,
        bitrate_kbps: 64,
        fec: true,
        dtx: true,
        frame_ms: 20,
    };
    roundtrip(&OpenMediaResponse {
        media_session_id: "m2".into(),
        audio: Some(NegotiatedAudio {
            tracks: vec![AudioTrack {
                track_id: 1,
                source: Some(AudioSource {
                    source_id: "app-firefox".into(),
                    kind: AudioSourceKind::Application as i32,
                    name: "Firefox".into(),
                    app: Some(AppInfo {
                        name: "Firefox".into(),
                        app_id: "firefox.desktop".into(),
                        pid: 77,
                    }),
                    windows: vec![WindowRef {
                        id: "w-1".into(),
                        epoch: 1,
                    }],
                    available: true,
                    limitation: "per-app capture unsupported; desktop mix".into(),
                    desktop_fallback: true,
                }),
            }],
            encoding: Some(negotiated),
            uplink: Some(AudioUplinkGrant {
                granted: false,
                denied_reason: "principal not allowed".into(),
                track_id: 0,
                encoding: None,
                virtual_source_name: String::new(),
            }),
        }),
        wire_version: cua_proto::MEDIA_WIRE_VERSION,
        ..Default::default()
    });
    roundtrip(&ListTargetsResponse {
        targets: vec![StreamTarget {
            target: Some(stream_target::Target::Display(Display {
                id: "primary".into(),
                ..Default::default()
            })),
            available: true,
            limitation: String::new(),
            audio: Some(AudioSource {
                source_id: "desktop".into(),
                kind: AudioSourceKind::Desktop as i32,
                available: true,
                ..Default::default()
            }),
        }],
        codecs: vec![MediaCodec::H264 as i32],
        audio_sources: vec![],
        audio_codecs: vec![AudioCodec::Opus as i32],
        audio_uplink_allowed: false,
        audio_uplink_limitation: "AUDIO_UPLINK_MODE_DISABLED".into(),
    });
    roundtrip(&SetPreferencesRequest {
        media_session_id: "m2".into(),
        audio_enabled: Some(false),
        audio_bitrate_kbps: 24,
        audio_fec: Some(true),
        audio_dtx: None,
        audio_frame_ms: 40,
        audio_expected_loss_percent: Some(0),
        audio_uplink_muted: Some(true),
        ..Default::default()
    });
    roundtrip(&SetPreferencesResponse {
        max_fps: 30,
        max_dimension: 0,
        bitrate_kbps: 0,
        audio_enabled: false,
        audio_encoding: Some(negotiated),
        audio_uplink_muted: true,
    });
    roundtrip(&InitRequest {
        audio_uplink: Some(AudioUplinkAccess {
            mode: AudioUplinkMode::Allowlist as i32,
            principal_ids: vec!["user-1".into()],
        }),
        ..Default::default()
    });
}

#[test]
fn audio_packet_magic_cannot_collide_with_length_prefixed_packets() {
    // Length-prefixed packets start with a big-endian u32 header length of
    // at most 1 MiB, so their first byte is always 0.
    let max_header_len: u32 = 1 << 20;
    assert_eq!(max_header_len.to_be_bytes()[0], 0);
    assert_ne!(cua_proto::AUDIO_PACKET_MAGIC[0], 0);
}
