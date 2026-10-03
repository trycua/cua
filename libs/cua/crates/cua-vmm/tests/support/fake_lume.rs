//! A fake `lume serve` for the hermetic Lume tests: an in-memory VM table
//! behind a minimal HTTP/1.1 server that records every API call.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use cua_vmm::lume::{LumeConfig, LumeRuntime};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Temp CUA_HOME for the whole test binary (set once, before any runtime
/// reads it), so owned-VM records never touch the real `~/.cua`. HOME stays:
/// the live test's `lume ssh` finds VMs under the real `~/.lume`.
pub fn temp_home() -> &'static std::path::Path {
    static HOME: OnceLock<tempfile::TempDir> = OnceLock::new();
    HOME.get_or_init(|| {
        let d = tempfile::tempdir().unwrap();
        // SAFETY: set once, first thing in every test, before other threads
        // of this test read the environment.
        unsafe {
            std::env::set_var("CUA_HOME", d.path().join(".cua"));
        }
        d
    })
    .path()
}

#[derive(Default)]
pub struct FakeLume {
    pub vms: BTreeMap<String, Value>,
    /// `METHOD path body` for every request.
    pub calls: Vec<(String, String, Value)>,
    pub fail_set: bool,
    /// `POST /lume/vms/:name/run` never answers (a boot a cancelled create
    /// cut off).
    pub hang_run: bool,
}

fn not_found(name: &str) -> (u16, Value) {
    (
        404,
        json!({ "message": format!("Virtual machine not found: {name}") }),
    )
}

fn handle(st: &mut FakeLume, method: &str, path: &str, body: Value) -> (u16, Value) {
    st.calls
        .push((method.to_string(), path.to_string(), body.clone()));
    let name = path.strip_prefix("/lume/vms/").unwrap_or("");
    match (method, path) {
        ("GET", "/lume/host/status") => (200, json!({ "status": "ok" })),
        ("POST", "/lume/vms/clone") => {
            let src = body["name"].as_str().unwrap();
            let new = body["newName"].as_str().unwrap().to_string();
            let Some(mut vm) = st.vms.get(src).cloned() else {
                return not_found(src);
            };
            vm["name"] = json!(new);
            st.vms.insert(new, vm);
            (200, json!({ "message": "cloned" }))
        }
        ("GET", _) => match st.vms.get(name) {
            Some(vm) => (200, vm.clone()),
            None => not_found(name),
        },
        ("PATCH", _) => {
            if st.fail_set {
                return (400, json!({ "message": "set refused" }));
            }
            let Some(vm) = st.vms.get_mut(name) else {
                return not_found(name);
            };
            assert_eq!(vm["status"], "stopped", "lume set needs a stopped VM");
            if let Some(c) = body.get("cpu") {
                vm["cpuCount"] = c.clone();
            }
            if let Some(m) = body.get("memory").and_then(Value::as_str) {
                let mb: u64 = m.strip_suffix("MB").unwrap().parse().unwrap();
                vm["memorySize"] = json!(mb << 20);
            }
            (
                200,
                json!({ "message": "VM settings updated successfully" }),
            )
        }
        ("POST", p) if p.ends_with("/run") => {
            let name = p.trim_start_matches("/lume/vms/").trim_end_matches("/run");
            let Some(vm) = st.vms.get_mut(name) else {
                return not_found(name);
            };
            vm["status"] = json!("running");
            vm["ipAddress"] = json!("192.0.2.10");
            (202, json!({ "message": "starting" }))
        }
        ("POST", p) if p.ends_with("/stop") => {
            let name = p.trim_start_matches("/lume/vms/").trim_end_matches("/stop");
            let Some(vm) = st.vms.get_mut(name) else {
                return not_found(name);
            };
            vm["status"] = json!("stopped");
            vm["ipAddress"] = Value::Null;
            (202, json!({ "message": "stopping" }))
        }
        ("DELETE", _) => match st.vms.remove(name) {
            Some(_) => (200, json!({})),
            None => not_found(name),
        },
        _ => (
            404,
            json!({ "message": format!("no route {method} {path}") }),
        ),
    }
}

/// Minimal HTTP/1.1 server (one request per connection, like lume).
pub async fn serve(state: Arc<Mutex<FakeLume>>) -> String {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                return;
            };
            let state = state.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                // Bounded read: headers + body never exceed 1 MiB here.
                let (head_end, len) = loop {
                    let n = s.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 || buf.len() > 1 << 20 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..i]).to_ascii_lowercase();
                        let len = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        break (i + 4, len);
                    }
                };
                while buf.len() < head_end + len && buf.len() <= 1 << 20 {
                    let n = s.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let mut first = head.lines().next().unwrap_or("").split(' ');
                let (method, path) = (first.next().unwrap_or(""), first.next().unwrap_or(""));
                let body = serde_json::from_slice(&buf[head_end..]).unwrap_or(Value::Null);
                let hang =
                    method == "POST" && path.ends_with("/run") && state.lock().unwrap().hang_run;
                if hang {
                    state
                        .lock()
                        .unwrap()
                        .calls
                        .push((method.to_string(), path.to_string(), body));
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    return;
                }
                let (code, resp) = handle(&mut state.lock().unwrap(), method, path, body);
                let resp = resp.to_string();
                let out = format!(
                    "HTTP/1.1 {code} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{resp}",
                    resp.len()
                );
                let _ = s.write_all(out.as_bytes()).await;
            });
        }
    });
    url
}

/// A [`LumeRuntime`] on the fake at `url`, with its state under the test
/// binary's temp home and short timeouts.
pub fn runtime(url: String) -> LumeRuntime {
    let home = temp_home();
    LumeRuntime::new(LumeConfig {
        url,
        spawn_serve: false,
        allow_install: false,
        root: home.join("lume-root"),
        ip_timeout: Duration::from_secs(10),
        pull_timeout: Duration::from_secs(10),
        leases_path: home.join("no-leases").display().to_string(),
    })
}
