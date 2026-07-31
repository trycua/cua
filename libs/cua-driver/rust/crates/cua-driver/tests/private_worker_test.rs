use cua_driver_sdk::{
    ConfiguredDriverOptions, DriverExecutionMode, EmbeddedDriverHostOptions,
    EmbeddedEnvironmentVariable, EmbeddedPermissionMode, PrivateWorkerOptions,
    RuntimeAuthorizationOptions, SessionPermissionMode, TrustedResourceDirectory,
    TrustedSessionOptions, TrustedSessionResources,
};

fn worker_options() -> PrivateWorkerOptions {
    PrivateWorkerOptions {
        binary_path: env!("CARGO_BIN_EXE_cua-driver").to_owned(),
        host_bundle_id: "com.trycua.private-worker-test".into(),
        startup_timeout_ms: Some(10_000),
        shutdown_timeout_ms: Some(2_000),
        configured_driver: ConfiguredDriverOptions {
            claude_code_compatibility: false,
            authorization: RuntimeAuthorizationOptions {
                allowed_modes: vec![
                    SessionPermissionMode::Standard,
                    SessionPermissionMode::Unrestricted,
                ],
                compatibility_mode: SessionPermissionMode::Standard,
                compatibility_bounded_manifest_path: None,
                unrestricted_acknowledged: true,
                max_session_ttl_seconds: 60,
                max_idle_ttl_seconds: 30,
            },
        },
        environment: Vec::new(),
        inherit_stderr: true,
    }
}

#[tokio::test]
async fn private_worker_owns_one_runtime_without_a_reconnect_endpoint() {
    let driver = cua_driver_sdk::CuaDriver::create_private_worker(worker_options()).unwrap();
    assert_eq!(driver.execution_mode(), DriverExecutionMode::PrivateWorker);
    assert!(driver.socket_path().is_empty());
    assert!(driver.is_available());

    let metadata = driver.metadata().await.unwrap();
    assert_ne!(metadata.pid, std::process::id());
    let tools: serde_json::Value =
        serde_json::from_str(&driver.list_tools_json().await.unwrap()).unwrap();
    assert!(tools["tools"]
        .as_array()
        .is_some_and(|tools| !tools.is_empty()));

    let managed_root = tempfile::tempdir().unwrap();
    let recording_root = managed_root.path().join("recordings");
    let download_root = managed_root.path().join("downloads");
    std::fs::create_dir_all(&recording_root).unwrap();
    std::fs::create_dir_all(&download_root).unwrap();
    let session = driver
        .create_trusted_session_with_resources(
            TrustedSessionOptions {
                public_session: "worker-trusted".into(),
                mode: SessionPermissionMode::Standard,
                ttl_seconds: 60,
                idle_ttl_seconds: 30,
                bounded_manifest_path: None,
            },
            TrustedSessionResources {
                recording_root: Some(recording_root.to_string_lossy().into_owned()),
                browser_download_root: Some(download_root.to_string_lossy().into_owned()),
                upload_files: Vec::new(),
                replay_directories: Vec::new(),
                ffmpeg_path: None,
                browser_existing_profile_approved: false,
            },
        )
        .unwrap();
    let healthy = session
        .call_tool("health_report".into(), "{}".into())
        .await
        .unwrap();
    assert_ne!(healthy.error_code.as_deref(), Some("permission_denied"));
    let health: serde_json::Value =
        serde_json::from_str(healthy.structured_json.as_deref().unwrap()).unwrap();
    assert_eq!(health["trusted_resources"]["managed"], true);
    assert_eq!(health["trusted_resources"]["recording_root_ready"], true);
    assert!(
        !healthy
            .raw_json
            .contains(&recording_root.to_string_lossy().as_ref()),
        "managed status must not disclose native paths"
    );
    for check in health["checks"].as_array().into_iter().flatten() {
        if let Some(executable_path) = check.pointer("/data/executable_path") {
            assert_eq!(executable_path, "[host-managed]");
        }
    }

    let started = session
        .call_tool(
            "start_recording".into(),
            serde_json::json!({
                "output_dir": managed_root.path().join("model-selected"),
                "record_video": false
            })
            .to_string(),
        )
        .await
        .unwrap();
    assert!(!started.is_error, "{}", started.text);
    assert_eq!(
        started.text,
        "Recording started in the host-managed session root."
    );
    assert!(!managed_root.path().join("model-selected").exists());
    assert!(recording_root.join("session.json").exists());
    let stopped = session
        .call_tool("stop_recording".into(), "{}".into())
        .await
        .unwrap();
    assert!(!stopped.is_error, "{}", stopped.text);

    let substituted = session
        .call_tool(
            "health_report".into(),
            serde_json::json!({"session": "other"}).to_string(),
        )
        .await
        .unwrap();
    assert_eq!(substituted.error_code.as_deref(), Some("permission_denied"));

    session.close();

    let replay_root = managed_root.path().join("replay-artifact");
    let replay_recording_root = managed_root.path().join("replay-recordings");
    std::fs::create_dir_all(&replay_recording_root).unwrap();
    let model_selected_replay_root = managed_root.path().join("model-selected-replay");
    let replay_turn = replay_root.join("turn-00001");
    std::fs::create_dir_all(&replay_turn).unwrap();
    std::fs::write(
        replay_turn.join("action.json"),
        serde_json::json!({
            "tool": "start_recording",
            "arguments": {
                "output_dir": model_selected_replay_root,
                "record_video": false
            }
        })
        .to_string(),
    )
    .unwrap();
    let replay_session = driver
        .create_trusted_session_with_resources(
            TrustedSessionOptions {
                public_session: "worker-replay".into(),
                mode: SessionPermissionMode::Unrestricted,
                ttl_seconds: 60,
                idle_ttl_seconds: 30,
                bounded_manifest_path: None,
            },
            TrustedSessionResources {
                recording_root: Some(replay_recording_root.to_string_lossy().into_owned()),
                browser_download_root: Some(download_root.to_string_lossy().into_owned()),
                upload_files: Vec::new(),
                replay_directories: vec![TrustedResourceDirectory {
                    resource_id: "fixture-replay".into(),
                    path: replay_root.to_string_lossy().into_owned(),
                }],
                ffmpeg_path: None,
                browser_existing_profile_approved: false,
            },
        )
        .unwrap();
    let replayed = replay_session
        .call_tool(
            "replay_trajectory".into(),
            serde_json::json!({
                "dir": "fixture-replay",
                "delay_ms": 0,
                "stop_on_error": true
            })
            .to_string(),
        )
        .await
        .unwrap();
    let replayed_structured: serde_json::Value =
        serde_json::from_str(replayed.structured_json.as_deref().unwrap()).unwrap();
    assert_eq!(replayed_structured["attempted"], 1);
    assert_eq!(replayed_structured["succeeded"], 1);
    assert_eq!(replayed_structured["failed"], 0);
    assert_eq!(
        replayed_structured["turns"][0]["result_summary"],
        "[host-managed result]"
    );
    assert!(
        !replayed
            .raw_json
            .contains(replay_root.to_string_lossy().as_ref()),
        "managed replay must not disclose the native artifact path"
    );
    assert!(
        !model_selected_replay_root.exists(),
        "nested replay dispatch accepted a model-selected native path"
    );
    assert!(replay_recording_root.join("session.json").exists());
    let stopped = replay_session
        .call_tool("stop_recording".into(), "{}".into())
        .await
        .unwrap();
    assert!(!stopped.is_error, "{}", stopped.text);
    replay_session.close();

    driver.shutdown().await.unwrap();
    assert!(!driver.is_available());
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn private_worker_owns_the_macos_cursor_overlay_facility() {
    let driver = cua_driver_sdk::CuaDriver::create_private_worker(worker_options()).unwrap();
    let result = driver
        .call_tool(
            "get_agent_cursor_state".into(),
            serde_json::json!({"session": "worker-overlay"}).to_string(),
        )
        .await
        .unwrap();
    assert_ne!(
        result.error_code.as_deref(),
        Some("facility_unavailable"),
        "private worker did not install its AppKit main-thread adapter"
    );
    let permissions = driver
        .call_tool(
            "check_permissions".into(),
            serde_json::json!({"prompt": true}).to_string(),
        )
        .await
        .unwrap();
    let structured: serde_json::Value =
        serde_json::from_str(permissions.structured_json.as_deref().unwrap()).unwrap();
    assert_eq!(structured["direct_capture_status"], "not_checked");
    assert_eq!(structured["source"]["attribution"], "host");
    assert_eq!(
        structured["source"]["host_bundle_id"],
        "com.trycua.private-worker-test"
    );
    driver.shutdown().await.unwrap();
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn private_worker_inherits_the_interactive_linux_display_scope() {
    if std::env::var("CUA_REQUIRE_GUI").as_deref() != Ok("1") {
        return;
    }
    let has_x11 = std::env::var("DISPLAY").is_ok_and(|display| !display.is_empty());
    let has_wayland = std::env::var("WAYLAND_DISPLAY").is_ok_and(|display| !display.is_empty());
    assert!(
        has_x11 || has_wayland,
        "canonical GUI E2E requires DISPLAY or WAYLAND_DISPLAY"
    );

    let driver = cua_driver_sdk::CuaDriver::create_private_worker(worker_options()).unwrap();
    let standard = driver
        .create_trusted_session(TrustedSessionOptions {
            public_session: "worker-standard-display-scope".into(),
            mode: SessionPermissionMode::Standard,
            ttl_seconds: 60,
            idle_ttl_seconds: 30,
            bounded_manifest_path: None,
        })
        .unwrap();
    let observed = standard
        .call_tool("list_windows".into(), "{}".into())
        .await
        .unwrap();
    assert!(
        !observed.is_error,
        "routine standard-mode observation must not require an authorization host: {}",
        observed.text
    );
    standard.close();

    let session = driver
        .create_trusted_session(TrustedSessionOptions {
            public_session: "worker-display-scope".into(),
            mode: SessionPermissionMode::Unrestricted,
            ttl_seconds: 60,
            idle_ttl_seconds: 30,
            bounded_manifest_path: None,
        })
        .unwrap();
    let started = session
        .call_tool(
            "start_session".into(),
            serde_json::json!({
                "session": "worker-display-scope",
                "capture_scope": "desktop"
            })
            .to_string(),
        )
        .await
        .unwrap();
    assert!(
        !started.is_error,
        "worker could not declare the trusted desktop capture scope: {}",
        started.text
    );
    let started_structured: serde_json::Value = serde_json::from_str(
        started
            .structured_json
            .as_deref()
            .expect("start_session omitted structuredContent"),
    )
    .unwrap();
    assert_eq!(started_structured["capture_scope"], "desktop");
    assert_eq!(started_structured["effective_scope"], "desktop");
    if has_x11 {
        let desktop = session
            .call_tool("get_desktop_state".into(), "{}".into())
            .await
            .unwrap();
        assert!(
            !desktop.images.is_empty(),
            "worker inherited no usable X11 capture scope: {}",
            desktop.text
        );
    } else {
        let windows = session
            .call_tool("list_windows".into(), "{}".into())
            .await
            .unwrap();
        assert!(
            windows.error_code.is_none(),
            "worker inherited no usable native Wayland session scope: {}",
            windows.text
        );
    }
    let ended = session
        .call_tool(
            "end_session".into(),
            serde_json::json!({"session": "worker-display-scope"}).to_string(),
        )
        .await
        .unwrap();
    assert!(
        !ended.is_error,
        "worker could not end session: {}",
        ended.text
    );
    session.close();
    driver.shutdown().await.unwrap();
}

#[tokio::test]
async fn dropping_the_host_closes_and_terminates_the_private_worker() {
    let driver = cua_driver_sdk::CuaDriver::create_private_worker(worker_options()).unwrap();
    let pid = driver.metadata().await.unwrap().pid;
    drop(driver);

    #[cfg(unix)]
    {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while unsafe { libc::kill(pid as i32, 0) } == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_ne!(
            unsafe { libc::kill(pid as i32, 0) },
            0,
            "private worker must not outlive its owning SDK object"
        );
    }
}

#[tokio::test]
async fn embedded_service_binds_authority_to_the_original_host_connection() {
    let host = cua_driver_sdk::EmbeddedCuaDriverHost::with_options(EmbeddedDriverHostOptions {
        binary_path: env!("CARGO_BIN_EXE_cua-driver").to_owned(),
        host_bundle_id: "com.trycua.trusted-service-test".into(),
        socket_path: None,
        startup_timeout_ms: Some(10_000),
        shutdown_timeout_ms: Some(2_000),
        permission_mode: Some(EmbeddedPermissionMode::Standard),
        session_policy_path: None,
        approve_session_policy: false,
        dangerously_bypass_approvals: false,
        environment: Vec::<EmbeddedEnvironmentVariable>::new(),
        inherit_stderr: true,
    })
    .unwrap();
    let connection = host.clone().start().await.unwrap();
    let driver = cua_driver_sdk::CuaDriver::connect(Some(connection.socket_path));
    let driver = driver.unwrap();
    let managed_root = tempfile::tempdir().unwrap();
    let recording_root = managed_root.path().join("recordings");
    let download_root = managed_root.path().join("downloads");
    std::fs::create_dir_all(&recording_root).unwrap();
    std::fs::create_dir_all(&download_root).unwrap();
    let session = driver
        .create_trusted_session_with_resources(
            TrustedSessionOptions {
                public_session: "service-trusted".into(),
                mode: SessionPermissionMode::Standard,
                ttl_seconds: 60,
                idle_ttl_seconds: 30,
                bounded_manifest_path: None,
            },
            TrustedSessionResources {
                recording_root: Some(recording_root.to_string_lossy().into_owned()),
                browser_download_root: Some(download_root.to_string_lossy().into_owned()),
                upload_files: Vec::new(),
                replay_directories: Vec::new(),
                ffmpeg_path: None,
                browser_existing_profile_approved: false,
            },
        )
        .unwrap();
    let result = session
        .call_tool("health_report".into(), "{}".into())
        .await
        .unwrap();
    assert_ne!(result.error_code.as_deref(), Some("permission_denied"));
    let health: serde_json::Value =
        serde_json::from_str(result.structured_json.as_deref().unwrap()).unwrap();
    assert_eq!(health["trusted_resources"]["managed"], true);

    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "service_untrusted_child_probe",
            "--nocapture",
        ])
        .env(
            "CUA_DRIVER_TEST_TRUSTED_SERVICE_SOCKET",
            driver.socket_path(),
        )
        .status()
        .unwrap();
    assert!(
        child.success(),
        "a non-parent same-user process must be refused before delegated authority is created"
    );

    session.close();
    host.stop().await.unwrap();
}

#[test]
#[ignore = "subprocess-only trusted-service authentication probe"]
fn service_untrusted_child_probe() {
    let socket = std::env::var("CUA_DRIVER_TEST_TRUSTED_SERVICE_SOCKET").unwrap();
    let driver = cua_driver_sdk::CuaDriver::connect(Some(socket)).unwrap();
    let result = driver.create_trusted_session(TrustedSessionOptions {
        public_session: "forged-service-session".into(),
        mode: SessionPermissionMode::Standard,
        ttl_seconds: 60,
        idle_ttl_seconds: 30,
        bounded_manifest_path: None,
    });
    assert!(
        result.is_err(),
        "same-user reachability without embedded-host process identity is not authority"
    );
}
