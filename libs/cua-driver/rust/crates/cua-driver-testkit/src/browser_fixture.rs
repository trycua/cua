use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Loopback HTTP host for the repo-owned browser fixture and its state oracle.
///
/// The page is served over HTTP so standalone browsers exercise an ordinary web
/// origin. Fixture state is POSTed back to `/state` and remains independent of
/// cua-driver responses.
pub struct BrowserFixtureServer {
    page_url: String,
    journal_url: String,
    latest: Arc<Mutex<serde_json::Value>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl BrowserFixtureServer {
    pub fn start(html: &str) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind browser fixture server");
        listener
            .set_nonblocking(true)
            .expect("make browser fixture server nonblocking");
        let address = listener
            .local_addr()
            .expect("read browser fixture server address");
        let page_url = format!("http://{address}/fixture");
        let journal_url = format!("http://{address}/state");
        let journal_script = format!(
            "<script>window.__CUA_E2E_FIXTURE_JOURNAL_URL={};</script>",
            serde_json::to_string(&journal_url).expect("serialize fixture journal URL")
        );
        let page = if let Some(head) = html.find("<head>") {
            let insertion = head + "<head>".len();
            format!(
                "{}{}{}",
                &html[..insertion],
                journal_script,
                &html[insertion..]
            )
        } else {
            format!("{journal_script}{html}")
        }
        .into_bytes();
        let latest = Arc::new(Mutex::new(serde_json::json!({})));
        let stop = Arc::new(AtomicBool::new(false));
        let latest_for_worker = Arc::clone(&latest);
        let stop_for_worker = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            while !stop_for_worker.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
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

                        let request_line = request
                            .split(|byte| *byte == b'\n')
                            .next()
                            .map(String::from_utf8_lossy)
                            .unwrap_or_default();
                        if std::env::var_os("CUA_E2E_BROWSER_SERVER_LOG").is_some() {
                            eprintln!("[browser-fixture] {request_line}");
                        }
                        if request_line.starts_with("GET /fixture ")
                            || request_line.starts_with("GET /fixture?")
                            || request_line.starts_with("GET / ")
                        {
                            let headers = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
                                page.len()
                            );
                            let _ = stream.write_all(headers.as_bytes());
                            let _ = stream.write_all(&page);
                        } else if request_line.starts_with("POST /state ") {
                            if let Some((body_start, content_len)) = body_range {
                                if let Some(body) =
                                    request.get(body_start..body_start + content_len)
                                {
                                    if let Ok(state) = serde_json::from_slice(body) {
                                        *latest_for_worker.lock().expect("lock browser fixture") =
                                            state;
                                    }
                                }
                            }
                            let _ = stream.write_all(
                                b"HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
                            );
                        } else {
                            let _ = stream.write_all(
                                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                            );
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            page_url,
            journal_url,
            latest,
            stop,
            worker: Some(worker),
        }
    }

    pub fn page_url(&self) -> &str {
        &self.page_url
    }

    pub fn journal_url(&self) -> &str {
        &self.journal_url
    }

    pub fn contains(&self, marker: &str) -> bool {
        self.latest
            .lock()
            .expect("lock browser fixture")
            .to_string()
            .contains(marker)
    }

    pub fn text(&self, id: &str) -> Option<String> {
        self.latest
            .lock()
            .expect("lock browser fixture")
            .get(id)
            .and_then(|entry| entry.get("text"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    }

    pub fn value(&self, id: &str) -> Option<String> {
        self.latest
            .lock()
            .expect("lock browser fixture")
            .get(id)
            .and_then(|entry| entry.get("value"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    }

    pub fn snapshot(&self) -> serde_json::Value {
        self.latest.lock().expect("lock browser fixture").clone()
    }
}

impl Drop for BrowserFixtureServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BrowserFixtureServer;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::{Duration, Instant};

    fn address(url: &str, suffix: &str) -> String {
        url.strip_prefix("http://")
            .and_then(|url| url.strip_suffix(suffix))
            .expect("fixture URL shape")
            .to_owned()
    }

    #[test]
    fn serves_fixture_and_receives_external_state() {
        let server = BrowserFixtureServer::start("<html><head></head><body>marker</body></html>");
        let address = address(server.page_url(), "/fixture");
        let mut stream = TcpStream::connect(&address).expect("connect browser fixture");
        stream
            .write_all(format!("GET /fixture HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes())
            .expect("request browser fixture");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("read browser fixture");
        assert!(response.contains("200 OK"));
        assert!(response.contains("marker"));
        assert!(response.contains(server.journal_url()));

        let body = r#"{"status":{"text":"last_action=left_click"}}"#;
        let mut stream = TcpStream::connect(&address).expect("connect browser journal");
        stream
            .write_all(
                format!(
                    "POST /state HTTP/1.1\r\nHost: {address}\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .expect("post browser fixture state");
        drop(stream);

        let deadline = Instant::now() + Duration::from_secs(1);
        while !server.contains("last_action=left_click") && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(server.contains("last_action=left_click"));
    }
}
