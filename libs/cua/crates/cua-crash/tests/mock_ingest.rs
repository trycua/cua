//! A panic goes to a local mock ingest only when consent is on, and what
//! arrives is scrubbed. One test function: the panic hook and the SDK's main
//! hub are process-wide.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

type Bodies = Arc<Mutex<Vec<String>>>;

fn read_request(mut s: TcpStream) -> Option<String> {
    s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = s.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_lowercase();
    let len: usize = head
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    while buf.len() < head_end + len {
        let n = s.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let _ = s.write_all(
        b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}",
    );
    Some(String::from_utf8_lossy(&buf[head_end..]).into_owned())
}

fn mock_ingest() -> (u16, Bodies) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let bodies: Bodies = Arc::default();
    let sink = bodies.clone();
    std::thread::spawn(move || {
        for s in listener.incoming().flatten() {
            if let Some(b) = read_request(s) {
                sink.lock().unwrap().push(b);
            }
        }
    });
    (port, bodies)
}

fn config(port: u16) -> cua_crash::Config {
    cua_crash::Config {
        dsn: format!("http://publickey@127.0.0.1:{port}/42"),
        release: Some("cua-spaces-desktop@0.0.0-test".into()),
        environment: "local".into(),
        shell: "swift".into(),
        component: "core".into(),
    }
}

fn panic_in_thread() {
    let _ = std::thread::spawn(|| {
        panic!("opened /Users/alicia/Secret Space.txt for alicia@example.com");
    })
    .join();
}

#[test]
fn consent_gates_and_scrubs() {
    // Consent off: the panic produces no request.
    let (port, bodies) = mock_ingest();
    let guard = cua_crash::init_with(config(port), Arc::new(|| false)).expect("init");
    panic_in_thread();
    drop(guard);
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        bodies.lock().unwrap().is_empty(),
        "nothing may be sent with consent off"
    );

    // Consent on (and revoked later in the same run): the event arrives,
    // scrubbed.
    let (port, bodies) = mock_ingest();
    let on = Arc::new(AtomicBool::new(true));
    let flag = on.clone();
    let guard = cua_crash::init_with(config(port), Arc::new(move || flag.load(Ordering::SeqCst)))
        .expect("init");
    sentry::configure_scope(|s| {
        s.set_extra("note", "from /home/bobby/x".into());
        s.set_user(Some(sentry::User {
            email: Some("caroline@example.com".into()),
            ..Default::default()
        }));
    });
    sentry::add_breadcrumb(sentry::Breadcrumb {
        message: Some("click on Secret Space".into()),
        ..Default::default()
    });
    panic_in_thread();
    // The panic path flushes before it returns; no sleep needed.
    let got = bodies.lock().unwrap().clone();
    assert_eq!(got.len(), 1, "one envelope expected, got {got:?}");
    let body = &got[0];
    for leaked in [
        "alicia",
        "bobby",
        "caroline",
        "example.com",
        "Secret",
        "Space.txt",
    ] {
        assert!(!body.contains(leaked), "{leaked} leaked");
    }
    for want in [
        "\"type\":\"panic\"",
        "\"shell\":\"swift\"",
        "\"component\":\"core\"",
        "cua-spaces-desktop@0.0.0-test",
        "\"environment\":\"local\"",
        "~/",
    ] {
        assert!(body.contains(want), "{want} missing");
    }
    assert!(!body.contains("server_name"), "host name sent");
    assert!(!body.contains("\"user\""), "user sent");
    assert!(!body.contains("breadcrumbs"), "breadcrumbs sent");

    // Consent withdrawn mid-run: the next panic is dropped.
    on.store(false, Ordering::SeqCst);
    panic_in_thread();
    drop(guard);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        bodies.lock().unwrap().len(),
        1,
        "revoked consent still sent"
    );
}
