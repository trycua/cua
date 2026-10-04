// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The S3-compatible backend (AWS S3, Cloudflare R2, MinIO), through the
//! official AWS SDK for Rust.
//!
//! The bucket must have versioning on: history, restore and delete markers
//! are the bucket's own. Preconditions are S3 conditional writes
//! (`If-None-Match: *` and `If-Match: <etag>`); a conditional delete
//! compares the etag first and then deletes, which is exact on a single
//! writer and best effort under a race (S3 has no general conditional
//! delete for every provider).
//!
//! MinIO cannot hold an object and a folder of the same name (`a` and
//! `a/b`); AWS S3 and R2 can. The drive layout never needs both.
//!
//! Credentials come from a [`CredentialSource`]: static keys for MinIO or a
//! bring-your-own bucket, or [`CloudVendor`], which asks the Cua cloud for
//! short-lived keys scoped to the session's folders (trycua/cloud: STS
//! AssumeRole with a session policy per prefix set, 15 to 60 minutes, at
//! most 12 prefixes). The cloud vendor is off unless it is configured
//! explicitly (see [`crate::config`]); the server answers `drive_disabled`
//! (404) while its flag is off and `drive_unavailable` (503) while it has no
//! bucket.

use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use aws_credential_types::Credentials;
use aws_credential_types::provider::{ProvideCredentials, error::CredentialsError, future};
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{BehaviorVersion, Region};
use aws_sdk_s3::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_s3::primitives::ByteStream;
use serde::{Deserialize, Serialize};

use crate::backend::{Backend, Condition, ObjectMeta, VersionInfo};
use crate::{Error, Result, now_ms};

/// Objects listed at most by one `list` (a guard against runaway listings).
pub const MAX_LIST: usize = 200_000;
/// Refresh vended credentials this long before they expire.
pub const REFRESH_BEFORE: Duration = Duration::from_secs(120);

/// Where the bucket is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct S3Config {
    /// `https://<account>.r2.cloudflarestorage.com`, `http://127.0.0.1:9000`;
    /// `None` for AWS's own endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// `us-east-1`, `auto` (R2).
    #[serde(default = "default_region")]
    pub region: String,
    pub bucket: String,
    /// A key prefix every drive key lives under (`acct-123/`), or empty.
    #[serde(default)]
    pub root: String,
    /// Path-style addressing (MinIO and most self-hosted stores).
    #[serde(default)]
    pub path_style: bool,
}

fn default_region() -> String {
    "us-east-1".into()
}

/// One set of keys.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct S3Credentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_token: Option<String>,
    /// Unix ms; `None` for long-lived keys.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_ms: Option<u64>,
}

impl std::fmt::Debug for S3Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Credentials")
            .field("access_key_id", &self.access_key_id)
            .field("secret_access_key", &"<redacted>")
            .field(
                "session_token",
                &self.session_token.as_ref().map(|_| "<redacted>"),
            )
            .field("expires_ms", &self.expires_ms)
            .finish()
    }
}

/// Where keys come from.
#[async_trait::async_trait]
pub trait CredentialSource: Send + Sync {
    /// Current keys (refreshed by the source when they near expiry).
    async fn credentials(&self) -> Result<S3Credentials>;
}

/// Fixed keys (MinIO, a bring-your-own bucket).
pub struct StaticCredentials(pub S3Credentials);

#[async_trait::async_trait]
impl CredentialSource for StaticCredentials {
    async fn credentials(&self) -> Result<S3Credentials> {
        Ok(self.0.clone())
    }
}

/// A bearer token for the Cua cloud API.
#[async_trait::async_trait]
pub trait TokenSource: Send + Sync {
    async fn token(&self) -> Result<String>;
}

/// `POST /api/drive/credentials` body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VendRequest {
    /// `user`, `agent:<name>` or `space:<id>`.
    pub principal: String,
    #[serde(default)]
    pub space: Option<String>,
    pub ttl_secs: u64,
}

/// One folder the vended keys reach.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VendedPrefix {
    pub prefix: String,
    /// `r` or `rw`.
    pub mode: String,
}

/// `POST /api/drive/credentials` response.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vended {
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    pub root: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    #[serde(default)]
    pub session_token: Option<String>,
    /// RFC 3339.
    pub expires_at: String,
    #[serde(default)]
    pub prefixes: Vec<VendedPrefix>,
}

impl std::fmt::Debug for Vended {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vended")
            .field("endpoint", &self.endpoint)
            .field("bucket", &self.bucket)
            .field("root", &self.root)
            .field("expires_at", &self.expires_at)
            .field("prefixes", &self.prefixes)
            .finish_non_exhaustive()
    }
}

impl Vended {
    /// The bucket these keys reach.
    pub fn config(&self) -> S3Config {
        S3Config {
            endpoint: Some(self.endpoint.clone()),
            region: self.region.clone(),
            bucket: self.bucket.clone(),
            root: self.root.clone(),
            path_style: false,
        }
    }

    fn credentials(&self) -> Result<S3Credentials> {
        Ok(S3Credentials {
            access_key_id: self.access_key_id.clone(),
            secret_access_key: self.secret_access_key.clone(),
            session_token: self.session_token.clone(),
            expires_ms: Some(parse_rfc3339_ms(&self.expires_at)?),
        })
    }
}

/// Parses `2026-09-29T12:00:00Z` (or with a fraction / offset) to Unix ms.
pub fn parse_rfc3339_ms(s: &str) -> Result<u64> {
    let dt = aws_smithy_types::DateTime::from_str(s, aws_smithy_types::date_time::Format::DateTime)
        .map_err(|e| Error::Backend(format!("expires_at {s:?}: {e}")))?;
    dt.to_millis()
        .map(|ms| ms.max(0) as u64)
        .map_err(|e| Error::Backend(format!("expires_at {s:?}: {e}")))
}

/// Short-lived keys from the Cua cloud, scoped to one session's folders.
pub struct CloudVendor {
    http: reqwest::Client,
    api: String,
    tokens: Arc<dyn TokenSource>,
    request: VendRequest,
    cache: tokio::sync::Mutex<Option<Vended>>,
}

impl CloudVendor {
    /// A vendor for `request` against the API at `api` (the cyclops-cs base
    /// URL the Fleet client uses).
    pub fn new(
        api: &str,
        tokens: Arc<dyn TokenSource>,
        request: VendRequest,
    ) -> Result<CloudVendor> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| Error::Backend(format!("http client: {e}")))?;
        Ok(CloudVendor {
            http,
            api: api.trim_end_matches('/').to_string(),
            tokens,
            request,
            cache: tokio::sync::Mutex::new(None),
        })
    }

    /// The URL keys are vended from.
    pub fn url(&self) -> String {
        format!("{}/api/drive/credentials", self.api)
    }

    /// Asks the cloud for fresh keys.
    pub async fn vend(&self) -> Result<Vended> {
        let token = self.tokens.token().await?;
        let resp = self
            .http
            .post(self.url())
            .bearer_auth(token)
            .json(&self.request)
            .send()
            .await
            .map_err(|e| Error::Backend(format!("drive credentials: {e}")))?;
        let status = resp.status();
        let body = resp
            .bytes()
            .await
            .map_err(|e| Error::Backend(format!("drive credentials: {e}")))?;
        if status.is_success() {
            return serde_json::from_slice(&body)
                .map_err(|e| Error::Backend(format!("drive credentials: {e}")));
        }
        let kind = serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v["error"].as_str().map(str::to_string))
            .unwrap_or_default();
        Err(match (status.as_u16(), kind.as_str()) {
            (404, "drive_disabled") => Error::Forbidden(
                "Cua Volume is not enabled for this account in the cloud (drive_disabled)".into(),
            ),
            (503, "drive_unavailable") => Error::Backend(
                "Cua Volume is enabled but the cloud has no storage configured yet (drive_unavailable)"
                    .into(),
            ),
            (422, _) => Error::Invalid(format!(
                "drive credentials: the session reaches too many folders for one key ({kind})"
            )),
            (401 | 403, _) => Error::Forbidden(format!("drive credentials refused ({status})")),
            _ => Error::Backend(format!("drive credentials: HTTP {status} {kind}")),
        })
    }

    /// The cached keys, refreshed when they are within [`REFRESH_BEFORE`]
    /// of expiring.
    pub async fn current(&self) -> Result<Vended> {
        let mut c = self.cache.lock().await;
        if let Some(v) = c.as_ref()
            && let Ok(exp) = parse_rfc3339_ms(&v.expires_at)
            && exp > now_ms() + REFRESH_BEFORE.as_millis() as u64
        {
            return Ok(v.clone());
        }
        let v = self.vend().await?;
        *c = Some(v.clone());
        Ok(v)
    }
}

#[async_trait::async_trait]
impl CredentialSource for CloudVendor {
    async fn credentials(&self) -> Result<S3Credentials> {
        self.current().await?.credentials()
    }
}

/// Adapts a [`CredentialSource`] to the SDK's provider (whose identity
/// cache calls it again before an expiring key lapses).
#[derive(Clone)]
struct Provider(Arc<dyn CredentialSource>);

impl std::fmt::Debug for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cua-volume credentials")
    }
}

impl ProvideCredentials for Provider {
    fn provide_credentials<'a>(&'a self) -> future::ProvideCredentials<'a>
    where
        Self: 'a,
    {
        future::ProvideCredentials::new(async move {
            let c = self
                .0
                .credentials()
                .await
                .map_err(CredentialsError::provider_error)?;
            Ok(Credentials::new(
                c.access_key_id,
                c.secret_access_key,
                c.session_token,
                c.expires_ms
                    .map(|ms| UNIX_EPOCH + Duration::from_millis(ms)),
                "cua-volume",
            ))
        })
    }
}

/// A versioned bucket as a drive backend.
#[derive(Clone)]
pub struct S3Backend {
    client: Client,
    config: S3Config,
}

impl std::fmt::Debug for S3Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Backend")
            .field("config", &self.config)
            .finish()
    }
}

fn http_status<E, R: HttpStatus>(e: &SdkError<E, R>) -> Option<u16> {
    e.raw_response().map(HttpStatus::status_u16)
}

trait HttpStatus {
    fn status_u16(&self) -> u16;
}

impl HttpStatus for aws_sdk_s3::config::http::HttpResponse {
    fn status_u16(&self) -> u16 {
        self.status().as_u16()
    }
}

fn map_err<E: ProvideErrorMetadata + std::error::Error + Send + Sync + 'static>(
    what: &str,
    key: &str,
    e: SdkError<E, aws_sdk_s3::config::http::HttpResponse>,
) -> Error {
    let status = http_status(&e);
    let code = e.as_service_error().and_then(|s| s.code()).unwrap_or("");
    match (status, code) {
        (_, "NoSuchBucket") => Error::NotFound(format!(
            "NoSuchBucket: the bucket does not exist ({what} {key})"
        )),
        (Some(404), _) | (_, "NoSuchKey") | (_, "NotFound") | (_, "NoSuchVersion") => {
            Error::NotFound(key.to_string())
        }
        (Some(412), _) | (_, "PreconditionFailed") => {
            Error::Precondition(format!("{key} changed or already exists"))
        }
        (Some(409), _) | (_, "ConditionalRequestConflict") => {
            Error::Precondition(format!("{key}: a concurrent write won"))
        }
        (Some(401 | 403), _) | (_, "AccessDenied") => Error::Forbidden(format!(
            "{what} {key}: access denied by the bucket{}",
            if code.is_empty() {
                String::new()
            } else {
                format!(" ({code})")
            }
        )),
        _ => Error::Backend(format!(
            "{what} {key}: {}",
            aws_sdk_s3::error::DisplayErrorContext(&e)
        )),
    }
}

fn ms(t: Option<&aws_smithy_types::DateTime>) -> u64 {
    t.and_then(|d| d.to_millis().ok())
        .map(|m| m.max(0) as u64)
        .unwrap_or(0)
}

impl S3Backend {
    /// A backend for the bucket in `config`, signing with `creds`.
    pub fn new(config: S3Config, creds: Arc<dyn CredentialSource>) -> Result<S3Backend> {
        if config.bucket.is_empty() {
            return Err(Error::Invalid("S3 bucket is empty".into()));
        }
        let mut root = config.root.trim_start_matches('/').to_string();
        if !root.is_empty() && !root.ends_with('/') {
            root.push('/');
        }
        let mut b = aws_sdk_s3::config::Builder::new()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(config.region.clone()))
            .force_path_style(config.path_style)
            .credentials_provider(Provider(creds));
        if let Some(e) = &config.endpoint {
            b = b.endpoint_url(e);
        }
        Ok(S3Backend {
            client: Client::from_conf(b.build()),
            config: S3Config { root, ..config },
        })
    }

    /// A backend on the Cua cloud's bucket, with keys vended for
    /// `vendor`'s session (the first vend learns where the bucket is).
    pub async fn cloud(vendor: Arc<CloudVendor>) -> Result<S3Backend> {
        let v = vendor.current().await?;
        S3Backend::new(v.config(), vendor)
    }

    pub fn config(&self) -> &S3Config {
        &self.config
    }

    /// The SDK client (tests create and version the bucket with it).
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Whether the bucket has versioning turned on (history, restore and
    /// conflict copies need it).
    pub async fn versioning_enabled(&self) -> Result<bool> {
        let out = self
            .client
            .get_bucket_versioning()
            .bucket(&self.config.bucket)
            .send()
            .await
            .map_err(|e| map_err("versioning", &self.config.bucket, e))?;
        Ok(matches!(
            out.status(),
            Some(aws_sdk_s3::types::BucketVersioningStatus::Enabled)
        ))
    }

    fn full(&self, key: &str) -> String {
        format!("{}{key}", self.config.root)
    }

    fn strip<'a>(&self, full: &'a str) -> &'a str {
        full.strip_prefix(self.config.root.as_str()).unwrap_or(full)
    }
}

#[async_trait::async_trait]
impl Backend for S3Backend {
    fn kind(&self) -> &'static str {
        "s3"
    }

    async fn put(&self, key: &str, bytes: Vec<u8>, cond: Condition) -> Result<ObjectMeta> {
        let size = bytes.len() as u64;
        let mut req = self
            .client
            .put_object()
            .bucket(&self.config.bucket)
            .key(self.full(key))
            .body(ByteStream::from(bytes));
        match &cond {
            Condition::None => {}
            Condition::IfNoneMatch => req = req.if_none_match("*"),
            Condition::IfMatch(etag) => req = req.if_match(etag),
        }
        let out = req.send().await.map_err(|e| map_err("put", key, e))?;
        Ok(ObjectMeta {
            key: key.to_string(),
            size,
            etag: out.e_tag().unwrap_or_default().to_string(),
            version: out.version_id().unwrap_or_default().to_string(),
            modified_ms: now_ms(),
        })
    }

    async fn get(&self, key: &str, version: Option<&str>) -> Result<(Vec<u8>, ObjectMeta)> {
        let out = self
            .client
            .get_object()
            .bucket(&self.config.bucket)
            .key(self.full(key))
            .set_version_id(version.map(str::to_string))
            .send()
            .await
            .map_err(|e| map_err("get", key, e))?;
        let meta = ObjectMeta {
            key: key.to_string(),
            size: out.content_length().unwrap_or(0).max(0) as u64,
            etag: out.e_tag().unwrap_or_default().to_string(),
            version: out.version_id().unwrap_or_default().to_string(),
            modified_ms: ms(out.last_modified()),
        };
        let bytes = out
            .body
            .collect()
            .await
            .map_err(|e| Error::Backend(format!("get {key}: {e}")))?
            .into_bytes()
            .to_vec();
        Ok((
            bytes.clone(),
            ObjectMeta {
                size: bytes.len() as u64,
                ..meta
            },
        ))
    }

    async fn head(&self, key: &str) -> Result<Option<ObjectMeta>> {
        match self
            .client
            .head_object()
            .bucket(&self.config.bucket)
            .key(self.full(key))
            .send()
            .await
        {
            Ok(out) => Ok(Some(ObjectMeta {
                key: key.to_string(),
                size: out.content_length().unwrap_or(0).max(0) as u64,
                etag: out.e_tag().unwrap_or_default().to_string(),
                version: out.version_id().unwrap_or_default().to_string(),
                modified_ms: ms(out.last_modified()),
            })),
            Err(e) => match map_err("head", key, e) {
                Error::NotFound(_) => Ok(None),
                other => Err(other),
            },
        }
    }

    async fn list(&self, prefix: &str) -> Result<Vec<ObjectMeta>> {
        let mut out = vec![];
        let mut token: Option<String> = None;
        loop {
            let page = self
                .client
                .list_objects_v2()
                .bucket(&self.config.bucket)
                .prefix(self.full(prefix))
                .set_continuation_token(token.take())
                .send()
                .await
                .map_err(|e| map_err("list", prefix, e))?;
            for o in page.contents() {
                let Some(k) = o.key() else { continue };
                out.push(ObjectMeta {
                    key: self.strip(k).to_string(),
                    size: o.size().unwrap_or(0).max(0) as u64,
                    etag: o.e_tag().unwrap_or_default().to_string(),
                    version: String::new(),
                    modified_ms: ms(o.last_modified()),
                });
            }
            if out.len() > MAX_LIST {
                return Err(Error::Backend(format!(
                    "listing {prefix} passed {MAX_LIST} objects; list a narrower folder"
                )));
            }
            match (page.is_truncated(), page.next_continuation_token()) {
                (Some(true), Some(t)) => token = Some(t.to_string()),
                _ => break,
            }
        }
        out.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(out)
    }

    async fn get_range(&self, key: &str, version: &str, offset: u64, len: u64) -> Result<Vec<u8>> {
        if len == 0 {
            return Ok(vec![]);
        }
        let mut req = self
            .client
            .get_object()
            .bucket(&self.config.bucket)
            .key(self.full(key))
            .range(format!("bytes={}-{}", offset, offset + len - 1));
        if !version.is_empty() && version != "null" {
            req = req.version_id(version);
        }
        let out = match req.send().await {
            Ok(o) => o,
            // Past the end of the object.
            Err(e) if http_status(&e) == Some(416) => return Ok(vec![]),
            Err(e) => return Err(map_err("get_range", key, e)),
        };
        Ok(out
            .body
            .collect()
            .await
            .map_err(|e| Error::Backend(format!("get_range {key}: {e}")))?
            .into_bytes()
            .to_vec())
    }

    async fn put_file(
        &self,
        key: &str,
        file: &std::path::Path,
        cond: Condition,
    ) -> Result<ObjectMeta> {
        let size = tokio::fs::metadata(file).await?.len();
        if size <= MULTIPART_THRESHOLD {
            let body = ByteStream::from_path(file)
                .await
                .map_err(|e| Error::Backend(format!("put {key}: {e}")))?;
            let mut req = self
                .client
                .put_object()
                .bucket(&self.config.bucket)
                .key(self.full(key))
                .body(body);
            match &cond {
                Condition::None => {}
                Condition::IfNoneMatch => req = req.if_none_match("*"),
                Condition::IfMatch(etag) => req = req.if_match(etag),
            }
            let out = req.send().await.map_err(|e| map_err("put", key, e))?;
            return Ok(ObjectMeta {
                key: key.to_string(),
                size,
                etag: out.e_tag().unwrap_or_default().to_string(),
                version: out.version_id().unwrap_or_default().to_string(),
                modified_ms: now_ms(),
            });
        }
        self.multipart(key, file, size, cond).await
    }

    async fn list_dir(&self, folder: &str) -> Result<(Vec<ObjectMeta>, Vec<String>)> {
        let mut files = vec![];
        let mut folders = vec![];
        let mut token: Option<String> = None;
        loop {
            let page = self
                .client
                .list_objects_v2()
                .bucket(&self.config.bucket)
                .prefix(self.full(folder))
                .delimiter("/")
                .set_continuation_token(token.take())
                .send()
                .await
                .map_err(|e| map_err("list", folder, e))?;
            for o in page.contents() {
                let Some(k) = o.key() else { continue };
                files.push(ObjectMeta {
                    key: self.strip(k).to_string(),
                    size: o.size().unwrap_or(0).max(0) as u64,
                    etag: o.e_tag().unwrap_or_default().to_string(),
                    version: String::new(),
                    modified_ms: ms(o.last_modified()),
                });
            }
            for p in page.common_prefixes() {
                if let Some(p) = p.prefix() {
                    folders.push(self.strip(p).to_string());
                }
            }
            if files.len() + folders.len() > MAX_LIST {
                return Err(Error::Backend(format!(
                    "folder {folder} passed {MAX_LIST} entries"
                )));
            }
            match (page.is_truncated(), page.next_continuation_token()) {
                (Some(true), Some(t)) => token = Some(t.to_string()),
                _ => break,
            }
        }
        files.sort_by(|a, b| a.key.cmp(&b.key));
        folders.sort();
        Ok((files, folders))
    }

    async fn list_after(
        &self,
        prefix: &str,
        start_after: &str,
        max: usize,
    ) -> Result<Vec<ObjectMeta>> {
        let mut out = vec![];
        let mut token: Option<String> = None;
        while out.len() < max {
            let mut req = self
                .client
                .list_objects_v2()
                .bucket(&self.config.bucket)
                .prefix(self.full(prefix))
                .max_keys((max - out.len()).min(1000) as i32)
                .set_continuation_token(token.take());
            if !start_after.is_empty() {
                req = req.start_after(self.full(start_after));
            }
            let page = req.send().await.map_err(|e| map_err("list", prefix, e))?;
            for o in page.contents() {
                let Some(k) = o.key() else { continue };
                out.push(ObjectMeta {
                    key: self.strip(k).to_string(),
                    size: o.size().unwrap_or(0).max(0) as u64,
                    etag: o.e_tag().unwrap_or_default().to_string(),
                    version: String::new(),
                    modified_ms: ms(o.last_modified()),
                });
            }
            match (page.is_truncated(), page.next_continuation_token()) {
                (Some(true), Some(t)) => token = Some(t.to_string()),
                _ => break,
            }
        }
        Ok(out)
    }

    async fn copy(&self, from: &str, to: &str) -> Result<ObjectMeta> {
        let src = self
            .head(from)
            .await?
            .ok_or_else(|| Error::NotFound(from.to_string()))?;
        let source = format!(
            "{}/{}{}",
            self.config.bucket,
            urlencode_key(&self.full(from)),
            if src.version.is_empty() || src.version == "null" {
                String::new()
            } else {
                format!("?versionId={}", src.version)
            }
        );
        if src.size > COPY_LIMIT {
            // One CopyObject reaches 5 GiB; larger objects go part by part.
            return self.multipart_copy(&source, to, src.size).await;
        }
        let out = self
            .client
            .copy_object()
            .bucket(&self.config.bucket)
            .key(self.full(to))
            .copy_source(source)
            .send()
            .await
            .map_err(|e| map_err("copy", from, e))?;
        Ok(ObjectMeta {
            key: to.to_string(),
            size: src.size,
            etag: out
                .copy_object_result()
                .and_then(|r| r.e_tag())
                .unwrap_or_default()
                .to_string(),
            version: out.version_id().unwrap_or_default().to_string(),
            modified_ms: now_ms(),
        })
    }

    fn remote(&self) -> bool {
        true
    }

    fn identity(&self) -> String {
        format!(
            "s3:{}/{}/{}",
            self.config.endpoint.as_deref().unwrap_or("aws"),
            self.config.bucket,
            self.config.root
        )
    }

    async fn delete(&self, key: &str, cond: Condition) -> Result<()> {
        let current = self.head(key).await?;
        match (&cond, &current) {
            (_, None) if cond != Condition::IfNoneMatch => {
                return Err(match cond {
                    Condition::IfMatch(_) => Error::Precondition(format!("{key} is gone")),
                    _ => Error::NotFound(key.to_string()),
                });
            }
            (Condition::IfMatch(want), Some(m)) if &m.etag != want => {
                return Err(Error::Precondition(format!(
                    "{key} changed since it was read"
                )));
            }
            (Condition::IfNoneMatch, Some(_)) => {
                return Err(Error::Precondition(format!("{key} already exists")));
            }
            _ => {}
        }
        self.client
            .delete_object()
            .bucket(&self.config.bucket)
            .key(self.full(key))
            .send()
            .await
            .map_err(|e| map_err("delete", key, e))?;
        Ok(())
    }

    async fn versions(&self, key: &str) -> Result<Vec<VersionInfo>> {
        let full = self.full(key);
        let mut out: Vec<VersionInfo> = vec![];
        let mut key_marker: Option<String> = None;
        let mut version_marker: Option<String> = None;
        loop {
            let page = self
                .client
                .list_object_versions()
                .bucket(&self.config.bucket)
                .prefix(&full)
                .set_key_marker(key_marker.take())
                .set_version_id_marker(version_marker.take())
                .send()
                .await
                .map_err(|e| map_err("versions", key, e))?;
            for v in page.versions() {
                if v.key() == Some(full.as_str()) {
                    out.push(VersionInfo {
                        version: v.version_id().unwrap_or_default().to_string(),
                        size: v.size().unwrap_or(0).max(0) as u64,
                        modified_ms: ms(v.last_modified()),
                        deleted: false,
                        latest: v.is_latest().unwrap_or(false),
                    });
                }
            }
            for d in page.delete_markers() {
                if d.key() == Some(full.as_str()) {
                    out.push(VersionInfo {
                        version: d.version_id().unwrap_or_default().to_string(),
                        size: 0,
                        modified_ms: ms(d.last_modified()),
                        deleted: true,
                        latest: false,
                    });
                }
            }
            if out.len() > MAX_LIST {
                break;
            }
            match (
                page.is_truncated(),
                page.next_key_marker(),
                page.next_version_id_marker(),
            ) {
                (Some(true), Some(k), v) if k <= full.as_str() => {
                    key_marker = Some(k.to_string());
                    version_marker = v.map(str::to_string);
                }
                _ => break,
            }
        }
        if out.is_empty() {
            return Err(Error::NotFound(key.to_string()));
        }
        // Newest first; the current version wins a tie on the clock.
        out.sort_by(|a, b| {
            b.modified_ms
                .cmp(&a.modified_ms)
                .then(b.latest.cmp(&a.latest))
                .then(a.deleted.cmp(&b.deleted).reverse())
        });
        Ok(out)
    }
}

/// Files larger than this go up as a multipart upload.
pub const MULTIPART_THRESHOLD: u64 = 16 * 1024 * 1024;
/// The size of one part (a 10 GiB file is 640 parts; S3 allows 10,000).
pub const PART_SIZE: u64 = 16 * 1024 * 1024;
/// Parts in flight at once (each streams from the file; none is buffered
/// whole in memory by the drive).
pub const PART_CONCURRENCY: usize = 4;
/// The largest single CopyObject.
const COPY_LIMIT: u64 = 5 * 1024 * 1024 * 1024;

fn urlencode_key(k: &str) -> String {
    let mut out = String::with_capacity(k.len());
    for b in k.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

impl S3Backend {
    /// A multipart upload of `file`, aborted on any failure so no parts are
    /// left behind. The precondition is sent with the completion (S3
    /// conditional writes cover CompleteMultipartUpload).
    async fn multipart(
        &self,
        key: &str,
        file: &std::path::Path,
        size: u64,
        cond: Condition,
    ) -> Result<ObjectMeta> {
        use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart};
        let full = self.full(key);
        let created = self
            .client
            .create_multipart_upload()
            .bucket(&self.config.bucket)
            .key(&full)
            .send()
            .await
            .map_err(|e| map_err("put", key, e))?;
        let upload_id = created
            .upload_id()
            .ok_or_else(|| Error::Backend(format!("put {key}: no upload id")))?
            .to_string();
        let run = async {
            let parts = size.div_ceil(PART_SIZE);
            let sem = Arc::new(tokio::sync::Semaphore::new(PART_CONCURRENCY));
            let mut tasks = tokio::task::JoinSet::new();
            let mut done = vec![];
            for i in 0..parts {
                let permit = sem
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|e| Error::Backend(e.to_string()))?;
                let (client, bucket, full, upload_id, path) = (
                    self.client.clone(),
                    self.config.bucket.clone(),
                    full.clone(),
                    upload_id.clone(),
                    file.to_path_buf(),
                );
                let offset = i * PART_SIZE;
                let len = PART_SIZE.min(size - offset);
                let key = key.to_string();
                tasks.spawn(async move {
                    let _permit = permit;
                    let body = ByteStream::read_from()
                        .path(&path)
                        .offset(offset)
                        .length(aws_sdk_s3::primitives::Length::Exact(len))
                        .build()
                        .await
                        .map_err(|e| Error::Backend(format!("put {key}: {e}")))?;
                    let out = client
                        .upload_part()
                        .bucket(bucket)
                        .key(full)
                        .upload_id(upload_id)
                        .part_number((i + 1) as i32)
                        .content_length(len as i64)
                        .body(body)
                        .send()
                        .await
                        .map_err(|e| map_err("put part", &key, e))?;
                    Ok::<_, Error>(
                        CompletedPart::builder()
                            .part_number((i + 1) as i32)
                            .set_e_tag(out.e_tag().map(str::to_string))
                            .build(),
                    )
                });
                // Keep finished parts; stop starting new ones once one failed.
                while let Some(r) = tasks.try_join_next() {
                    done.push(r.map_err(|e| Error::Backend(e.to_string()))??);
                }
            }
            while let Some(r) = tasks.join_next().await {
                done.push(r.map_err(|e| Error::Backend(e.to_string()))??);
            }
            Ok::<_, Error>(done)
        };
        let result = async {
            let mut parts = run.await?;
            parts.sort_by_key(|p| p.part_number());
            if parts.len() as u64 != size.div_ceil(PART_SIZE) {
                return Err(Error::Backend(format!(
                    "put {key}: {} of {} parts landed",
                    parts.len(),
                    size.div_ceil(PART_SIZE)
                )));
            }
            let mut req = self
                .client
                .complete_multipart_upload()
                .bucket(&self.config.bucket)
                .key(&full)
                .upload_id(&upload_id)
                .multipart_upload(
                    CompletedMultipartUpload::builder()
                        .set_parts(Some(parts))
                        .build(),
                );
            match &cond {
                Condition::None => {}
                Condition::IfNoneMatch => req = req.if_none_match("*"),
                Condition::IfMatch(etag) => req = req.if_match(etag),
            }
            let out = req.send().await.map_err(|e| map_err("put", key, e))?;
            Ok(ObjectMeta {
                key: key.to_string(),
                size,
                etag: out.e_tag().unwrap_or_default().to_string(),
                version: out.version_id().unwrap_or_default().to_string(),
                modified_ms: now_ms(),
            })
        }
        .await;
        if result.is_err() {
            let _ = self
                .client
                .abort_multipart_upload()
                .bucket(&self.config.bucket)
                .key(&full)
                .upload_id(&upload_id)
                .send()
                .await;
        }
        result
    }

    async fn multipart_copy(&self, source: &str, to: &str, size: u64) -> Result<ObjectMeta> {
        use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart};
        let full = self.full(to);
        let created = self
            .client
            .create_multipart_upload()
            .bucket(&self.config.bucket)
            .key(&full)
            .send()
            .await
            .map_err(|e| map_err("copy", to, e))?;
        let upload_id = created.upload_id().unwrap_or_default().to_string();
        let result = async {
            const COPY_PART: u64 = 512 * 1024 * 1024;
            let mut parts = vec![];
            for i in 0..size.div_ceil(COPY_PART) {
                let start = i * COPY_PART;
                let end = (start + COPY_PART).min(size) - 1;
                let out = self
                    .client
                    .upload_part_copy()
                    .bucket(&self.config.bucket)
                    .key(&full)
                    .upload_id(&upload_id)
                    .part_number((i + 1) as i32)
                    .copy_source(source)
                    .copy_source_range(format!("bytes={start}-{end}"))
                    .send()
                    .await
                    .map_err(|e| map_err("copy", to, e))?;
                parts.push(
                    CompletedPart::builder()
                        .part_number((i + 1) as i32)
                        .set_e_tag(
                            out.copy_part_result()
                                .and_then(|r| r.e_tag())
                                .map(str::to_string),
                        )
                        .build(),
                );
            }
            let out = self
                .client
                .complete_multipart_upload()
                .bucket(&self.config.bucket)
                .key(&full)
                .upload_id(&upload_id)
                .multipart_upload(
                    CompletedMultipartUpload::builder()
                        .set_parts(Some(parts))
                        .build(),
                )
                .send()
                .await
                .map_err(|e| map_err("copy", to, e))?;
            Ok(ObjectMeta {
                key: to.to_string(),
                size,
                etag: out.e_tag().unwrap_or_default().to_string(),
                version: out.version_id().unwrap_or_default().to_string(),
                modified_ms: now_ms(),
            })
        }
        .await;
        if result.is_err() {
            let _ = self
                .client
                .abort_multipart_upload()
                .bucket(&self.config.bucket)
                .key(&full)
                .upload_id(&upload_id)
                .send()
                .await;
        }
        result
    }
}

/// The Cua cloud's bucket, opened on first use: the first vend says where
/// the bucket is, so a runtime can be built without network I/O.
pub struct CloudBackend {
    vendor: Arc<CloudVendor>,
    inner: tokio::sync::OnceCell<S3Backend>,
}

impl CloudBackend {
    pub fn new(vendor: Arc<CloudVendor>) -> CloudBackend {
        CloudBackend {
            vendor,
            inner: tokio::sync::OnceCell::new(),
        }
    }

    async fn s3(&self) -> Result<&S3Backend> {
        self.inner
            .get_or_try_init(|| S3Backend::cloud(self.vendor.clone()))
            .await
    }
}

#[async_trait::async_trait]
impl Backend for CloudBackend {
    fn kind(&self) -> &'static str {
        "cloud"
    }
    async fn put(&self, key: &str, bytes: Vec<u8>, cond: Condition) -> Result<ObjectMeta> {
        self.s3().await?.put(key, bytes, cond).await
    }
    async fn get(&self, key: &str, version: Option<&str>) -> Result<(Vec<u8>, ObjectMeta)> {
        self.s3().await?.get(key, version).await
    }
    async fn head(&self, key: &str) -> Result<Option<ObjectMeta>> {
        self.s3().await?.head(key).await
    }
    async fn list(&self, prefix: &str) -> Result<Vec<ObjectMeta>> {
        self.s3().await?.list(prefix).await
    }
    async fn delete(&self, key: &str, cond: Condition) -> Result<()> {
        self.s3().await?.delete(key, cond).await
    }
    async fn versions(&self, key: &str) -> Result<Vec<VersionInfo>> {
        self.s3().await?.versions(key).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_and_vended_shapes() {
        let c: S3Config = serde_json::from_str(r#"{"bucket":"b"}"#).unwrap();
        assert_eq!(c.region, "us-east-1");
        assert!(!c.path_style);
        let v: Vended = serde_json::from_str(
            r#"{"endpoint":"https://x.r2.example","region":"auto","bucket":"cua-volume",
                "root":"acct-1/","access_key_id":"AK","secret_access_key":"SECRETVALUE",
                "session_token":"TOKENVALUE","expires_at":"2026-09-29T12:00:00Z",
                "prefixes":[{"prefix":"public/","mode":"r"}]}"#,
        )
        .unwrap();
        assert_eq!(v.config().root, "acct-1/");
        let dbg = format!("{v:?} {:?}", v.credentials().unwrap());
        assert!(
            !dbg.contains("SECRETVALUE") && !dbg.contains("TOKENVALUE"),
            "{dbg}"
        );
        assert_eq!(
            parse_rfc3339_ms("2026-09-29T12:00:00Z").unwrap(),
            1_790_683_200_000
        );
        assert_eq!(
            parse_rfc3339_ms("2026-09-29T12:00:00.500Z").unwrap(),
            1_790_683_200_500
        );
        assert!(parse_rfc3339_ms("yesterday").is_err());
    }

    #[test]
    fn keys_live_under_the_root() {
        let b = S3Backend::new(
            S3Config {
                endpoint: Some("http://127.0.0.1:9".into()),
                region: "us-east-1".into(),
                bucket: "b".into(),
                root: "/acct-1".into(),
                path_style: true,
            },
            Arc::new(StaticCredentials(S3Credentials {
                access_key_id: "a".into(),
                secret_access_key: "s".into(),
                session_token: None,
                expires_ms: None,
            })),
        )
        .unwrap();
        assert_eq!(b.full("agents/ada/x"), "acct-1/agents/ada/x");
        assert_eq!(b.strip("acct-1/agents/ada/x"), "agents/ada/x");
        assert!(
            S3Backend::new(
                S3Config::default(),
                Arc::new(StaticCredentials(S3Credentials {
                    access_key_id: String::new(),
                    secret_access_key: String::new(),
                    session_token: None,
                    expires_ms: None,
                }))
            )
            .is_err()
        );
    }

    struct Tok;
    #[async_trait::async_trait]
    impl TokenSource for Tok {
        async fn token(&self) -> Result<String> {
            Ok("bearer-x".into())
        }
    }

    /// A one-shot HTTP server answering every request with `status` and
    /// `body`, recording the requests it saw.
    async fn serve(status: u16, body: String) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let seen = Arc::new(std::sync::Mutex::new(vec![]));
        let seen2 = seen.clone();
        tokio::spawn(async move {
            for _ in 0..4 {
                let Ok((mut s, _)) = l.accept().await else {
                    return;
                };
                let mut buf = vec![0u8; 16 * 1024];
                let n = s.read(&mut buf).await.unwrap_or(0);
                seen2
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buf[..n]).into_owned());
                let resp = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(resp.as_bytes()).await;
            }
        });
        (format!("http://{addr}"), seen)
    }

    #[tokio::test]
    async fn the_vendor_speaks_the_contract_and_caches() {
        let body = serde_json::json!({
            "endpoint":"https://x.example","region":"auto","bucket":"b","root":"acct/",
            "access_key_id":"AK","secret_access_key":"SK","session_token":"ST",
            "expires_at":"2999-01-01T00:00:00Z","prefixes":[{"prefix":"agents/ada/","mode":"rw"}]
        })
        .to_string();
        let (api, seen) = serve(200, body).await;
        let v = CloudVendor::new(
            &api,
            Arc::new(Tok),
            VendRequest {
                principal: "agent:ada".into(),
                space: Some("cloud:ada".into()),
                ttl_secs: 900,
            },
        )
        .unwrap();
        let c = v.credentials().await.unwrap();
        assert_eq!(c.session_token.as_deref(), Some("ST"));
        v.credentials().await.unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "cached until near expiry");
        assert!(
            seen[0].starts_with("POST /api/drive/credentials"),
            "{}",
            seen[0]
        );
        assert!(
            seen[0]
                .to_ascii_lowercase()
                .contains("authorization: bearer bearer-x")
        );
        assert!(
            seen[0].contains(r#""principal":"agent:ada""#),
            "{}",
            seen[0]
        );
        assert!(seen[0].contains(r#""ttl_secs":900"#));
    }

    #[tokio::test]
    async fn a_disabled_cloud_says_so() {
        let (api, _) = serve(404, r#"{"error":"drive_disabled"}"#.into()).await;
        let v = CloudVendor::new(
            &api,
            Arc::new(Tok),
            VendRequest {
                principal: "user".into(),
                space: None,
                ttl_secs: 900,
            },
        )
        .unwrap();
        let e = v.vend().await.unwrap_err();
        assert_eq!(e.tag(), "forbidden");
        assert!(e.to_string().contains("drive_disabled"), "{e}");
    }
}
