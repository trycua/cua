//! The fallback copy of snapshots, for when the host that has a snapshot
//! cannot be reached (asleep, off the LAN): in your S3 bucket, or in a
//! folder every host mounts (your Cua Volume, a NAS share). The bake host
//! publishes it once; every other host fetches the files it lacks from
//! there, resuming and verifying them like a LAN copy.
//!
//! Layout under the prefix:
//!
//! ```text
//! blobs/<sha256>        files, by content (uploaded once)
//! manifests/<digest>    manifests
//! names/<name>          the digest of the latest published snapshot of a name
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _, AsyncWriteExt as _};

use crate::pull::BlobSource;
use crate::store::Store;
use crate::{Error, Manifest, Progress, ProgressFn, Result, valid_sha};

/// Bytes read per range request.
const CHUNK: u64 = 8 << 20;

/// A flat object store: what the fallback needs of S3 or a folder.
#[async_trait::async_trait]
pub trait ObjectStore: Send + Sync {
    /// The size of `key`, or `None`.
    async fn head(&self, key: &str) -> Result<Option<u64>>;
    /// The whole (small) object.
    async fn get(&self, key: &str) -> Result<Vec<u8>>;
    /// Bytes `[offset, offset + len)` (short at the end).
    async fn get_range(&self, key: &str, offset: u64, len: u64) -> Result<Vec<u8>>;
    /// Writes a small object.
    async fn put(&self, key: &str, bytes: Vec<u8>) -> Result<()>;
    /// Writes `key` from a local file without holding it in memory.
    async fn put_file(&self, key: &str, file: &Path) -> Result<()>;
}

/// A folder (a mounted Cua Volume, a NAS share): keys are relative paths.
pub struct DirStore {
    root: PathBuf,
}

impl DirStore {
    /// A store in `root` (a leading `~` is this user's home).
    pub fn new(root: &str) -> DirStore {
        let root = match root
            .strip_prefix("~/")
            .or(if root == "~" { Some("") } else { None })
        {
            Some(rest) => std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default()
                .join(rest),
            None => PathBuf::from(root),
        };
        DirStore { root }
    }

    fn path(&self, key: &str) -> Result<PathBuf> {
        if key
            .split('/')
            .any(|c| c.is_empty() || c == "." || c == "..")
        {
            return Err(Error::Invalid(format!("bad key {key:?}")));
        }
        Ok(self.root.join(key))
    }
}

#[async_trait::async_trait]
impl ObjectStore for DirStore {
    async fn head(&self, key: &str) -> Result<Option<u64>> {
        match tokio::fs::metadata(self.path(key)?).await {
            Ok(m) => Ok(Some(m.len())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(Error::Unavailable(format!("{}: {e}", self.root.display()))),
        }
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>> {
        tokio::fs::read(self.path(key)?).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::NotFound(format!("{key} in {}", self.root.display()))
            } else {
                Error::Unavailable(format!("{}: {e}", self.root.display()))
            }
        })
    }

    async fn get_range(&self, key: &str, offset: u64, len: u64) -> Result<Vec<u8>> {
        let mut f = tokio::fs::File::open(self.path(key)?)
            .await
            .map_err(|e| Error::Unavailable(format!("{}: {e}", self.root.display())))?;
        f.seek(std::io::SeekFrom::Start(offset)).await?;
        let mut out = Vec::with_capacity(len as usize);
        f.take(len).read_to_end(&mut out).await?;
        Ok(out)
    }

    async fn put(&self, key: &str, bytes: Vec<u8>) -> Result<()> {
        let p = self.path(key)?;
        if let Some(d) = p.parent() {
            tokio::fs::create_dir_all(d).await?;
        }
        let tmp = p.with_extension(format!("part-{:08x}", rand::random::<u32>()));
        tokio::fs::write(&tmp, bytes).await?;
        tokio::fs::rename(&tmp, &p).await?;
        Ok(())
    }

    async fn put_file(&self, key: &str, file: &Path) -> Result<()> {
        let p = self.path(key)?;
        if let Some(d) = p.parent() {
            tokio::fs::create_dir_all(d).await?;
        }
        let tmp = p.with_extension(format!("part-{:08x}", rand::random::<u32>()));
        tokio::fs::copy(file, &tmp).await?;
        tokio::fs::rename(&tmp, &p).await?;
        Ok(())
    }
}

/// Where an S3 copy lives.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct S3Location {
    /// The bucket.
    pub bucket: String,
    /// Region (empty: the host's AWS default, else `us-east-1`).
    pub region: String,
    /// An S3-compatible endpoint (empty: AWS).
    pub endpoint: String,
}

/// An S3 bucket, with the host's AWS default credentials (environment,
/// profile, SSO, instance role).
pub struct S3Store {
    client: aws_sdk_s3::Client,
    bucket: String,
}

/// Parts of a multipart upload (S3 allows 10,000 parts of at least 5 MiB).
const PART: u64 = 64 << 20;

fn s3err(op: &str, key: &str, e: impl std::fmt::Display) -> Error {
    Error::Unavailable(format!("s3 {op} {key}: {e}"))
}

impl S3Store {
    /// The bucket at `loc`.
    pub async fn new(loc: &S3Location) -> Result<S3Store> {
        if loc.bucket.trim().is_empty() {
            return Err(Error::Invalid("the S3 fallback needs a bucket".into()));
        }
        let mut conf = aws_config::defaults(aws_config::BehaviorVersion::latest());
        if !loc.region.is_empty() {
            conf = conf.region(aws_config::Region::new(loc.region.clone()));
        }
        let conf = conf.load().await;
        let mut b = aws_sdk_s3::config::Builder::from(&conf);
        if conf.region().is_none() {
            b = b.region(aws_sdk_s3::config::Region::new("us-east-1"));
        }
        if !loc.endpoint.is_empty() {
            b = b.endpoint_url(&loc.endpoint).force_path_style(true);
        }
        Ok(S3Store {
            client: aws_sdk_s3::Client::from_conf(b.build()),
            bucket: loc.bucket.clone(),
        })
    }
}

#[async_trait::async_trait]
impl ObjectStore for S3Store {
    async fn head(&self, key: &str) -> Result<Option<u64>> {
        match self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
        {
            Ok(o) => Ok(Some(o.content_length().unwrap_or(0).max(0) as u64)),
            Err(e) => {
                if e.as_service_error().is_some_and(|s| s.is_not_found()) {
                    Ok(None)
                } else {
                    Err(s3err(
                        "head",
                        key,
                        aws_sdk_s3::error::DisplayErrorContext(e),
                    ))
                }
            }
        }
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>> {
        let o = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| {
                if e.as_service_error().is_some_and(|s| s.is_no_such_key()) {
                    Error::NotFound(format!("s3://{}/{key}", self.bucket))
                } else {
                    s3err("get", key, aws_sdk_s3::error::DisplayErrorContext(e))
                }
            })?;
        Ok(o.body
            .collect()
            .await
            .map_err(|e| s3err("get", key, e))?
            .into_bytes()
            .to_vec())
    }

    async fn get_range(&self, key: &str, offset: u64, len: u64) -> Result<Vec<u8>> {
        if len == 0 {
            return Ok(vec![]);
        }
        let o = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .range(format!("bytes={offset}-{}", offset + len - 1))
            .send()
            .await
            .map_err(|e| s3err("get", key, aws_sdk_s3::error::DisplayErrorContext(e)))?;
        Ok(o.body
            .collect()
            .await
            .map_err(|e| s3err("get", key, e))?
            .into_bytes()
            .to_vec())
    }

    async fn put(&self, key: &str, bytes: Vec<u8>) -> Result<()> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(bytes.into())
            .send()
            .await
            .map_err(|e| s3err("put", key, aws_sdk_s3::error::DisplayErrorContext(e)))?;
        Ok(())
    }

    async fn put_file(&self, key: &str, file: &Path) -> Result<()> {
        use aws_sdk_s3::primitives::ByteStream;
        use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart};
        let size = tokio::fs::metadata(file).await?.len();
        if size <= PART {
            let body = ByteStream::from_path(file)
                .await
                .map_err(|e| s3err("put", key, e))?;
            self.client
                .put_object()
                .bucket(&self.bucket)
                .key(key)
                .body(body)
                .send()
                .await
                .map_err(|e| s3err("put", key, aws_sdk_s3::error::DisplayErrorContext(e)))?;
            return Ok(());
        }
        let up = self
            .client
            .create_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| s3err("upload", key, aws_sdk_s3::error::DisplayErrorContext(e)))?;
        let id = up.upload_id().unwrap_or_default().to_string();
        let result: Result<Vec<CompletedPart>> = async {
            let mut parts = Vec::new();
            let mut offset = 0u64;
            let mut n = 1i32;
            while offset < size {
                let len = PART.min(size - offset);
                let body = ByteStream::read_from()
                    .path(file)
                    .offset(offset)
                    .length(aws_sdk_s3::primitives::Length::Exact(len))
                    .build()
                    .await
                    .map_err(|e| s3err("upload", key, e))?;
                let p = self
                    .client
                    .upload_part()
                    .bucket(&self.bucket)
                    .key(key)
                    .upload_id(&id)
                    .part_number(n)
                    .body(body)
                    .send()
                    .await
                    .map_err(|e| s3err("upload", key, aws_sdk_s3::error::DisplayErrorContext(e)))?;
                parts.push(
                    CompletedPart::builder()
                        .part_number(n)
                        .set_e_tag(p.e_tag().map(str::to_string))
                        .build(),
                );
                offset += len;
                n += 1;
            }
            Ok(parts)
        }
        .await;
        match result {
            Ok(parts) => {
                self.client
                    .complete_multipart_upload()
                    .bucket(&self.bucket)
                    .key(key)
                    .upload_id(&id)
                    .multipart_upload(
                        CompletedMultipartUpload::builder()
                            .set_parts(Some(parts))
                            .build(),
                    )
                    .send()
                    .await
                    .map_err(|e| s3err("upload", key, aws_sdk_s3::error::DisplayErrorContext(e)))?;
                Ok(())
            }
            Err(e) => {
                let _ = self
                    .client
                    .abort_multipart_upload()
                    .bucket(&self.bucket)
                    .key(key)
                    .upload_id(&id)
                    .send()
                    .await;
                Err(e)
            }
        }
    }
}

/// The fallback store: an [`ObjectStore`] and a key prefix.
pub struct FallbackStore {
    store: Arc<dyn ObjectStore>,
    prefix: String,
    label: String,
}

fn norm_prefix(p: &str) -> String {
    let p = p.trim().trim_start_matches('/');
    if p.is_empty() || p.ends_with('/') {
        p.to_string()
    } else {
        format!("{p}/")
    }
}

impl FallbackStore {
    /// A store over `store` under `prefix`, reported as `label`.
    pub fn new(store: Arc<dyn ObjectStore>, prefix: &str, label: &str) -> FallbackStore {
        FallbackStore {
            store,
            prefix: norm_prefix(prefix),
            label: label.into(),
        }
    }

    /// A folder every host mounts (`~` is the home), reported as `volume`.
    pub fn dir(path: &str) -> FallbackStore {
        FallbackStore::new(Arc::new(DirStore::new(path)), "", "volume")
    }

    /// An S3 bucket under `prefix`, reported as `s3`.
    pub async fn s3(loc: &S3Location, prefix: &str) -> Result<FallbackStore> {
        Ok(FallbackStore::new(
            Arc::new(S3Store::new(loc).await?),
            prefix,
            "s3",
        ))
    }

    fn blob_key(&self, sha: &str) -> String {
        format!("{}blobs/{sha}", self.prefix)
    }

    /// Uploads `m`'s files (from where this host's export index says they
    /// are) that are not there yet, then its manifest and its name. Returns
    /// the bytes uploaded.
    pub async fn publish(
        &self,
        store: &Store,
        m: &Manifest,
        progress: Option<ProgressFn>,
    ) -> Result<u64> {
        let digest = m.digest();
        let index = store.export_index(&digest)?;
        let mut todo = Vec::new();
        let mut reused = 0;
        for (sha, size) in m.blobs() {
            match self.store.head(&self.blob_key(&sha)).await? {
                Some(s) if s == size => reused += size,
                _ => todo.push((sha, size)),
            }
        }
        let total: u64 = todo.iter().map(|(_, s)| *s).sum();
        let report = |done: u64, fin: bool| {
            if let Some(p) = &progress {
                p(Progress {
                    name: m.name.clone(),
                    digest: digest.clone(),
                    from: self.label.clone(),
                    bytes_done: done,
                    bytes_total: total,
                    bytes_reused: reused,
                    done: fin,
                    verified: fin,
                });
            }
        };
        report(0, false);
        let mut done = 0;
        for (sha, size) in todo {
            let file = index
                .files
                .get(&sha)
                .ok_or_else(|| Error::NotFound(format!("file {sha} of {}", m.name)))?;
            if crate::hash::Stamp::of(&file.path)? != file.stamp {
                return Err(Error::Corrupt(format!(
                    "{} changed since it was exported; export it again",
                    file.path.display()
                )));
            }
            self.store
                .put_file(&self.blob_key(&sha), &file.path)
                .await?;
            done += size;
            report(done, false);
        }
        self.store
            .put(&format!("{}manifests/{digest}", self.prefix), m.to_bytes())
            .await?;
        self.store
            .put(
                &format!("{}names/{}", self.prefix, m.name),
                digest.clone().into_bytes(),
            )
            .await?;
        report(done, true);
        Ok(done)
    }

    /// The latest manifest published under `name`.
    pub async fn manifest_by_name(&self, name: &str) -> Result<Manifest> {
        let d = self
            .store
            .get(&format!("{}names/{name}", self.prefix))
            .await?;
        let digest = String::from_utf8_lossy(&d).trim().to_string();
        if !valid_sha(&digest) {
            return Err(Error::Corrupt(format!("names/{name} is not a digest")));
        }
        let b = self
            .store
            .get(&format!("{}manifests/{digest}", self.prefix))
            .await?;
        let m = Manifest::from_bytes(&b)?;
        if m.digest() != digest {
            return Err(Error::Corrupt(format!(
                "manifest {digest} does not match its digest"
            )));
        }
        Ok(m)
    }
}

#[async_trait::async_trait]
impl BlobSource for FallbackStore {
    fn label(&self) -> String {
        self.label.clone()
    }

    async fn fetch(
        &self,
        sha: &str,
        size: u64,
        partial: &Path,
        on_bytes: &(dyn Fn(u64) + Send + Sync),
    ) -> Result<()> {
        let key = self.blob_key(sha);
        let have_size = self
            .store
            .head(&key)
            .await?
            .ok_or_else(|| Error::NotFound(format!("{}: no {key}", self.label)))?;
        if have_size != size {
            return Err(Error::Corrupt(format!(
                "{}: {key} is {have_size} bytes, not {size}",
                self.label
            )));
        }
        let mut have = tokio::fs::metadata(partial)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        if have > size {
            tokio::fs::remove_file(partial).await?;
            have = 0;
        }
        let mut o = tokio::fs::OpenOptions::new();
        o.create(true).append(true);
        #[cfg(unix)]
        o.mode(0o600);
        let mut f = o.open(partial).await?;
        while have < size {
            let n = CHUNK.min(size - have);
            let b = self.store.get_range(&key, have, n).await?;
            if b.is_empty() {
                return Err(Error::Unavailable(format!(
                    "{}: {key} ended early",
                    self.label
                )));
            }
            f.write_all(&b).await?;
            have += b.len() as u64;
            on_bytes(b.len() as u64);
        }
        f.flush().await?;
        f.sync_all().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FileSpec;

    /// Published to a shared folder, a snapshot is pulled on another host
    /// with no LAN source, by name.
    #[tokio::test]
    async fn publish_then_pull_by_name_from_a_shared_folder() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a"), vec![1u8; (CHUNK as usize) + 123]).unwrap();
        std::fs::write(src.join("b"), b"small").unwrap();
        let a = Store::at(dir.path().join("a"));
        let (m, digest) = a
            .export(
                "w0",
                "qemu",
                "abi",
                serde_json::json!({}),
                serde_json::json!({}),
                vec![],
                vec![
                    FileSpec {
                        id: "f0".into(),
                        path: src.join("a"),
                        dir: None,
                        qcow2_backing: None,
                    },
                    FileSpec {
                        id: "f1".into(),
                        path: src.join("b"),
                        dir: None,
                        qcow2_backing: None,
                    },
                ],
            )
            .await
            .unwrap();
        let vol = FallbackStore::dir(dir.path().join("volume/snapshots").to_str().unwrap());
        let up = vol.publish(&a, &m, None).await.unwrap();
        assert_eq!(up, m.size());
        // Published again: nothing uploaded.
        assert_eq!(vol.publish(&a, &m, None).await.unwrap(), 0);
        let got = vol.manifest_by_name("w0").await.unwrap();
        assert_eq!(got.digest(), digest);
        let b = Store::at(dir.path().join("b"));
        let r = crate::pull::pull(&b, &got, vec![Arc::new(vol)], None)
            .await
            .unwrap();
        assert_eq!(r.bytes_done, m.size());
        assert!(r.from.values().all(|f| f == "volume"));
        for f in &m.files {
            assert!(b.has_blob(&f.sha256, f.size));
        }
    }

    #[tokio::test]
    async fn missing_names_bad_keys_and_buckets() {
        let dir = tempfile::tempdir().unwrap();
        let vol = FallbackStore::dir(dir.path().to_str().unwrap());
        assert!(matches!(
            vol.manifest_by_name("nope").await,
            Err(Error::NotFound(_))
        ));
        let d = DirStore::new(dir.path().to_str().unwrap());
        assert!(matches!(
            d.get("../etc/passwd").await,
            Err(Error::Invalid(_))
        ));
        assert!(matches!(d.get("a//b").await, Err(Error::Invalid(_))));
        assert_eq!(
            DirStore::new("~/x").root,
            PathBuf::from(std::env::var("HOME").unwrap()).join("x")
        );
        assert!(matches!(
            S3Store::new(&S3Location::default()).await,
            Err(Error::Invalid(_))
        ));
    }
}
