//! The object storage Daytona's `Image` builder uploads build contexts to.
//!
//! `ObjectStorageApi.get_push_access()` (`GET /object-storage/push-access`)
//! hands the SDK S3 credentials and an endpoint; both SDKs then `HEAD` or
//! list `<org>/<hash>/context.tar` and, when it is missing, upload it
//! (Python: `obstore` single or multipart `PUT`; TypeScript:
//! `@aws-sdk/lib-storage` multipart, `ListObjectsV2` first). This is the
//! small S3 subset those calls use, path-style, on the gateway itself, over
//! files under the gateway directory. Requests need the issued access key;
//! signatures are not verified (the listener is loopback by default and the
//! credentials are random per gateway).

use super::{Daytona, error};
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::routing::any;
use futures_util::StreamExt;
use http::{HeaderMap, Method, Response, StatusCode, header};
use md5::{Digest, Md5};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

pub const BUCKET: &str = "daytona-volume-builds";
pub const ORG: &str = "cua";
/// The largest object accepted (a build context).
const MAX_OBJECT: u64 = 4 << 30;

pub struct ObjectStore {
    root: PathBuf,
    pub access_key: String,
    pub secret: String,
    uploads: Mutex<HashMap<String, Upload>>,
}

struct Upload {
    key: String,
    parts: BTreeMap<u32, PathBuf>,
}

impl ObjectStore {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            access_key: format!("CUA{}", crate::auth::new_token()[..16].to_uppercase()),
            secret: crate::auth::new_token(),
            uploads: Mutex::default(),
        }
    }

    fn object_path(&self, key: &str) -> Option<PathBuf> {
        if key.is_empty()
            || key
                .split('/')
                .any(|c| c.is_empty() || c == ".." || c == ".")
        {
            return None;
        }
        Some(self.root.join("objects").join(BUCKET).join(key))
    }

    pub fn stage_dir(&self) -> PathBuf {
        self.root.join("stage")
    }

    /// The extracted contexts for `hashes`, each in its own directory
    /// (extracted once). A hash the client never uploaded is an error.
    pub async fn context_roots(&self, hashes: &[String]) -> Result<Vec<PathBuf>, String> {
        let mut out = Vec::new();
        for h in hashes {
            if h.is_empty() || !h.chars().all(|c| c.is_ascii_alphanumeric()) {
                return Err(format!("invalid context hash {h:?}"));
            }
            let dir = self.root.join("contexts").join(h);
            if !dir.join(".done").exists() {
                let tarball = self
                    .object_path(&format!("{ORG}/{h}/context.tar"))
                    .filter(|p| p.is_file())
                    .ok_or_else(|| format!("build context {h} was not uploaded"))?;
                let dir2 = dir.clone();
                tokio::task::spawn_blocking(move || extract(&tarball, &dir2))
                    .await
                    .map_err(|e| e.to_string())??;
            }
            out.push(dir);
        }
        Ok(out)
    }
}

fn extract(tarball: &FsPath, dir: &FsPath) -> Result<(), String> {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let f = std::fs::File::open(tarball).map_err(|e| e.to_string())?;
    let mut ar = tar::Archive::new(f);
    ar.set_preserve_permissions(true);
    ar.set_unpack_xattrs(false);
    // `Archive::unpack` refuses entries that escape `dir` (absolute paths,
    // `..`); links are unpacked as links and checked at COPY time.
    ar.unpack(dir)
        .map_err(|e| format!("unpacking the build context: {e}"))?;
    std::fs::write(dir.join(".done"), b"").map_err(|e| e.to_string())
}

pub fn routes(d: Arc<Daytona>) -> Router {
    Router::new()
        .route(
            "/api/object-storage/push-access",
            axum::routing::get(push_access),
        )
        .route(&format!("/{BUCKET}"), any(bucket))
        // path-style clients (the AWS SDK) put a slash after the bucket
        .route(&format!("/{BUCKET}/"), any(bucket))
        .route(&format!("/{BUCKET}/{{*key}}"), any(object))
        .with_state(d)
}

/// `GET /object-storage/push-access` (`StorageAccessDto`).
async fn push_access(State(d): State<Arc<Daytona>>, h: HeaderMap) -> Response<Body> {
    if d.caller(&h).await.is_none() {
        return error(StatusCode::UNAUTHORIZED, "Invalid credentials");
    }
    super::ok_json(serde_json::json!({
        "accessKey": d.objects.access_key,
        "secret": d.objects.secret,
        "sessionToken": "cua",
        "storageUrl": d.cfg.object_url(),
        "organizationId": ORG,
        "bucket": BUCKET,
        "region": "us-east-1",
    }))
}

fn s3_error(status: StatusCode, code: &str, msg: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/xml")
        .body(Body::from(format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Error><Code>{code}</Code><Message>{msg}</Message></Error>"
        )))
        .expect("response")
}

fn xml(body: String) -> Response<Body> {
    Response::builder()
        .header(header::CONTENT_TYPE, "application/xml")
        .body(Body::from(format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>{body}"
        )))
        .expect("response")
}

/// The access key id in `Authorization: AWS4-HMAC-SHA256 Credential=<id>/...`.
fn access_key(h: &HeaderMap) -> Option<String> {
    let a = h.get(header::AUTHORIZATION)?.to_str().ok()?;
    let c = a.split("Credential=").nth(1)?;
    Some(c.split('/').next()?.to_string())
}

fn authorized(d: &Daytona, h: &HeaderMap) -> bool {
    access_key(h).is_some_and(|k| crate::auth::same(&k, &d.objects.access_key))
}

async fn bucket(
    State(d): State<Arc<Daytona>>,
    method: Method,
    h: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response<Body> {
    if !authorized(&d, &h) {
        return s3_error(StatusCode::FORBIDDEN, "AccessDenied", "Access Denied");
    }
    if method != Method::GET {
        return s3_error(
            StatusCode::METHOD_NOT_ALLOWED,
            "MethodNotAllowed",
            "method not allowed",
        );
    }
    let prefix = q.get("prefix").cloned().unwrap_or_default();
    let max: usize = q
        .get("max-keys")
        .and_then(|m| m.parse().ok())
        .unwrap_or(1000);
    let base = d.objects.root.join("objects").join(BUCKET);
    let mut keys = Vec::new();
    collect(&base, &base, &mut keys);
    keys.sort();
    let matching: Vec<&String> = keys.iter().filter(|k| k.starts_with(&prefix)).collect();
    let shown: Vec<&&String> = matching.iter().take(max).collect();
    let contents: String = shown
        .iter()
        .map(|k| {
            let len = std::fs::metadata(base.join(k.as_str())).map(|m| m.len()).unwrap_or(0);
            format!("<Contents><Key>{k}</Key><Size>{len}</Size><StorageClass>STANDARD</StorageClass></Contents>")
        })
        .collect();
    xml(format!(
        "<ListBucketResult><Name>{BUCKET}</Name><Prefix>{prefix}</Prefix><KeyCount>{}</KeyCount><MaxKeys>{max}</MaxKeys><IsTruncated>{}</IsTruncated>{contents}</ListBucketResult>",
        shown.len(),
        matching.len() > max
    ))
}

fn collect(base: &FsPath, dir: &FsPath, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(base, &p, out);
        } else if let Ok(r) = p.strip_prefix(base) {
            out.push(r.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// Decodes an `aws-chunked` body (`<hex>[;ext]\r\n<data>\r\n ... 0\r\n<trailers>\r\n\r\n`).
pub(crate) fn dechunk_aws(raw: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        let eol = raw[i..]
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or("truncated aws-chunked body")?;
        let head = std::str::from_utf8(&raw[i..i + eol]).map_err(|_| "bad chunk header")?;
        let size = usize::from_str_radix(head.split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| "bad chunk size")?;
        i += eol + 2;
        if size == 0 {
            return Ok(out);
        }
        if raw.len() < i + size + 2 {
            return Err("truncated aws-chunked body".into());
        }
        out.extend_from_slice(&raw[i..i + size]);
        i += size + 2;
    }
}

async fn write_body(
    body: Body,
    h: &HeaderMap,
    path: &FsPath,
) -> Result<(u64, String), Response<Body>> {
    let chunked = h
        .get("content-encoding")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("aws-chunked"))
        || h.get("x-amz-content-sha256")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("STREAMING-"));
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            s3_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalError",
                &e.to_string(),
            )
        })?;
    }
    let tmp = path.with_extension("partial");
    let internal = |e: String| s3_error(StatusCode::INTERNAL_SERVER_ERROR, "InternalError", &e);
    let mut hasher = Md5::new();
    let mut total = 0u64;
    if chunked {
        // Small bodies only (the SDKs send parts of at most 5 MiB per
        // request here): decode in memory.
        let raw = http_body_util::BodyExt::collect(http_body_util::Limited::new(body, 64 << 20))
            .await
            .map_err(|e| internal(e.to_string()))?
            .to_bytes();
        let data = dechunk_aws(&raw)
            .map_err(|e| s3_error(StatusCode::BAD_REQUEST, "InvalidRequest", &e))?;
        hasher.update(&data);
        total = data.len() as u64;
        tokio::fs::write(&tmp, &data)
            .await
            .map_err(|e| internal(e.to_string()))?;
    } else {
        let mut f = tokio::fs::File::create(&tmp)
            .await
            .map_err(|e| internal(e.to_string()))?;
        let mut s = body.into_data_stream();
        while let Some(c) = s.next().await {
            let c = c.map_err(|e| internal(e.to_string()))?;
            total += c.len() as u64;
            if total > MAX_OBJECT {
                let _ = tokio::fs::remove_file(&tmp).await;
                return Err(s3_error(
                    StatusCode::BAD_REQUEST,
                    "EntityTooLarge",
                    "object too large",
                ));
            }
            hasher.update(&c);
            f.write_all(&c).await.map_err(|e| internal(e.to_string()))?;
        }
        f.flush().await.map_err(|e| internal(e.to_string()))?;
    }
    tokio::fs::rename(&tmp, path)
        .await
        .map_err(|e| internal(e.to_string()))?;
    Ok((total, hex::encode(hasher.finalize())))
}

async fn object(
    State(d): State<Arc<Daytona>>,
    Path(key): Path<String>,
    method: Method,
    h: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
    body: Body,
) -> Response<Body> {
    if !authorized(&d, &h) {
        return s3_error(StatusCode::FORBIDDEN, "AccessDenied", "Access Denied");
    }
    let Some(path) = d.objects.object_path(&key) else {
        return s3_error(StatusCode::BAD_REQUEST, "InvalidArgument", "invalid key");
    };
    let upload_id = q.get("uploadId").cloned();
    match (
        method.clone(),
        upload_id,
        q.get("partNumber"),
        q.contains_key("uploads"),
    ) {
        (Method::POST, None, _, true) => {
            let id = crate::auth::new_token();
            d.objects.uploads.lock().await.insert(
                id.clone(),
                Upload {
                    key: key.clone(),
                    parts: BTreeMap::new(),
                },
            );
            xml(format!(
                "<InitiateMultipartUploadResult><Bucket>{BUCKET}</Bucket><Key>{key}</Key><UploadId>{id}</UploadId></InitiateMultipartUploadResult>"
            ))
        }
        (Method::PUT, Some(id), Some(n), _) => {
            let Ok(n) = n.parse::<u32>() else {
                return s3_error(StatusCode::BAD_REQUEST, "InvalidArgument", "bad partNumber");
            };
            if !d
                .objects
                .uploads
                .lock()
                .await
                .get(&id)
                .is_some_and(|u| u.key == key)
            {
                return s3_error(StatusCode::NOT_FOUND, "NoSuchUpload", "no such upload");
            }
            let part = d
                .objects
                .root
                .join("uploads")
                .join(&id)
                .join(format!("{n:05}"));
            match write_body(body, &h, &part).await {
                Ok((_, etag)) => {
                    if let Some(u) = d.objects.uploads.lock().await.get_mut(&id) {
                        u.parts.insert(n, part);
                    }
                    Response::builder()
                        .header(header::ETAG, format!("\"{etag}\""))
                        .body(Body::empty())
                        .expect("response")
                }
                Err(r) => r,
            }
        }
        (Method::POST, Some(id), _, _) => {
            let up = d.objects.uploads.lock().await.remove(&id);
            let Some(up) = up.filter(|u| u.key == key) else {
                return s3_error(StatusCode::NOT_FOUND, "NoSuchUpload", "no such upload");
            };
            if let Some(parent) = path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            let tmp = path.with_extension("partial");
            let res: Result<String, std::io::Error> = async {
                let mut out = tokio::fs::File::create(&tmp).await?;
                let mut hasher = Md5::new();
                for p in up.parts.values() {
                    let data = tokio::fs::read(p).await?;
                    hasher.update(&data);
                    out.write_all(&data).await?;
                }
                out.flush().await?;
                tokio::fs::rename(&tmp, &path).await?;
                Ok(hex::encode(hasher.finalize()))
            }
            .await;
            let _ = tokio::fs::remove_dir_all(d.objects.root.join("uploads").join(&id)).await;
            match res {
                Ok(etag) => xml(format!(
                    "<CompleteMultipartUploadResult><Location>{}/{BUCKET}/{key}</Location><Bucket>{BUCKET}</Bucket><Key>{key}</Key><ETag>\"{etag}-{}\"</ETag></CompleteMultipartUploadResult>",
                    d.cfg.object_url(),
                    up.parts.len()
                )),
                Err(e) => s3_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "InternalError",
                    &e.to_string(),
                ),
            }
        }
        (Method::DELETE, Some(id), _, _) => {
            d.objects.uploads.lock().await.remove(&id);
            let _ = tokio::fs::remove_dir_all(d.objects.root.join("uploads").join(&id)).await;
            Response::builder()
                .status(StatusCode::NO_CONTENT)
                .body(Body::empty())
                .expect("response")
        }
        (Method::PUT, None, None, _) => match write_body(body, &h, &path).await {
            Ok((_, etag)) => Response::builder()
                .header(header::ETAG, format!("\"{etag}\""))
                .body(Body::empty())
                .expect("response"),
            Err(r) => r,
        },
        (Method::HEAD, ..) | (Method::GET, ..) => match tokio::fs::metadata(&path).await {
            Ok(m) if m.is_file() => {
                let b = if method == Method::GET {
                    match tokio::fs::read(&path).await {
                        Ok(d) => Body::from(d),
                        Err(_) => Body::empty(),
                    }
                } else {
                    Body::empty()
                };
                Response::builder()
                    .header(header::CONTENT_LENGTH, m.len())
                    .header(header::CONTENT_TYPE, "application/octet-stream")
                    .body(b)
                    .expect("response")
            }
            _ => {
                if method == Method::HEAD {
                    Response::builder()
                        .status(StatusCode::NOT_FOUND)
                        .body(Body::empty())
                        .expect("response")
                } else {
                    s3_error(
                        StatusCode::NOT_FOUND,
                        "NoSuchKey",
                        "The specified key does not exist.",
                    )
                }
            }
        },
        (Method::DELETE, None, ..) => {
            let _ = tokio::fs::remove_file(&path).await;
            Response::builder()
                .status(StatusCode::NO_CONTENT)
                .body(Body::empty())
                .expect("response")
        }
        _ => s3_error(
            StatusCode::METHOD_NOT_ALLOWED,
            "MethodNotAllowed",
            "method not allowed",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn aws_chunked_bodies_decode() {
        let body = b"5\r\nhello\r\n6;chunk-signature=abc\r\n world\r\n0\r\nx-amz-checksum-crc32:AAAA\r\n\r\n";
        assert_eq!(dechunk_aws(body).unwrap(), b"hello world");
        assert!(dechunk_aws(b"5\r\nhel").is_err());
        assert!(dechunk_aws(b"zz\r\n").is_err());
    }

    #[test]
    fn keys_cannot_escape_the_store() {
        let s = ObjectStore::new(PathBuf::from("/x"));
        assert!(s.object_path("cua/abc/context.tar").is_some());
        for bad in ["", "../a", "a/../b", "a//b", "/a", "a/./b"] {
            assert!(s.object_path(bad).is_none(), "{bad:?}");
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn dechunk_never_panics(b in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..200)) {
            let _ = dechunk_aws(&b);
        }

        #[test]
        fn dechunk_roundtrips(chunks in proptest::collection::vec(proptest::collection::vec(proptest::prelude::any::<u8>(), 1..40), 0..6)) {
            let mut enc = Vec::new();
            let mut want = Vec::new();
            for c in &chunks {
                enc.extend_from_slice(format!("{:x}\r\n", c.len()).as_bytes());
                enc.extend_from_slice(c);
                enc.extend_from_slice(b"\r\n");
                want.extend_from_slice(c);
            }
            enc.extend_from_slice(b"0\r\n\r\n");
            prop_assert_eq!(dechunk_aws(&enc).unwrap(), want);
        }
    }
}
