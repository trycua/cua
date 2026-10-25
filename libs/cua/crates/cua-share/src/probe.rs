//! One GET against the gateway's own socket: what the daemon's `local`
//! link check and the integration tests use.

#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use http_body_util::{BodyExt as _, Empty};

#[cfg(unix)]
use crate::access::{CALLER_HEADER, Caller};

/// What a probe saw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    /// HTTP status.
    pub status: u16,
    /// Content type, or empty.
    pub content_type: String,
    /// Body bytes read (capped at 1 MiB).
    pub bytes: usize,
}

/// GETs `path_and_query` from the gateway socket at `sock`, as `caller`
/// (none: anonymous).
#[cfg(unix)]
pub async fn probe_unix(
    sock: &Path,
    path_and_query: &str,
    caller: Option<&Caller>,
) -> std::result::Result<Probe, String> {
    let work = async {
        let stream = tokio::net::UnixStream::connect(sock)
            .await
            .map_err(|e| format!("the share gateway is not running ({e})"))?;
        let (mut sender, conn) =
            hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream))
                .await
                .map_err(|e| e.to_string())?;
        tokio::spawn(conn);
        let mut req = http::Request::builder()
            .method("GET")
            .uri(path_and_query)
            .header(http::header::HOST, "share.local")
            .header("x-forwarded-proto", "https");
        if let Some(c) = caller {
            req = req.header(
                CALLER_HEADER,
                serde_json::to_string(c).map_err(|e| e.to_string())?,
            );
        }
        let req = req
            .body(Empty::<bytes::Bytes>::new())
            .map_err(|e| e.to_string())?;
        let resp = sender.send_request(req).await.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let content_type = resp
            .headers()
            .get(http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let mut body = resp.into_body();
        let mut bytes = 0usize;
        while let Some(frame) = body.frame().await {
            match frame {
                Ok(f) => {
                    if let Some(d) = f.data_ref() {
                        bytes += d.len();
                    }
                    if bytes >= 1 << 20 {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        Ok(Probe {
            status,
            content_type,
            bytes,
        })
    };
    tokio::time::timeout(Duration::from_secs(20), work)
        .await
        .map_err(|_| "timed out".to_string())?
}
