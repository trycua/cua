use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Fixture-owned state receiver used as an oracle independently of cua-driver.
///
/// Web fixtures POST their current DOM state to this loopback endpoint. Tests
/// may still use AX or pixels to target an action, but delivery is judged from
/// this state rather than by reading the driver's own accessibility snapshot.
pub struct FixtureJournal {
    url: String,
    latest: Arc<Mutex<serde_json::Value>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl FixtureJournal {
    pub fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fixture journal");
        listener
            .set_nonblocking(true)
            .expect("make fixture journal nonblocking");
        let address = listener.local_addr().expect("read fixture journal address");
        let latest = Arc::new(Mutex::new(serde_json::json!({})));
        let stop = Arc::new(AtomicBool::new(false));
        let latest_for_worker = Arc::clone(&latest);
        let stop_for_worker = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            while !stop_for_worker.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                        let mut request = Vec::new();
                        let mut chunk = [0u8; 8192];
                        let mut body_range = None;
                        loop {
                            match stream.read(&mut chunk) {
                                Ok(0) => break,
                                Ok(n) => request.extend_from_slice(&chunk[..n]),
                                Err(error)
                                    if matches!(
                                        error.kind(),
                                        std::io::ErrorKind::WouldBlock
                                            | std::io::ErrorKind::TimedOut
                                    ) =>
                                {
                                    break;
                                }
                                Err(_) => break,
                            }
                            if body_range.is_none() {
                                body_range = request
                                    .windows(4)
                                    .position(|window| window == b"\r\n\r\n")
                                    .map(|header_end| {
                                        let headers =
                                            String::from_utf8_lossy(&request[..header_end]);
                                        let content_len = headers
                                            .lines()
                                            .find_map(|line| {
                                                line.split_once(':').and_then(|(name, value)| {
                                                    name.eq_ignore_ascii_case("content-length")
                                                        .then_some(value)
                                                })
                                            })
                                            .and_then(|value| value.trim().parse::<usize>().ok())
                                            .unwrap_or(0);
                                        (header_end + 4, content_len)
                                    });
                            }
                            if let Some((body_start, content_len)) = body_range {
                                if request.len() >= body_start + content_len {
                                    break;
                                }
                            }
                        }
                        if let Some((body_start, content_len)) = body_range {
                            if let Some(body) = request.get(body_start..body_start + content_len) {
                                if let Ok(state) = serde_json::from_slice(body) {
                                    *latest_for_worker.lock().expect("lock fixture journal") =
                                        state;
                                }
                            }
                        }
                        let _ = stream.write_all(
                            b"HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
                        );
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            url: format!("http://{address}/state"),
            latest,
            stop,
            worker: Some(worker),
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn contains(&self, marker: &str) -> bool {
        self.latest
            .lock()
            .expect("lock fixture journal")
            .to_string()
            .contains(marker)
    }

    pub fn text(&self, id: &str) -> Option<String> {
        self.latest
            .lock()
            .expect("lock fixture journal")
            .get(id)
            .and_then(|entry| entry.get("text"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    }

    pub fn snapshot(&self) -> serde_json::Value {
        self.latest.lock().expect("lock fixture journal").clone()
    }
}

impl Drop for FixtureJournal {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FixtureJournal;
    use std::io::Write;
    use std::net::TcpStream;
    use std::time::{Duration, Instant};

    #[test]
    fn receives_fixture_state_over_loopback() {
        let journal = FixtureJournal::start();
        let body = r#"{"status":{"text":"last_action=left_click"}}"#;
        let address = journal
            .url()
            .strip_prefix("http://")
            .and_then(|url| url.strip_suffix("/state"))
            .expect("journal URL shape");
        let mut stream = TcpStream::connect(address).expect("connect fixture journal");
        let request = format!(
            "POST /state HTTP/1.1\r\nHost: {address}\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        stream
            .write_all(request.as_bytes())
            .expect("post fixture state");
        drop(stream);

        let deadline = Instant::now() + Duration::from_secs(1);
        while !journal.contains("last_action=left_click") && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(journal.contains("last_action=left_click"));
    }
}
