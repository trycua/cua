//! Blob storage for the gateway: content-addressed files (mount files,
//! image context files) and upload-by-URL blobs (`BlobCreate` then an
//! HTTP `PUT`, `BlobGet` then an HTTP `GET`), kept under
//! `<dir>/blobs`.

use crate::modal::Modal;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{Method, Request, Response, StatusCode, header};
use md5::Digest as _;
use sha2::Digest as _;
use std::path::PathBuf;
use std::sync::Arc;

pub struct Blobs {
    dir: PathBuf,
}

fn valid_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn valid_id(s: &str) -> bool {
    !s.is_empty() && s.len() < 80 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

impl Blobs {
    pub fn open(dir: PathBuf) -> std::io::Result<Blobs> {
        std::fs::create_dir_all(dir.join("sha256"))?;
        std::fs::create_dir_all(dir.join("id"))?;
        Ok(Blobs { dir })
    }

    fn sha_path(&self, hex: &str) -> PathBuf {
        self.dir.join("sha256").join(hex.to_ascii_lowercase())
    }

    fn id_path(&self, id: &str) -> PathBuf {
        self.dir.join("id").join(id)
    }

    pub fn has_sha(&self, hex: &str) -> bool {
        valid_hex(hex) && self.sha_path(hex).is_file()
    }

    /// Stores `data` by its SHA-256 (verified against `claimed` when given).
    pub fn put_sha(&self, claimed: Option<&str>, data: &[u8]) -> Result<String, String> {
        let hex = hex::encode(sha2::Sha256::digest(data));
        if let Some(c) = claimed
            && !c.eq_ignore_ascii_case(&hex)
        {
            return Err(format!("sha256 mismatch: expected {c}, got {hex}"));
        }
        let p = self.sha_path(&hex);
        if !p.is_file() {
            crate::tls::write_file(&p, data, 0o600).map_err(|e| e.to_string())?;
        }
        Ok(hex)
    }

    pub fn get_sha(&self, hex: &str) -> Option<Vec<u8>> {
        if !valid_hex(hex) {
            return None;
        }
        std::fs::read(self.sha_path(hex)).ok()
    }

    /// Stores an uploaded blob under its id; returns its MD5 (hex).
    pub fn put_id(&self, id: &str, data: &[u8]) -> Result<String, String> {
        if !valid_id(id) {
            return Err("invalid blob id".into());
        }
        crate::tls::write_file(&self.id_path(id), data, 0o600).map_err(|e| e.to_string())?;
        Ok(hex::encode(md5::Md5::digest(data)))
    }

    pub fn get_id(&self, id: &str) -> Option<Vec<u8>> {
        if !valid_id(id) {
            return None;
        }
        std::fs::read(self.id_path(id)).ok()
    }

    pub fn has_id(&self, id: &str) -> bool {
        valid_id(id) && self.id_path(id).is_file()
    }

    /// Bytes of a payload that is inline or in a blob.
    pub fn inline_or_blob(&self, data: &[u8], blob_id: &str) -> Option<Vec<u8>> {
        if !blob_id.is_empty() {
            self.get_id(blob_id)
        } else {
            Some(data.to_vec())
        }
    }
}

/// `PUT /_cua/blob/<id>` stores an upload (answering S3-style: 200 and the
/// body's quoted MD5 as `ETag`, rejecting a `Content-MD5` mismatch);
/// `GET` serves it.
pub async fn serve(
    State(m): State<Arc<Modal>>,
    Path(id): Path<String>,
    req: Request<Body>,
) -> Response<Body> {
    let plain = |status: StatusCode, msg: &str| {
        Response::builder()
            .status(status)
            .body(Body::from(msg.to_string()))
            .expect("response")
    };
    match *req.method() {
        Method::PUT => {
            let md5_b64 = req
                .headers()
                .get("content-md5")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            let bytes = match axum::body::to_bytes(req.into_body(), usize::MAX).await {
                Ok(b) => b,
                Err(e) => return plain(StatusCode::BAD_REQUEST, &e.to_string()),
            };
            use base64::Engine;
            if let Some(want) = md5_b64 {
                let got =
                    base64::engine::general_purpose::STANDARD.encode(md5::Md5::digest(&bytes));
                if got != want {
                    return plain(
                        StatusCode::BAD_REQUEST,
                        "The Content-MD5 you specified did not match what we received.",
                    );
                }
            }
            match m.blobs.put_id(&id, &bytes) {
                Ok(md5) => Response::builder()
                    .status(StatusCode::OK)
                    .header(header::ETAG, format!("\"{md5}\""))
                    .body(Body::empty())
                    .expect("response"),
                Err(e) => plain(StatusCode::BAD_REQUEST, &e),
            }
        }
        Method::GET => match m.blobs.get_id(&id) {
            Some(b) => Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .body(Body::from(b))
                .expect("response"),
            None => plain(StatusCode::NOT_FOUND, "no such blob"),
        },
        _ => plain(StatusCode::METHOD_NOT_ALLOWED, "PUT or GET"),
    }
}

/// `GET /_cua/vol/<token>`: one block of a volume file (the URLs of
/// `VolumeGetFile2`), with the SHA-256 of the body as `Repr-Digest`.
pub async fn serve_volume(
    State(m): State<Arc<Modal>>,
    Path(token): Path<String>,
) -> Response<Body> {
    let plain = |status: StatusCode, msg: &str| {
        Response::builder()
            .status(status)
            .body(Body::from(msg.to_string()))
            .expect("response")
    };
    let claims = match m.signer.verify(&token, "vol") {
        Ok(c) => c,
        Err(why) => return plain(StatusCode::FORBIDDEN, why),
    };
    let (Some(vid), Some(path), Some(lo), Some(hi), Some(key)) = (
        claims["sub"].as_str(),
        claims["path"].as_str(),
        claims["lo"].as_u64(),
        claims["hi"].as_u64(),
        claims["key"].as_str(),
    ) else {
        return plain(StatusCode::BAD_REQUEST, "bad token");
    };
    let Some(vol) = m.db.read(|d| d.volumes.get(vid).cloned()) else {
        return plain(StatusCode::NOT_FOUND, "no such volume");
    };
    let who = m.identity_of_key(key).await;
    let cache_key = format!("{vid}/{path}");
    let data = {
        let mut cache = m.live.vol_cache.lock().await;
        match cache.as_ref() {
            Some((k, d, at))
                if *k == cache_key && at.elapsed() < std::time::Duration::from_secs(60) =>
            {
                d.clone()
            }
            _ => match m
                .backend
                .volume_read(&who, &crate::modal::Modal::vol_key(&vol, path))
                .await
            {
                Ok(d) => {
                    let d = Arc::new(d);
                    *cache = Some((cache_key, d.clone(), tokio::time::Instant::now()));
                    d
                }
                Err(e) => return plain(StatusCode::NOT_FOUND, &e.to_string()),
            },
        }
    };
    let (lo, hi) = (lo as usize, (hi as usize).min(data.len()));
    let body = if lo < hi { &data[lo..hi] } else { &[][..] };
    use base64::Engine;
    let digest = base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(body));
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header("Repr-Digest", format!("sha-256=:{digest}:"))
        .body(Body::from(body.to_vec()))
        .expect("response")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_addressed_files_round_trip_and_verify() {
        let d = tempfile::tempdir().unwrap();
        let b = Blobs::open(d.path().to_path_buf()).unwrap();
        let sha = b.put_sha(None, b"hello").unwrap();
        assert_eq!(sha, hex::encode(sha2::Sha256::digest(b"hello")));
        assert!(b.has_sha(&sha));
        assert_eq!(b.get_sha(&sha).unwrap(), b"hello");
        assert!(b.put_sha(Some("00"), b"x").is_err());
        assert!(b.get_sha("../../etc/passwd").is_none());
        assert!(!b.has_sha("zz"));
    }

    #[test]
    fn upload_ids_cannot_escape() {
        let d = tempfile::tempdir().unwrap();
        let b = Blobs::open(d.path().to_path_buf()).unwrap();
        assert!(b.put_id("../x", b"a").is_err());
        let md5 = b.put_id("bl-abc", b"abc").unwrap();
        assert_eq!(md5, "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(b.get_id("bl-abc").unwrap(), b"abc");
    }
}
