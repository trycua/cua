#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let request = args.iter().any(|arg| arg == "--request-accessibility");
    let evidence = args
        .windows(2)
        .find(|pair| pair[0] == "--evidence")
        .map(|pair| &pair[1]);
    let mut record = serde_json::json!({
        "schema": "cua-driver/embedded-sdk-window@1",
        "source_sha": std::env::var("CUA_E2E_SOURCE_SHA").unwrap_or_default(),
        "process": native::process_status(),
        "status": "preflight",
    });
    if args.iter().any(|arg| arg == "--status-only") {
        let path = evidence.expect("--status-only requires --evidence");
        std::fs::write(path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
        if record["process"]["ax_trusted"] != true {
            eprintln!("The signed fixture process lacks existing Accessibility trust; no permission request was made.");
            std::process::exit(2);
        }
        return;
    }
    if !request && !args.iter().any(|arg| arg == "--run-gui") {
        println!("embedded_menu_restore: skipped; pass --run-gui in a TCC-authorized Aqua session");
        return;
    }
    let report = args
        .windows(2)
        .find(|pair| pair[0] == "--report")
        .map(|pair| &pair[1]);
    if let Some(path) = report {
        std::fs::write(path, "running\n").unwrap();
    }
    if let Some(path) = evidence {
        record["status"] = "running".into();
        std::fs::write(path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
    }
    let result = std::panic::catch_unwind(|| {
        if request {
            native::request_accessibility();
            Vec::new()
        } else {
            native::run()
        }
    });
    match &result {
        Ok(cases) => {
            record["status"] = "pass".into();
            record["cases"] = serde_json::json!(cases);
            record["cleanup"] =
                serde_json::json!({"external_process_reaped": true, "host_windows_closed": true});
        }
        Err(_) => record["status"] = "failed".into(),
    }
    if let Some(path) = evidence {
        std::fs::write(path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
    }
    if let Some(path) = report {
        let status = match &result {
            Ok(_) => "passed\n".to_owned(),
            Err(error) => format!(
                "failed: {}\n",
                error
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| error.downcast_ref::<&str>().copied())
                    .unwrap_or("non-string panic")
            ),
        };
        std::fs::write(path, status).unwrap();
    }
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}

#[cfg(target_os = "macos")]
mod native {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send, sel};
    use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};
    use std::path::Path;
    use std::process::{Child, Command};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    struct Fixture(Child);

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    unsafe fn window(title: &str, x: f64) -> *mut AnyObject {
        let allocated: *mut AnyObject = msg_send![class!(NSWindow), alloc];
        let window: *mut AnyObject = msg_send![allocated,
            initWithContentRect: NSRect::new(NSPoint::new(x, 200.0), NSSize::new(360.0, 240.0))
            styleMask: 15u64
            backing: 2u64
            defer: false
        ];
        assert!(!window.is_null());
        let _: () = msg_send![window, setReleasedWhenClosed: false];
        let _: () = msg_send![window, setTitle: &*NSString::from_str(title)];
        let _: () = msg_send![window, makeKeyAndOrderFront: std::ptr::null::<AnyObject>()];
        window
    }

    unsafe fn pump(app: *mut AnyObject) {
        let until: *mut AnyObject =
            msg_send![class!(NSDate), dateWithTimeIntervalSinceNow: 0.01f64];
        let event: *mut AnyObject = msg_send![app,
            nextEventMatchingMask: u64::MAX
            untilDate: until
            inMode: &*NSString::from_str("kCFRunLoopDefaultMode")
            dequeue: true
        ];
        if !event.is_null() {
            let _: () = msg_send![app, sendEvent: event];
        }
        let _: () = msg_send![app, updateWindows];
    }

    unsafe fn install_menu(app: *mut AnyObject) {
        let menu: *mut AnyObject = msg_send![class!(NSMenu), new];
        let application_item: *mut AnyObject = msg_send![class!(NSMenuItem), new];
        let application_menu: *mut AnyObject = msg_send![class!(NSMenu), new];
        let _: () = msg_send![application_item, setSubmenu: application_menu];
        let _: () = msg_send![menu, addItem: application_item];
        let root: *mut AnyObject = msg_send![class!(NSMenuItem), new];
        let _: () = msg_send![root, setTitle: &*NSString::from_str("Window")];
        let submenu: *mut AnyObject = msg_send![class!(NSMenu), new];
        let _: () = msg_send![submenu, setTitle: &*NSString::from_str("Window")];
        let allocated: *mut AnyObject = msg_send![class!(NSMenuItem), alloc];
        let item: *mut AnyObject = msg_send![allocated,
            initWithTitle: &*NSString::from_str("Minimize")
            action: sel!(performMiniaturize:)
            keyEquivalent: &*NSString::from_str("")
        ];
        let _: () = msg_send![submenu, addItem: item];
        let _: () = msg_send![root, setSubmenu: submenu];
        let _: () = msg_send![menu, addItem: root];
        let _: () = msg_send![app, setMainMenu: menu];
    }

    unsafe fn child(app: *mut AnyObject, directory: &Path) {
        install_menu(app);
        let target = window("embedded menu target", 650.0);
        let _: () = msg_send![app, activateIgnoringOtherApps: true];
        let wid: i64 = msg_send![target, windowNumber];
        std::fs::write(directory.join("window.pending"), wid.to_string()).unwrap();
        std::fs::rename(directory.join("window.pending"), directory.join("window")).unwrap();
        let deadline = Instant::now() + Duration::from_secs(45);
        while Instant::now() < deadline {
            pump(app);
            let minimized: bool = msg_send![target, isMiniaturized];
            if minimized && !directory.join("minimized").exists() {
                std::fs::write(directory.join("minimized"), "true").unwrap();
            }
        }
    }

    pub fn request_accessibility() {
        use core_foundation::{
            base::TCFType, boolean::CFBoolean, dictionary::CFDictionary, string::CFString,
        };
        let _main =
            MainThreadMarker::new().expect("permission request must run on the main thread");
        unsafe {
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let _: bool = msg_send![app, setActivationPolicy: 0i64];
            let _: () = msg_send![app, finishLaunching];
            let options = CFDictionary::from_CFType_pairs(&[(
                CFString::new("AXTrustedCheckOptionPrompt"),
                CFBoolean::true_value(),
            )]);
            platform_macos::ax::bindings::AXIsProcessTrustedWithOptions(
                options.as_concrete_TypeRef(),
            );
            let deadline = Instant::now() + Duration::from_secs(180);
            while !platform_macos::ax::bindings::AXIsProcessTrusted() {
                assert!(
                    Instant::now() < deadline,
                    "Accessibility authorization was not granted"
                );
                pump(app);
            }
        }
    }

    fn embedded_driver() -> std::sync::Arc<cua_driver_sdk::CuaDriver> {
        cua_driver_sdk::CuaDriver::try_create_for_host(cua_driver_sdk::DriverHostOptions {
            cursor: cursor_overlay::CursorConfig {
                enabled: false,
                ..Default::default()
            },
            host_owns_permission_ux: true,
            host_bundle_id: None,
            claude_code_compatibility: false,
            prepare_desktop_environment: false,
            register_host_tools: None,
            authorization_host: None,
            activity_observer: None,
        })
        .unwrap()
    }

    fn frame_request(id: u32, before: platform_macos::windows::WindowBounds) -> serde_json::Value {
        serde_json::json!({"pid": std::process::id(), "window_id": id,
            "x": before.x + 25.0, "y": before.y + 25.0,
            "width": before.width + 40.0, "height": before.height + 30.0})
    }

    unsafe fn wait_event(
        app: *mut AnyObject,
        events: &mpsc::Receiver<&str>,
        expected: &str,
        drain: bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match events.try_recv() {
                Ok(event) => {
                    assert_eq!(event, expected);
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => panic!("native lifecycle worker failed"),
                Err(mpsc::TryRecvError::Empty) => {}
            }
            assert!(
                Instant::now() < deadline,
                "native lifecycle event timed out: {expected}"
            );
            if drain {
                pump(app);
            } else {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    unsafe fn drain_for(app: *mut AnyObject, duration: Duration) {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            pump(app);
        }
    }

    unsafe fn native_lifecycle_cases(
        app: *mut AnyObject,
        external_pid: u32,
    ) -> Vec<serde_json::Value> {
        use platform_macos::windows::window_bounds_by_id as bounds;
        let target = window("embedded lifecycle target", 320.0);
        let id: i64 = msg_send![target, windowNumber];
        let id = id as u32;
        let before = bounds(id).unwrap();
        let request = frame_request(id, before);
        let worker_request = request.clone();
        let (event_tx, event_rx) = mpsc::channel();
        let (gate_tx, gate_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            let driver = embedded_driver();
            event_tx.send("ready").unwrap();
            gate_rx.recv().unwrap();
            let invoke = |driver: std::sync::Arc<cua_driver_sdk::CuaDriver>| {
                let args = worker_request.to_string();
                runtime
                    .spawn(async move { driver.call_tool("set_window_frame".into(), args).await })
            };
            let queued = invoke(driver.clone());
            std::thread::sleep(Duration::from_millis(200));
            assert!(!queued.is_finished());
            queued.abort();
            assert!(runtime.block_on(queued).unwrap_err().is_cancelled());
            event_tx.send("cancelled-before-drain").unwrap();
            gate_rx.recv().unwrap();
            let unavailable = runtime.block_on(invoke(driver.clone())).unwrap().unwrap();
            assert!(
                unavailable.is_error
                    && unavailable.text.contains("AppKit main queue did not start"),
                "{unavailable:?}"
            );
            event_tx.send("unavailable-before-drain").unwrap();
            gate_rx.recv().unwrap();
            let started = invoke(driver.clone());
            event_tx.send("requested").unwrap();
            gate_rx.recv().unwrap(); // Main observed an actual write and made correction necessary.
            started.abort();
            assert!(runtime.block_on(started).unwrap_err().is_cancelled());
            let mut shutdown = runtime.spawn(async move { driver.shutdown().await });
            assert!(
                runtime
                    .block_on(async {
                        tokio::time::timeout(Duration::from_millis(200), &mut shutdown).await
                    })
                    .is_err(),
                "shutdown completed while the AppKit correction queue was deliberately stalled"
            );
            event_tx.send("shutdown-pending").unwrap();
            runtime.block_on(shutdown).unwrap().unwrap();
            event_tx.send("shutdown-finished").unwrap();
        });
        wait_event(app, &event_rx, "ready", true);
        gate_tx.send(()).unwrap();
        wait_event(app, &event_rx, "cancelled-before-drain", false);
        drain_for(app, Duration::from_millis(400));
        let after_cancel = bounds(id).unwrap();
        assert_eq!(
            serde_json::to_value(before).unwrap(),
            serde_json::to_value(after_cancel).unwrap()
        );
        gate_tx.send(()).unwrap();
        wait_event(app, &event_rx, "unavailable-before-drain", false);
        drain_for(app, Duration::from_millis(400));
        let after_timeout = bounds(id).unwrap();
        assert_eq!(
            serde_json::to_value(before).unwrap(),
            serde_json::to_value(after_timeout).unwrap()
        );
        gate_tx.send(()).unwrap();
        wait_event(app, &event_rx, "requested", true);
        let deadline = Instant::now() + Duration::from_secs(5);
        while (bounds(id).unwrap().x - before.x).abs() <= 2.0 {
            assert!(Instant::now() < deadline, "native write never started");
            pump(app);
        }
        let initial_write = bounds(id).unwrap();
        let frame: NSRect = msg_send![target, frame];
        let _: () =
            msg_send![target, setFrameOrigin: NSPoint::new(frame.origin.x + 20.0, frame.origin.y)];
        let disturbed = bounds(id).unwrap();
        assert!((disturbed.x - request["x"].as_f64().unwrap()).abs() > 2.0);
        gate_tx.send(()).unwrap();
        wait_event(app, &event_rx, "shutdown-pending", false);
        wait_event(app, &event_rx, "shutdown-finished", true);
        worker.join().unwrap();
        let after_shutdown = bounds(id).unwrap();
        for component in ["x", "y", "width", "height"] {
            assert!(
                (serde_json::to_value(after_shutdown).unwrap()[component]
                    .as_f64()
                    .unwrap()
                    - request[component].as_f64().unwrap())
                .abs()
                    <= 2.0
            );
        }
        let mut cases = vec![
            serde_json::json!({"id":"cancel-before-mainqueue-drain", "status":"pass", "before":before, "observed":after_cancel,
                "public_future_cancelled":true, "main_queue_drained_after_cancel":true, "queue_arrival_instrumented":false}),
            serde_json::json!({"id":"unavailable-mainqueue-late-drain", "status":"pass", "before":before, "observed":after_timeout,
                "unavailable_error_observed":true, "main_queue_drained_after_error":true}),
            serde_json::json!({"id":"started-cancel-shutdown-correction", "status":"pass", "before":before, "initial_write":initial_write,
                "disturbed":disturbed, "requested":request, "observed":after_shutdown,
                "shutdown_pending_while_main_queue_stalled":true, "shutdown_completed_after_drain":true}),
        ];
        // Closed and foreign-owner requests must refuse; closing/replacing after a write
        // must preserve an uncertain action outcome and leave the new window untouched.
        let stale = window("embedded closed target", 400.0);
        let stale_id: i64 = msg_send![stale, windowNumber];
        let _: () = msg_send![stale, close];
        let _: () = msg_send![stale, release];
        drain_for(app, Duration::from_millis(100));
        assert!(bounds(stale_id as u32).is_none());
        let request = frame_request(id, after_shutdown);
        let worker_request = request.clone();
        let (event_tx, event_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            let driver = embedded_driver();
            let mut closed = worker_request.clone();
            closed["window_id"] = stale_id.into();
            let mut foreign = worker_request.clone();
            foreign["pid"] = external_pid.into();
            let mut refusals = Vec::new();
            for (name, tool, args) in [
                ("closed", "set_window_frame", closed),
                ("wrong-owner", "set_window_frame", foreign),
                (
                    "public-own-pid",
                    "bring_to_front",
                    serde_json::json!({"pid":std::process::id(),"window_id":id}),
                ),
            ] {
                let result = runtime
                    .block_on(driver.call_tool(tool.into(), args.to_string()))
                    .unwrap();
                assert!(result.is_error, "expected {name} refusal: {result:?}");
                let reason = match name {
                    "closed" => "closed",
                    "wrong-owner" => "belongs to pid",
                    "public-own-pid" => "own authorization process",
                    _ => unreachable!(),
                };
                assert!(
                    result.text.contains(reason),
                    "wrong refusal path for {name}: {result:?}"
                );
                refusals.push(
                    serde_json::json!({"kind":name,"is_error":result.is_error,"text":result.text}),
                );
            }
            event_tx.send("refusals-finished").unwrap();
            let result = runtime
                .block_on(driver.call_tool("set_window_frame".into(), worker_request.to_string()))
                .unwrap();
            assert!(
                !result.is_error,
                "a correction failure lost the action outcome: {result:?}"
            );
            assert_eq!(
                result.action.as_ref().unwrap().effect,
                cua_driver_contract::ActionEffect::Unverifiable
            );
            runtime.block_on(driver.shutdown()).unwrap();
            result_tx.send((result.action, refusals)).unwrap();
            event_tx.send("closed-correction-finished").unwrap();
        });
        wait_event(app, &event_rx, "refusals-finished", true);
        let deadline = Instant::now() + Duration::from_secs(5);
        while (bounds(id).unwrap().x - after_shutdown.x).abs() <= 2.0 {
            assert!(
                Instant::now() < deadline,
                "correction-close initial write never started"
            );
            pump(app);
        }
        let write_before_close = bounds(id).unwrap();
        let _: () = msg_send![target, close];
        let _: () = msg_send![target, release];
        let replacement = window("embedded replacement must not change", 500.0);
        let replacement_id: i64 = msg_send![replacement, windowNumber];
        assert_ne!(replacement_id as u32, id);
        let replacement_before = bounds(replacement_id as u32).unwrap();
        wait_event(app, &event_rx, "closed-correction-finished", true);
        worker.join().unwrap();
        let (action, refusals) = result_rx.recv().unwrap();
        let replacement_after = bounds(replacement_id as u32).unwrap();
        assert_eq!(
            serde_json::to_value(replacement_before).unwrap(),
            serde_json::to_value(replacement_after).unwrap()
        );
        cases.push(serde_json::json!({"id":"closed-wrong-owner-public-auth", "status":"pass", "refusals":refusals}));
        cases.push(serde_json::json!({"id":"correction-close-replacement", "status":"pass", "initial_write":write_before_close,
            "closed_window_id":id, "replacement_window_id":replacement_id, "replacement_before":replacement_before,
            "replacement_after":replacement_after, "action":action}));
        let _: () = msg_send![replacement, close];
        let _: () = msg_send![replacement, release];
        drain_for(app, Duration::from_millis(100));
        assert!(bounds(replacement_id as u32).is_none());
        cases
    }

    pub fn process_status() -> serde_json::Value {
        let mut ancestry = Vec::new();
        let mut pid = std::process::id();
        for _ in 0..16 {
            let output = Command::new("/bin/ps")
                .args(["-p", &pid.to_string(), "-o", "pid=,ppid=,comm="])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "could not observe fixture ancestry"
            );
            let row = String::from_utf8(output.stdout).unwrap();
            let parent = row
                .split_whitespace()
                .nth(1)
                .unwrap()
                .parse::<u32>()
                .unwrap();
            ancestry.push(row.trim().to_owned());
            if parent == 0 || parent == pid {
                break;
            }
            pid = parent;
        }
        let (bundle_path, bundle_id) = unsafe {
            let bundle: *mut AnyObject = msg_send![class!(NSBundle), mainBundle];
            let path: *const NSString = msg_send![bundle, bundlePath];
            let identifier: *const NSString = msg_send![bundle, bundleIdentifier];
            (
                path.as_ref().map(ToString::to_string),
                identifier.as_ref().map(ToString::to_string),
            )
        };
        serde_json::json!({
            "bundle_path": bundle_path, "bundle_id": bundle_id,
            "pid": std::process::id(), "parent_pid": unsafe { libc::getppid() },
            "executable": std::env::current_exe().unwrap(),
            "ax_trusted": unsafe { platform_macos::ax::bindings::AXIsProcessTrusted() },
            "appkit_main_thread": MainThreadMarker::new().is_some(),
            "ancestry": ancestry,
        })
    }

    pub fn run() -> Vec<serde_json::Value> {
        let _main = MainThreadMarker::new().expect("fixture must own the AppKit main thread");
        unsafe {
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let _: bool = msg_send![app, setActivationPolicy: 0i64];
            let _: () = msg_send![app, finishLaunching];
            if let Some(directory) = std::env::var_os("CUA_EMBEDDED_MENU_CHILD") {
                child(app, Path::new(&directory));
                return Vec::new();
            }
            assert!(
                platform_macos::ax::bindings::AXIsProcessTrusted(),
                "Accessibility permission is required for this test executable"
            );
            let directory = tempfile::tempdir().unwrap();
            let mut fixture = Fixture(
                Command::new(std::env::current_exe().unwrap())
                    .arg("--run-gui")
                    .env("CUA_EMBEDDED_MENU_CHILD", directory.path())
                    .spawn()
                    .unwrap(),
            );
            let deadline = Instant::now() + Duration::from_secs(10);
            while !directory.path().join("window").exists() {
                assert!(
                    fixture.0.try_wait().unwrap().is_none(),
                    "target fixture exited"
                );
                assert!(
                    Instant::now() < deadline,
                    "target fixture did not publish a window"
                );
                pump(app);
            }
            let target_wid: u32 = std::fs::read_to_string(directory.path().join("window"))
                .unwrap()
                .parse()
                .unwrap();
            let original = window("embedded host original", 100.0);
            let distractor = window("embedded host distractor", 200.0);
            let _: () = msg_send![original, makeKeyAndOrderFront: std::ptr::null::<AnyObject>()];
            let _: () = msg_send![app, activateIgnoringOtherApps: true];
            let original_wid: i64 = msg_send![original, windowNumber];
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stable_since = None;
            loop {
                pump(app);
                let key: bool = msg_send![original, isKeyWindow];
                let ready = key
                    && platform_macos::apps::frontmost_pid() == Some(std::process::id() as i32)
                    && platform_macos::ax::bindings::focused_window_id_of_pid(
                        std::process::id() as i32
                    ) == Some(original_wid as u32);
                if ready {
                    if stable_since.get_or_insert_with(Instant::now).elapsed()
                        >= Duration::from_millis(300)
                    {
                        break;
                    }
                } else {
                    stable_since = None;
                }
                assert!(
                    Instant::now() < deadline,
                    "host precondition: exact original window never became key"
                );
            }
            let target_pid = fixture.0.id();
            let (tx, rx) = mpsc::channel();
            let worker = std::thread::spawn(move || {
                let runtime = tokio::runtime::Runtime::new().unwrap();
                let driver = cua_driver_sdk::CuaDriver::try_create_for_host(
                    cua_driver_sdk::DriverHostOptions {
                        cursor: cursor_overlay::CursorConfig {
                            enabled: false,
                            ..Default::default()
                        },
                        host_owns_permission_ux: true,
                        host_bundle_id: None,
                        claude_code_compatibility: false,
                        prepare_desktop_environment: false,
                        register_host_tools: None,
                        authorization_host: None,
                        activity_observer: None,
                    },
                )
                .unwrap();
                let mut cases = Vec::new();
                for (case, pid, id, resize) in [
                    ("own-move", std::process::id(), original_wid as u32, false),
                    ("own-resize", std::process::id(), original_wid as u32, true),
                    ("external-move", target_pid, target_wid, false),
                    ("external-resize", target_pid, target_wid, true),
                ] {
                    let before = platform_macos::windows::window_bounds_by_id(id).unwrap();
                    let requested = serde_json::json!({
                        "pid": pid, "window_id": id,
                        "x": before.x + 25.0, "y": before.y + 25.0,
                        "width": before.width + if resize { 40.0 } else { 0.0 },
                        "height": before.height + if resize { 30.0 } else { 0.0 },
                    });
                    let result = runtime
                        .block_on(
                            driver.call_tool("set_window_frame".into(), requested.to_string()),
                        )
                        .unwrap();
                    assert!(!result.is_error, "frame call failed: {result:?}");
                    let observed = serde_json::to_value(
                        platform_macos::windows::window_bounds_by_id(id).unwrap(),
                    )
                    .unwrap();
                    for component in ["x", "y", "width", "height"] {
                        assert!((observed[component].as_f64().unwrap() - requested[component].as_f64().unwrap()).abs() <= 2.0,
                            "independent geometry mismatch for {component}: requested={requested} observed={observed}");
                    }
                    cases.push(serde_json::json!({
                        "id": case, "status": "pass", "pid": pid, "window_id": id,
                        "before": before, "requested": requested, "observed": observed,
                    }));
                    println!("embedded frame: pid={pid} window={id} requested={requested} observed={observed}");
                }
                let result = runtime
                    .block_on(
                        driver.call_tool(
                            "invoke_menu".into(),
                            serde_json::json!({
                                "pid": target_pid,
                                "window_id": target_wid,
                                "path": ["Window", "Minimize"]
                            })
                            .to_string(),
                        ),
                    )
                    .unwrap();
                runtime.block_on(driver.shutdown()).unwrap();
                tx.send((result, cases)).unwrap();
            });
            let deadline = Instant::now() + Duration::from_secs(25);
            let (result, mut cases) = loop {
                pump(app);
                match rx.try_recv() {
                    Ok(result) => break result,
                    Err(mpsc::TryRecvError::Disconnected) => panic!("embedded worker failed"),
                    Err(mpsc::TryRecvError::Empty) => {}
                }
                assert!(Instant::now() < deadline, "embedded invoke_menu timed out");
            };
            worker.join().unwrap();
            assert!(!result.is_error, "invoke_menu failed: {result:?}");
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                pump(app);
                let key: bool = msg_send![original, isKeyWindow];
                let other_key: bool = msg_send![distractor, isKeyWindow];
                if key
                    && !other_key
                    && platform_macos::apps::frontmost_pid() == Some(std::process::id() as i32)
                    && directory.path().join("minimized").exists()
                {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "menu did not minimize the target and restore the exact native host key window"
                );
            }
            let focused_window_id =
                platform_macos::ax::bindings::focused_window_id_of_pid(std::process::id() as i32);
            assert_eq!(focused_window_id, Some(original_wid as u32));
            let original_key: bool = msg_send![original, isKeyWindow];
            let distractor_key: bool = msg_send![distractor, isKeyWindow];
            cases.push(serde_json::json!({
                "id": "external-menu-host-restore", "status": "pass",
                "target_minimized": directory.path().join("minimized").exists(),
                "original_key": original_key, "distractor_key": distractor_key,
                "original_window_id": original_wid, "focused_window_id": focused_window_id,
                "frontmost_pid": platform_macos::apps::frontmost_pid(),
            }));
            cases.extend(native_lifecycle_cases(app, target_pid));
            if fixture.0.try_wait().unwrap().is_none() {
                fixture.0.kill().unwrap();
            }
            fixture.0.wait().expect("external fixture reaped");
            let distractor_wid: i64 = msg_send![distractor, windowNumber];
            for window in [original, distractor] {
                let _: () = msg_send![window, close];
                let _: () = msg_send![window, release];
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            while [original_wid, distractor_wid]
                .iter()
                .any(|id| platform_macos::windows::window_bounds_by_id(*id as u32).is_some())
            {
                assert!(
                    Instant::now() < deadline,
                    "host windows remained after cleanup"
                );
                pump(app);
            }
            println!(
                "embedded_menu_restore: passed (target minimized; exact host window restored)"
            );
            cases
        }
    }
}
