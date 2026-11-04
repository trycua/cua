#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let request = args.iter().any(|arg| arg == "--request-accessibility");
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
    let result = std::panic::catch_unwind(|| {
        if request {
            native::request_accessibility();
        } else {
            native::run();
        }
    });
    if let Some(path) = report {
        let status = match &result {
            Ok(()) => "passed\n".to_owned(),
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

    pub fn run() {
        let _main = MainThreadMarker::new().expect("fixture must own the AppKit main thread");
        unsafe {
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let _: bool = msg_send![app, setActivationPolicy: 0i64];
            let _: () = msg_send![app, finishLaunching];
            if let Some(directory) = std::env::var_os("CUA_EMBEDDED_MENU_CHILD") {
                child(app, Path::new(&directory));
                return;
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
                let registry = platform_macos::register_tools();
                let result = runtime.block_on(registry.invoke(
                    "invoke_menu",
                    serde_json::json!({
                        "pid": target_pid,
                        "window_id": target_wid,
                        "path": ["Window", "Minimize"]
                    }),
                ));
                tx.send(result).unwrap();
            });
            let deadline = Instant::now() + Duration::from_secs(25);
            let result = loop {
                pump(app);
                match rx.try_recv() {
                    Ok(result) => break result,
                    Err(mpsc::TryRecvError::Disconnected) => panic!("embedded worker failed"),
                    Err(mpsc::TryRecvError::Empty) => {}
                }
                assert!(Instant::now() < deadline, "embedded invoke_menu timed out");
            };
            worker.join().unwrap();
            assert!(
                !result.is_error.unwrap_or(false),
                "invoke_menu failed: {result:?}"
            );
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
            println!(
                "embedded_menu_restore: passed (target minimized; exact host window restored)"
            );
        }
    }
}
