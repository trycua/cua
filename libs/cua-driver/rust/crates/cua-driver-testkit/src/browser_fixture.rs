use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
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
    download_url: String,
    latest: Arc<Mutex<serde_json::Value>>,
    observed: Arc<Mutex<Vec<serde_json::Value>>>,
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
        let download_url = format!("http://{address}/download");
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
        let page = Arc::new(page);
        let latest = Arc::new(Mutex::new(serde_json::json!({})));
        let observed = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let latest_for_worker = Arc::clone(&latest);
        let observed_for_worker = Arc::clone(&observed);
        let stop_for_worker = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            while !stop_for_worker.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let page = Arc::clone(&page);
                        let latest = Arc::clone(&latest_for_worker);
                        let observed = Arc::clone(&observed_for_worker);
                        thread::spawn(move || handle_connection(stream, &page, &latest, &observed));
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
            download_url,
            latest,
            observed,
            stop,
            worker: Some(worker),
        }
    }

    pub fn page_url(&self) -> &str {
        &self.page_url
    }

    /// Return the fixture URL through another loopback hostname.
    ///
    /// This is used by browser E2E rows that require Chromium to place an
    /// iframe in a distinct site while retaining a repo-owned loopback oracle.
    pub fn page_url_on_host(&self, host: &str) -> String {
        assert!(
            matches!(host, "localhost" | "127.0.0.1"),
            "fixture host must remain on loopback"
        );
        self.page_url.replacen("127.0.0.1", host, 1)
    }

    pub fn journal_url(&self) -> &str {
        &self.journal_url
    }

    /// Fixed loopback attachment used by browser download E2E rows.
    pub fn download_url(&self) -> &str {
        &self.download_url
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

    pub fn has_observed(&self, marker: &str) -> bool {
        self.observed
            .lock()
            .expect("lock browser fixture history")
            .iter()
            .any(|state| state.to_string().contains(marker))
    }
}

fn handle_connection(
    mut stream: TcpStream,
    page: &[u8],
    latest: &Arc<Mutex<serde_json::Value>>,
    observed: &Arc<Mutex<Vec<serde_json::Value>>>,
) {
    // BSD accept can preserve the listener's nonblocking flag. The fixture
    // handler needs a bounded blocking read so it does not close before the
    // browser has written its request bytes.
    if stream.set_nonblocking(false).is_err() {
        return;
    }
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
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
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
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_len = headers
                        .lines()
                        .find_map(|line| {
                            line.split_once(':').and_then(|(name, value)| {
                                name.eq_ignore_ascii_case("content-length").then_some(value)
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
        let _ = stream.write_all(page);
    } else if request_line.starts_with("GET /download ") {
        const DOWNLOAD: &[u8] = b"CUA_DRIVER_BROWSER_DOWNLOAD_FIXTURE_V1\n";
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=fixture.txt\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
            DOWNLOAD.len()
        );
        let _ = stream.write_all(headers.as_bytes());
        let _ = stream.write_all(DOWNLOAD);
    } else if request_line.starts_with("POST /state ") {
        if let Some((body_start, content_len)) = body_range {
            if let Some(body) = request.get(body_start..body_start + content_len) {
                if let Ok(state) = serde_json::from_slice::<serde_json::Value>(body) {
                    *latest.lock().expect("lock browser fixture") = state.clone();
                    observed
                        .lock()
                        .expect("lock browser fixture history")
                        .push(state);
                }
            }
        }
        let _ = stream.write_all(
            b"HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
        );
    } else {
        let _ = stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
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
        let fixture_address = address(server.page_url(), "/fixture");
        let mut stream = TcpStream::connect(&fixture_address).expect("connect browser fixture");
        stream
            .write_all(
                format!("GET /fixture HTTP/1.1\r\nHost: {fixture_address}\r\n\r\n").as_bytes(),
            )
            .expect("request browser fixture");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("read browser fixture");
        assert!(response.contains("200 OK"));
        assert!(response.contains("marker"));
        assert!(response.contains(server.journal_url()));

        let download_address = address(server.download_url(), "/download");
        let mut stream =
            TcpStream::connect(&download_address).expect("connect browser download fixture");
        stream
            .write_all(
                format!("GET /download HTTP/1.1\r\nHost: {download_address}\r\n\r\n").as_bytes(),
            )
            .expect("request browser download fixture");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("read browser download fixture");
        assert!(response.contains("Content-Disposition: attachment"));
        assert!(response.contains("CUA_DRIVER_BROWSER_DOWNLOAD_FIXTURE_V1"));

        let body = r#"{"status":{"text":"last_action=left_click"}}"#;
        let mut stream = TcpStream::connect(&fixture_address).expect("connect browser journal");
        stream
            .write_all(
                format!(
                    "POST /state HTTP/1.1\r\nHost: {fixture_address}\r\nContent-Length: {}\r\n\r\n{body}",
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

    #[test]
    fn idle_preconnection_does_not_block_fixture_requests() {
        let server = BrowserFixtureServer::start("<html><body>concurrent-marker</body></html>");
        let address = address(server.page_url(), "/fixture");
        let _idle = TcpStream::connect(&address).expect("open idle browser preconnection");

        let mut stream = TcpStream::connect(&address).expect("connect active browser request");
        stream
            .set_read_timeout(Some(Duration::from_millis(500)))
            .expect("bound active response wait");
        stream
            .write_all(format!("GET /fixture HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes())
            .expect("request fixture behind idle connection");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("active request must not wait for idle preconnection");

        assert!(response.contains("200 OK"));
        assert!(response.contains("concurrent-marker"));
    }

    #[test]
    fn accepts_request_bytes_after_a_short_client_delay() {
        let server = BrowserFixtureServer::start("<html><body>delayed-marker</body></html>");
        let address = address(server.page_url(), "/fixture");
        let mut stream = TcpStream::connect(&address).expect("connect delayed browser request");

        std::thread::sleep(Duration::from_millis(50));
        stream
            .write_all(format!("GET /fixture HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes())
            .expect("send delayed browser request");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("read delayed browser response");

        assert!(response.contains("200 OK"));
        assert!(response.contains("delayed-marker"));
    }
}
