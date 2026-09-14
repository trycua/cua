#![cfg(target_os = "linux")]

use cua_driver_core::action_record::ActionEffect;
use cua_driver_testkit::{keyboard_fixture::KeyboardFixture, spawn_in_job, ChildReaper};
use serde_json::json;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream, UdpSocket};
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Condvar, Mutex,
};

#[derive(Clone, Copy)]
enum Interruption {
    Disconnect,
    Cancel,
}

struct Receipt(Arc<(Mutex<bool>, Condvar)>);

impl Receipt {
    fn release(&self) {
        let (lock, wake) = &*self.0;
        *lock.lock().unwrap() = true;
        wake.notify_all();
    }
}

impl Drop for Receipt {
    fn drop(&mut self) {
        self.release();
    }
}

struct ProxyLifetime {
    running: Arc<AtomicBool>,
    server: Option<std::thread::JoinHandle<()>>,
}

impl Drop for ProxyLifetime {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
    }
}

fn number(bytes: &[u8], little: bool) -> u16 {
    if little {
        u16::from_le_bytes([bytes[0], bytes[1]])
    } else {
        u16::from_be_bytes([bytes[0], bytes[1]])
    }
}

fn authentication() -> (Vec<u8>, Vec<u8>) {
    let output = Command::new("xauth")
        .args(["list", &std::env::var("DISPLAY").unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "could not read this test display's existing authority"
    );
    let text = String::from_utf8(output.stdout).unwrap();
    let fields: Vec<_> = text.split_whitespace().collect();
    if fields.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let cookie = fields[2]
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect();
    (fields[1].as_bytes().to_vec(), cookie)
}

fn relay(
    mut client: TcpStream,
    socket: &str,
    auth: &(Vec<u8>, Vec<u8>),
    window: u32,
    receipt: Arc<(Mutex<bool>, Condvar)>,
    interruption: Interruption,
) -> std::io::Result<()> {
    let mut server = UnixStream::connect(socket)?;
    let mut header = [0; 12];
    client.read_exact(&mut header)?;
    let little = header[0] == b'l';
    let padded = |n: usize| (n + 3) & !3;
    let mut supplied = vec![
        0;
        padded(number(&header[6..8], little) as usize)
            + padded(number(&header[8..10], little) as usize)
    ];
    client.read_exact(&mut supplied)?;
    let encode = |n: u16| {
        if little {
            n.to_le_bytes()
        } else {
            n.to_be_bytes()
        }
    };
    header[6..8].copy_from_slice(&encode(auth.0.len() as u16));
    header[8..10].copy_from_slice(&encode(auth.1.len() as u16));
    server.write_all(&header)?;
    for value in [&auth.0, &auth.1] {
        server.write_all(value)?;
        server.write_all(&vec![0; padded(value.len()) - value.len()])?;
    }
    let mut back = server.try_clone()?;
    let mut front = client.try_clone()?;
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut back, &mut front);
    });
    let result = (|| loop {
        let mut header = [0; 4];
        client.read_exact(&mut header)?;
        let size = usize::from(number(&header[2..4], little)) * 4;
        if size < 4 {
            return Err(std::io::Error::other("unexpected X11 extended request"));
        }
        let mut request = vec![0; size];
        request[..4].copy_from_slice(&header);
        client.read_exact(&mut request[4..])?;
        server.write_all(&request)?;
        let target = if request.len() >= 8 {
            let bytes = request[4..8].try_into().unwrap();
            if little {
                u32::from_le_bytes(bytes)
            } else {
                u32::from_be_bytes(bytes)
            }
        } else {
            0
        };
        if request[0] == 25 && request.get(12) == Some(&2) && target == window {
            let (lock, wake) = &*receipt;
            let mut observed = lock.lock().unwrap();
            while !*observed {
                observed = wake.wait(observed).unwrap();
            }
            if matches!(interruption, Interruption::Disconnect) {
                return Ok(());
            }
        }
    })();
    let _ = client.shutdown(Shutdown::Both);
    let _ = server.shutdown(Shutdown::Both);
    result
}

#[test]
#[ignore = "requires an isolated X11/Openbox desktop and xauth"]
fn observed_key_down_then_connection_failure_is_not_a_clean_refusal() {
    fault_case(
        "press_key",
        "observed_key_down_then_connection_failure_is_not_a_clean_refusal",
        0xffc2,
        Interruption::Disconnect,
    );
}

#[test]
#[ignore = "requires an isolated X11/Openbox desktop and xauth"]
fn observed_modifier_down_then_connection_failure_is_not_a_clean_refusal() {
    fault_case(
        "hotkey",
        "observed_modifier_down_then_connection_failure_is_not_a_clean_refusal",
        0xffe3,
        Interruption::Disconnect,
    );
}

#[test]
#[ignore = "requires an isolated X11/Openbox desktop and xauth"]
fn cancelled_admitted_press_key_finishes_its_native_release() {
    fault_case(
        "press_key",
        "cancelled_admitted_press_key_finishes_its_native_release",
        0xffc2,
        Interruption::Cancel,
    );
}

#[test]
#[ignore = "requires an isolated X11/Openbox desktop and xauth"]
fn cancelled_admitted_hotkey_finishes_its_native_sequence_and_releases_modifiers() {
    fault_case(
        "hotkey",
        "cancelled_admitted_hotkey_finishes_its_native_sequence_and_releases_modifiers",
        0xffe3,
        Interruption::Cancel,
    );
}

fn cancel_child(
    runtime: tokio::runtime::Runtime,
    registry: cua_driver_core::tool::ToolRegistry,
    tool: &str,
    args: serde_json::Value,
) {
    let control = UdpSocket::bind("127.0.0.1:0").unwrap();
    control
        .set_read_timeout(Some(std::time::Duration::from_secs(15)))
        .unwrap();
    control
        .connect(std::env::var("CUA_KEYBOARD_CANCEL_PARENT").unwrap())
        .unwrap();
    let registry = Arc::new(registry);
    let pending_registry = registry.clone();
    let pending_args = args.clone();
    let tool = tool.to_owned();
    let pending = runtime.spawn(async move { pending_registry.invoke(&tool, pending_args).await });
    control.send(b"ready").unwrap();
    let mut message = [0; 32];
    let count = control.recv(&mut message).unwrap();
    assert_eq!(&message[..count], b"cancel");
    pending.abort();
    assert!(runtime.block_on(pending).unwrap_err().is_cancelled());
    control.send(b"cancelled").unwrap();
    let count = control.recv(&mut message).unwrap();
    assert_eq!(&message[..count], b"finished");
    let mut recovery = args;
    recovery.as_object_mut().unwrap().remove("keys");
    recovery["key"] = json!("f6");
    let result = runtime.block_on(registry.invoke("press_key", recovery));
    assert_ne!(result.is_error, Some(true), "{result:?}");
}

fn fault_case(tool: &str, test: &str, key: u64, interruption: Interruption) {
    if std::env::var_os("CUA_KEYBOARD_FAULT_CHILD").is_some() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let registry = platform_linux::tools::build_registry(false);
        let mut args = json!({
            "pid": std::env::var("CUA_KEYBOARD_TARGET_PID").unwrap().parse::<u32>().unwrap(),
            "window_id": std::env::var("CUA_KEYBOARD_TARGET_WINDOW").unwrap().parse::<u64>().unwrap(),
            "delivery_mode":"background"
        });
        if tool == "press_key" {
            args["key"] = json!("f5");
        } else {
            args["keys"] = json!(["ctrl", "h"]);
        }
        if matches!(interruption, Interruption::Cancel) {
            cancel_child(runtime, registry, tool, args);
            return;
        }
        let result = runtime.block_on(registry.invoke(tool, args));
        assert_eq!(
            result.is_error,
            Some(true),
            "the injected connection loss did not reach the native error path: {result:?}"
        );
        assert!(
            result.action_record.is_some(),
            "post-input error must carry an execution record: {result:?}"
        );
        let record = result.action_record.as_ref().unwrap();
        assert_ne!(
            record.effect,
            ActionEffect::Refused,
            "observed key down cannot become a clean refusal: {result:?}"
        );
        assert_ne!(record.effect, ActionEffect::Confirmed, "{result:?}");
        assert!(record.actual_delivery.is_some(), "{result:?}");
        return;
    }
    let fixture = KeyboardFixture::spawn(false);
    let display = std::env::var("DISPLAY").unwrap();
    let display = display
        .strip_prefix(':')
        .expect("requires a local isolated X11 display")
        .split('.')
        .next()
        .unwrap();
    let socket = format!("/tmp/.X11-unix/X{display}");
    let auth = authentication();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy_display = format!("127.0.0.1:{}", listener.local_addr().unwrap().port() - 6000);
    listener.set_nonblocking(true).unwrap();
    let receipt = Receipt(Arc::new((Mutex::new(false), Condvar::new())));
    let signal = receipt.0.clone();
    let window = fixture.window_id as u32;
    let running = Arc::new(AtomicBool::new(true));
    let alive = running.clone();
    let server = std::thread::spawn(move || {
        while alive.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((client, _)) => {
                    let socket = socket.clone();
                    let auth = auth.clone();
                    let signal = signal.clone();
                    std::thread::spawn(move || {
                        let _ = relay(client, &socket, &auth, window, signal, interruption);
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(2))
                }
                Err(error) => panic!("proxy accept: {error}"),
            }
        }
    });
    let _proxy = ProxyLifetime {
        running,
        server: Some(server),
    };
    let control = UdpSocket::bind("127.0.0.1:0").unwrap();
    control
        .set_read_timeout(Some(std::time::Duration::from_secs(15)))
        .unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", test, "--nocapture"])
        .env("CUA_KEYBOARD_FAULT_CHILD", "1")
        .env(
            "CUA_KEYBOARD_CANCEL_PARENT",
            control.local_addr().unwrap().to_string(),
        )
        .env("DISPLAY", proxy_display)
        .env("CUA_KEYBOARD_TARGET_PID", fixture.pid().to_string())
        .env("CUA_KEYBOARD_TARGET_WINDOW", fixture.window_id.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let mut child = spawn_in_job(&mut command).unwrap();
    let mut reaper = ChildReaper::new();
    reaper.track_pid(child.id());
    fixture.key_event("down", key);
    if matches!(interruption, Interruption::Cancel) {
        let mut message = [0; 32];
        let (count, peer) = control.recv_from(&mut message).unwrap();
        assert_eq!(&message[..count], b"ready");
        control.connect(peer).unwrap();
        control.send(b"cancel").unwrap();
        let count = control.recv(&mut message).unwrap();
        assert_eq!(&message[..count], b"cancelled");
        receipt.release();
        if tool == "hotkey" {
            fixture.key_event("down", 0x68);
            fixture.key_event("up", 0x68);
        }
        fixture.key_event("up", key);
        fixture.assert_no_key_down();
        control.send(b"finished").unwrap();
        assert_eq!(fixture.key_event("down", 0xffc3)["flags"], 0);
        fixture.key_event("up", 0xffc3);
    } else {
        receipt.release();
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "native input did not finish after its connection closed"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    fixture.assert_no_key_down();
    assert!(
        status.success(),
        "public dispatch misreported an independently observed partial keyboard attempt"
    );
}
