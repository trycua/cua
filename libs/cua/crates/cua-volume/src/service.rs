// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! [`DriveService`]: everything a runtime runs for the drive beyond tool
//! calls. It owns the block cache, the change feed, the filesystem view,
//! the mount, and live storage switching, and answers the app's
//! Settings > Storage questions.
//!
//! A host builds one per cua home with a [`KeyStore`] (where S3 keys are
//! kept: the credential store in the daemon) and calls
//! [`DriveService::start`] once a Tokio runtime is up.
//!
//! | Platform | Mount | How |
//! |---|---|---|
//! | macOS | `nfs` | a localhost NFSv3 server, mounted with the system's `mount_nfs` (feature `nfs`) |
//! | Linux | `fuse` | FUSE through `/dev/fuse` and `fusermount3` (feature `fuse`) |
//! | other | none | reported as `unsupported` |
//!
//! The volume is mounted on `~/Cua Volume` (macOS) or `~/cua-volume`
//! (elsewhere), named "Cua Volume". A Cua Spaces Local build
//! ([`cua_home::channel::CuaChannel::Local`]) uses `~/Cua Volume Local` and
//! "Cua Volume Local" instead, so it never shares a folder with the Stable
//! app on the same Mac. `CUA_DRIVE_MOUNT_PATH` moves either.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cua_home::channel::CuaChannel;
use serde::{Deserialize, Serialize};

use crate::backend::{Backend, Condition};
use crate::cache::{BlockCache, CacheStats, DEFAULT_CAPACITY};
use crate::config::{BackendKind, DriveConfig, S3Settings};
use crate::feed::{DeviceId, Feed, SyncEvent, SyncStatus};
use crate::vfs::Vfs;
use crate::{Context, Drive, Error, Result, new_id};

/// The environment variable that overrides where the volume mounts.
pub const MOUNT_PATH_VAR: &str = "CUA_DRIVE_MOUNT_PATH";

/// Where S3 keys are kept (never in `config.json`).
pub trait KeyStore: Send + Sync {
    /// The saved `(access_key_id, secret_access_key)`, if any.
    fn load(&self) -> Result<Option<(String, String)>>;
    /// Saves keys (replacing any).
    fn save(&self, access_key_id: &str, secret_access_key: &str) -> Result<()>;
}

/// Keys from `CUA_DRIVE_S3_ACCESS_KEY_ID` / `CUA_DRIVE_S3_SECRET_ACCESS_KEY`
/// only (tests and hosts without a credential store); saving is refused.
pub struct EnvKeys;

impl KeyStore for EnvKeys {
    fn load(&self) -> Result<Option<(String, String)>> {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        Ok(
            match (
                get("CUA_DRIVE_S3_ACCESS_KEY_ID"),
                get("CUA_DRIVE_S3_SECRET_ACCESS_KEY"),
            ) {
                (Some(a), Some(s)) => Some((a, s)),
                _ => None,
            },
        )
    }
    fn save(&self, _: &str, _: &str) -> Result<()> {
        Err(Error::Invalid(
            "this runtime has no credential store to save keys in".into(),
        ))
    }
}

/// Settings > Storage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageInfo {
    pub backend: String,
    pub fs_path: String,
    pub s3: Option<S3Settings>,
    pub has_keys: bool,
    /// Always false: the Cua cloud drive backend has ended.
    pub cloud_available: bool,
}

/// A storage change (or, with `dry_run`, a connection test).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageUpdate {
    pub backend: String,
    #[serde(default)]
    pub s3: Option<S3Settings>,
    #[serde(default)]
    pub access_key_id: Option<String>,
    #[serde(default)]
    pub secret_access_key: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
}

/// What a storage test found.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageCheck {
    pub ok: bool,
    pub reachable: bool,
    pub authorized: bool,
    pub versioning: bool,
    /// One sentence saying what is wrong and how to fix it (when not ok).
    pub detail: Option<String>,
    /// What is wrong, for the app to key on: `unreachable`, `bad_keys`,
    /// `forbidden`, `bucket_missing`, `versioning_off`,
    /// `path_style_needed`.
    #[serde(default)]
    pub problem: Option<String>,
    pub applied: bool,
}

/// Sorts a failed S3 call into a problem code and a sentence for the user.
fn classify(e: &Error, bucket: &str, endpoint: Option<&str>) -> (&'static str, String) {
    let m = e.to_string();
    let at = endpoint.unwrap_or("AWS");
    if m.contains("NoSuchBucket") {
        return (
            "bucket_missing",
            format!(
                "There is no bucket named {bucket:?} at {at}. Create it first, or check the name."
            ),
        );
    }
    if m.contains("InvalidAccessKeyId")
        || m.contains("SignatureDoesNotMatch")
        || m.contains("InvalidToken")
    {
        return (
            "bad_keys",
            "The access key id or the secret was refused. Check both (and the region).".into(),
        );
    }
    if matches!(e, Error::Forbidden(_)) {
        return (
            "forbidden",
            format!(
                "The keys are valid but may not use {bucket:?}. Give them list, read, write and delete on the bucket."
            ),
        );
    }
    (
        "unreachable",
        format!(
            "Nothing answered at {at}. Check the endpoint address and that the storage is running."
        ),
    )
}

/// How to turn versioning on, for the store at `endpoint`.
fn versioning_help(bucket: &str, endpoint: Option<&str>) -> String {
    let how = match endpoint {
        None => format!(
            "in the AWS console (S3 > {bucket} > Properties > Bucket Versioning > Enable) or with `aws s3api put-bucket-versioning --bucket {bucket} --versioning-configuration Status=Enabled`"
        ),
        Some(e) if e.contains("r2.cloudflarestorage.com") => {
            "in the Cloudflare dashboard if your account offers it; without versioning R2 cannot keep history".into()
        }
        Some(_) => format!(
            "with `mc version enable <alias>/{bucket}` (MinIO) or your store's bucket settings"
        ),
    };
    format!(
        "Bucket versioning is off. Turn it on {how}: history, restore and conflict copies need it."
    )
}

/// The mount, for the app.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MountStatus {
    pub enabled: bool,
    /// `off`, `mounting`, `mounted`, `needs_approval`, `unsupported`, `error`.
    pub state: String,
    /// `nfs`, `fuse`, `fskit` or `none`.
    pub method: String,
    pub path: Option<String>,
    pub volume_name: String,
    pub detail: Option<String>,
    pub settings_url: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct MountSettings {
    #[serde(default)]
    enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cache_capacity: Option<u64>,
}

enum Active {
    #[cfg(all(feature = "nfs", target_os = "macos"))]
    Nfs(crate::nfs::NfsServer),
    #[cfg(all(feature = "fuse", target_os = "linux"))]
    Fuse(crate::fuse::FuseMount),
}

struct MountState {
    active: Option<(Active, PathBuf)>,
    error: Option<String>,
}

/// The drive's runtime services for one cua home.
pub struct DriveService {
    drive: Drive,
    home: PathBuf,
    keys: Arc<dyn KeyStore>,
    cache: Arc<BlockCache>,
    feed: Arc<Feed>,
    vfs: Arc<Vfs>,
    mount: tokio::sync::Mutex<MountState>,
    settings: std::sync::Mutex<MountSettings>,
    /// Which Cua Spaces build this is: it names the volume and where it
    /// mounts by default (Stable `~/Cua Volume`, Local `~/Cua Volume Local`).
    channel: CuaChannel,
}

impl std::fmt::Debug for DriveService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DriveService")
            .field("home", &self.home)
            .finish()
    }
}

fn state_dir(home: &Path) -> PathBuf {
    crate::state_dir(home)
}

fn settings_path(home: &Path) -> PathBuf {
    state_dir(home).join("mount.json")
}

/// The mount method this build and platform have.
pub fn mount_method() -> &'static str {
    if cfg!(all(feature = "nfs", target_os = "macos")) {
        "nfs"
    } else if cfg!(all(feature = "fuse", target_os = "linux")) {
        "fuse"
    } else {
        "none"
    }
}

/// Where the volume mounts: `env_path` (`CUA_DRIVE_MOUNT_PATH`) when set,
/// else the user's saved `saved` path, else the channel's default under
/// `user_home` (see [`default_mount_path`]).
fn resolve_mount_path(
    env_path: Option<&std::ffi::OsStr>,
    saved: Option<&Path>,
    channel: CuaChannel,
    user_home: &Path,
    macos: bool,
) -> PathBuf {
    if let Some(p) = env_path.filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    if let Some(p) = saved {
        return p.to_path_buf();
    }
    default_mount_path(channel, user_home, macos)
}

/// The default mount folder under `user_home`: the channel's volume name on
/// macOS (`Cua Volume`, `Cua Volume Local`), that name in lower case with
/// dashes elsewhere (`cua-volume`, `cua-volume-local`).
fn default_mount_path(channel: CuaChannel, user_home: &Path, macos: bool) -> PathBuf {
    if macos {
        user_home.join(channel.volume_name())
    } else {
        user_home.join(channel.volume_name().to_lowercase().replace(' ', "-"))
    }
}

impl DriveService {
    /// The service for `drive` (whose state lives in `<home>/volume`), for
    /// the channel of this process ([`CuaChannel::current`]).
    pub fn new(drive: Drive, home: &Path, keys: Arc<dyn KeyStore>) -> Result<Arc<DriveService>> {
        Self::new_for_channel(drive, home, keys, CuaChannel::current())
    }

    /// [`DriveService::new`] for an explicit channel: Local names the volume
    /// "Cua Volume Local" and mounts it on `~/Cua Volume Local`, so it never
    /// touches the Stable volume's folder.
    pub fn new_for_channel(
        drive: Drive,
        home: &Path,
        keys: Arc<dyn KeyStore>,
        channel: CuaChannel,
    ) -> Result<Arc<DriveService>> {
        let state = state_dir(home);
        std::fs::create_dir_all(&state)?;
        let settings: MountSettings = std::fs::read(settings_path(home))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let cache = Arc::new(BlockCache::open(
            state.join("cache"),
            settings.cache_capacity.unwrap_or(DEFAULT_CAPACITY),
        )?);
        let feed = Feed::new(&drive, DeviceId::load_or_create(&state)?);
        let vfs = Vfs::new(
            &drive,
            Context::user(),
            Some(cache.clone()),
            Some(feed.clone()),
            &state.join("mount"),
        )?;
        Ok(Arc::new(DriveService {
            drive,
            home: home.to_path_buf(),
            keys,
            cache,
            feed,
            vfs,
            mount: tokio::sync::Mutex::new(MountState {
                active: None,
                error: None,
            }),
            settings: std::sync::Mutex::new(settings),
            channel,
        }))
    }

    pub fn drive(&self) -> &Drive {
        &self.drive
    }

    pub fn feed(&self) -> &Arc<Feed> {
        &self.feed
    }

    pub fn vfs(&self) -> &Arc<Vfs> {
        &self.vfs
    }

    pub fn cache(&self) -> &Arc<BlockCache> {
        &self.cache
    }

    /// Starts the change feed and, when the user turned it on, the mount.
    pub async fn start(self: &Arc<Self>) {
        self.feed.spawn();
        if self.settings.lock().unwrap().enabled
            && let Err(e) = self.mount_now().await
        {
            tracing::warn!(error = %e, "drive: the mount did not come up");
        }
    }

    /// Stops the feed and unmounts (uploads land first). The setting is
    /// kept, so the next start mounts again.
    pub async fn shutdown(&self) {
        self.feed.stop();
        let _ = self.unmount_now().await;
    }

    fn save_settings(&self) -> Result<()> {
        let s = self.settings.lock().unwrap().clone();
        let p = settings_path(&self.home);
        cua_home::guard_write(&p)?;
        std::fs::write(p, serde_json::to_vec_pretty(&s)?)?;
        Ok(())
    }

    // -- Storage -------------------------------------------------------

    pub fn storage(&self) -> Result<StorageInfo> {
        let c = DriveConfig::load(&self.home)?;
        Ok(StorageInfo {
            backend: c.backend.as_str().to_string(),
            fs_path: state_dir(&self.home).join("data").display().to_string(),
            s3: c.s3,
            has_keys: self.keys.load().ok().flatten().is_some(),
            cloud_available: false,
        })
    }

    /// Tests, and unless `dry_run` saves and switches to, a backend.
    pub async fn set_storage(&self, u: StorageUpdate) -> Result<StorageCheck> {
        let kind = BackendKind::parse(&u.backend)?;
        let mut config = DriveConfig::load(&self.home).unwrap_or_default();
        let backend: Arc<dyn Backend>;
        let mut check = StorageCheck::default();
        match kind {
            BackendKind::Fs => {
                backend = Arc::new(crate::fs::FsBackend::new(
                    state_dir(&self.home).join("data"),
                ));
                check.reachable = true;
                check.authorized = true;
                check.versioning = true;
            }
            BackendKind::S3 => {
                let s3 =
                    u.s3.clone()
                        .ok_or_else(|| Error::Invalid("s3 settings are required".into()))?;
                if s3.bucket.trim().is_empty() {
                    return Err(Error::Invalid("the bucket is empty".into()));
                }
                let keys = match (&u.access_key_id, &u.secret_access_key) {
                    (Some(a), Some(s)) if !a.is_empty() && !s.is_empty() => {
                        Some((a.clone(), s.clone()))
                    }
                    (None, None) => self.keys.load()?,
                    _ => {
                        return Err(Error::Invalid(
                            "pass both the access key id and the secret, or neither".into(),
                        ));
                    }
                };
                let Some((id, secret)) = keys else {
                    return Err(Error::Invalid(
                        "no S3 keys: enter the access key id and the secret".into(),
                    ));
                };
                let (b, versioning) = s3_backend(&s3, &id, &secret).await?;
                match versioning {
                    Ok(on) => {
                        check.reachable = true;
                        check.authorized = true;
                        check.versioning = on;
                    }
                    Err(e) => {
                        let (problem, detail) = classify(&e, &s3.bucket, s3.endpoint.as_deref());
                        check.reachable = problem != "unreachable";
                        // A self-hosted store that only answers path-style
                        // requests fails oddly with virtual-host addressing
                        // (no DNS for bucket.host, or the server treats the
                        // name as no bucket): try the other way before
                        // reporting anything but refused keys.
                        if problem != "bad_keys" && !s3.path_style && s3.endpoint.is_some() {
                            let alt = S3Settings {
                                path_style: true,
                                ..s3.clone()
                            };
                            if let Ok((_, Ok(_))) = s3_backend(&alt, &id, &secret).await {
                                check.problem = Some("path_style_needed".into());
                                check.detail = Some(
                                    "This store needs path-style addressing (common for MinIO and self-hosted stores). Turn on Path-style and test again."
                                        .into(),
                                );
                                return Ok(check);
                            }
                        }
                        check.problem = Some(problem.into());
                        check.detail = Some(detail);
                        return Ok(check);
                    }
                }
                backend = b;
                config.s3 = Some(s3);
            }
        }
        // Reach it: list, then write, read back and remove a probe object.
        let (bucket, endpoint) = config
            .s3
            .as_ref()
            .filter(|_| kind == BackendKind::S3)
            .map(|s| (s.bucket.clone(), s.endpoint.clone()))
            .unwrap_or_default();
        if let Err(e) = backend.list_dir("").await {
            let (problem, detail) = classify(&e, &bucket, endpoint.as_deref());
            check.authorized = false;
            check.reachable = problem != "unreachable";
            check.problem = Some(problem.into());
            check.detail = Some(detail);
        }
        if check.problem.is_none() {
            let probe = format!(".cua-probe-{}", new_id());
            let body = b"cua volume probe".to_vec();
            let r = async {
                backend.put(&probe, body.clone(), Condition::None).await?;
                let meta = backend
                    .head(&probe)
                    .await?
                    .ok_or_else(|| Error::Backend("the probe object vanished".into()))?;
                let read = backend.get_range(&probe, &meta.version, 0, 64).await?;
                if read != body {
                    return Err(Error::Backend("the probe object read back wrong".into()));
                }
                Ok::<_, Error>(())
            }
            .await;
            let _ = backend.delete(&probe, Condition::None).await;
            if let Err(e) = r {
                let (problem, detail) = classify(&e, &bucket, endpoint.as_deref());
                check.authorized = false;
                check.problem = Some(
                    if problem == "unreachable" {
                        "forbidden"
                    } else {
                        problem
                    }
                    .into(),
                );
                check.detail = Some(format!(
                    "The keys cannot write, read and delete in the bucket: {detail}"
                ));
            }
        }
        if check.problem.is_none() && !check.versioning {
            check.problem = Some("versioning_off".into());
            check.detail = Some(versioning_help(&bucket, endpoint.as_deref()));
        }
        check.ok = check.reachable && check.authorized && check.versioning;
        if !check.ok || u.dry_run {
            return Ok(check);
        }
        if let (Some(a), Some(s)) = (&u.access_key_id, &u.secret_access_key) {
            self.keys.save(a, s)?;
        }
        config.backend = kind;
        config.save(&self.home)?;
        self.drive.set_backend(backend);
        self.cache.clear();
        check.applied = true;
        Ok(check)
    }

    // -- Mount ---------------------------------------------------------

    fn mount_path(&self) -> PathBuf {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        resolve_mount_path(
            std::env::var_os(MOUNT_PATH_VAR).as_deref(),
            self.settings.lock().unwrap().path.as_deref(),
            self.channel,
            &home,
            cfg!(target_os = "macos"),
        )
    }

    /// Where the volume mounts (`None`: the channel's default, `~/Cua Volume`
    /// on macOS and `~/cua-volume` elsewhere for Stable; `~/Cua Volume Local`
    /// and `~/cua-volume-local` for Local). `CUA_DRIVE_MOUNT_PATH` still
    /// overrides both. Takes effect on the next mount.
    pub fn set_mount_path(&self, path: Option<PathBuf>) -> Result<()> {
        self.settings.lock().unwrap().path = path;
        self.save_settings()
    }

    pub async fn mount_status(&self) -> MountStatus {
        let enabled = self.settings.lock().unwrap().enabled;
        let m = self.mount.lock().await;
        let method = mount_method();
        let (state, path, detail) = if method == "none" {
            (
                "unsupported",
                None,
                Some("This build cannot mount the drive on this system.".to_string()),
            )
        } else if let Some((_, p)) = &m.active {
            ("mounted", Some(p.display().to_string()), None)
        } else if let Some(e) = &m.error {
            ("error", None, Some(e.clone()))
        } else {
            ("off", None, None)
        };
        MountStatus {
            enabled,
            state: state.into(),
            method: method.into(),
            path,
            volume_name: self.channel.volume_name().into(),
            detail,
            settings_url: None,
        }
    }

    /// Turns the mount on (persisted) and mounts.
    pub async fn mount(&self) -> Result<MountStatus> {
        {
            self.settings.lock().unwrap().enabled = true;
        }
        self.save_settings()?;
        if let Err(e) = self.mount_now().await {
            self.mount.lock().await.error = Some(e.to_string());
        }
        Ok(self.mount_status().await)
    }

    /// Turns the mount off (persisted) and unmounts after uploads land.
    pub async fn unmount(&self) -> Result<MountStatus> {
        {
            self.settings.lock().unwrap().enabled = false;
        }
        self.save_settings()?;
        self.unmount_now().await?;
        Ok(self.mount_status().await)
    }

    async fn mount_now(&self) -> Result<()> {
        let mut m = self.mount.lock().await;
        if m.active.is_some() {
            return Ok(());
        }
        m.error = None;
        let path = self.mount_path();
        let active = platform::mount(self.vfs.clone(), &path).await?;
        m.active = Some((active, path));
        Ok(())
    }

    async fn unmount_now(&self) -> Result<()> {
        let mut m = self.mount.lock().await;
        let Some((active, path)) = m.active.take() else {
            return Ok(());
        };
        let r = platform::unmount(active, &path).await;
        if let Err(e) = &r {
            m.error = Some(e.to_string());
        }
        r
    }

    // -- Sync ----------------------------------------------------------

    /// Sync across devices, with the store, this machine's mount and the
    /// cache.
    pub async fn sync_status(&self) -> SyncStatus {
        let mut s = self.feed.status();
        let backend = self.drive.backend();
        s.backend = DriveConfig::load(&self.home)
            .map(|c| c.backend.as_str().to_string())
            .unwrap_or_default();
        s.mount = self.mount_status().await.state;
        if backend.remote() {
            s.cache = Some(self.cache.stats());
        }
        s
    }

    pub async fn sync_events(&self, since: u64, wait: Duration) -> (Vec<SyncEvent>, u64) {
        self.feed.events(since, wait).await
    }

    pub fn sync_resolve(&self, path: &str) -> Result<()> {
        if self.feed.resolve(path) {
            Ok(())
        } else {
            Err(Error::NotFound(format!("no conflict at {path}")))
        }
    }

    // -- Cache ---------------------------------------------------------

    pub fn cache_stats(&self) -> CacheStats {
        self.cache.stats()
    }

    pub fn cache_set(&self, capacity: u64) -> Result<CacheStats> {
        self.cache.set_capacity(capacity)?;
        self.settings.lock().unwrap().cache_capacity = Some(capacity);
        self.save_settings()?;
        Ok(self.cache.stats())
    }

    pub fn cache_clear(&self) -> CacheStats {
        self.cache.clear();
        self.cache.stats()
    }
}

/// An S3 backend for `s`, and whether its bucket has versioning on (the
/// error when the bucket cannot be asked).
#[cfg(feature = "s3")]
async fn s3_backend(
    s: &S3Settings,
    id: &str,
    secret: &str,
) -> Result<(Arc<dyn Backend>, Result<bool>)> {
    use crate::s3::{S3Backend, S3Config, S3Credentials, StaticCredentials};
    let b = S3Backend::new(
        S3Config {
            endpoint: s.endpoint.clone(),
            region: s.region.clone(),
            bucket: s.bucket.clone(),
            root: s.root.clone(),
            path_style: s.path_style,
        },
        Arc::new(StaticCredentials(S3Credentials {
            access_key_id: id.into(),
            secret_access_key: secret.into(),
            session_token: None,
            expires_ms: None,
        })),
    )?;
    let versioning = tokio::time::timeout(Duration::from_secs(20), b.versioning_enabled())
        .await
        .unwrap_or_else(|_| Err(Error::Backend("timed out".into())));
    Ok((Arc::new(b), versioning))
}

#[cfg(not(feature = "s3"))]
async fn s3_backend(_: &S3Settings, _: &str, _: &str) -> Result<(Arc<dyn Backend>, Result<bool>)> {
    Err(Error::Invalid(
        "this build has no S3 support; use the cua daemon".into(),
    ))
}

#[cfg(all(feature = "nfs", target_os = "macos"))]
mod platform {
    use super::*;

    /// Whether `path` is an NFS mount from this machine (ours, possibly
    /// left by a daemon that stopped without unmounting).
    fn our_nfs_mount(path: &Path) -> bool {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let Ok(c) = CString::new(path.as_os_str().as_bytes()) else {
            return false;
        };
        // SAFETY: a zeroed statfs is a valid out-parameter.
        let mut st: libc::statfs = unsafe { std::mem::zeroed() };
        // SAFETY: c is NUL-terminated; st is writable.
        if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
            return false;
        }
        let s = |b: &[libc::c_char]| {
            let bytes: Vec<u8> = b
                .iter()
                .take_while(|c| **c != 0)
                .map(|c| *c as u8)
                .collect();
            String::from_utf8_lossy(&bytes).into_owned()
        };
        s(&st.f_fstypename) == "nfs" && s(&st.f_mntfromname).starts_with("127.0.0.1:")
    }

    async fn run(cmd: &str, args: &[&str]) -> Result<()> {
        let out = tokio::time::timeout(
            Duration::from_secs(30),
            tokio::process::Command::new(cmd).args(args).output(),
        )
        .await
        .map_err(|_| Error::Backend(format!("{cmd} timed out")))?
        .map_err(|e| Error::Backend(format!("{cmd}: {e}")))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(Error::Backend(format!(
                "{cmd}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )))
        }
    }

    pub async fn mount(vfs: Arc<Vfs>, path: &Path) -> Result<Active> {
        if our_nfs_mount(path) {
            // A previous daemon's mount whose server is gone.
            let p = path.to_string_lossy().to_string();
            let _ = run("/sbin/umount", &["-f", &p]).await;
        }
        std::fs::create_dir_all(path)?;
        if std::fs::read_dir(path)?.next().is_some() {
            return Err(Error::Invalid(format!(
                "{} is not empty; the drive mounts on an empty folder",
                path.display()
            )));
        }
        let server = crate::nfs::NfsServer::start(vfs).await?;
        let p = path.to_string_lossy().to_string();
        let opts = server.mount_options();
        if let Err(e) = run("/sbin/mount_nfs", &["-o", &opts, "127.0.0.1:/", &p]).await {
            let _ = server.stop().await;
            let _ = std::fs::remove_dir(path);
            return Err(e);
        }
        Ok(Active::Nfs(server))
    }

    pub async fn unmount(active: Active, path: &Path) -> Result<()> {
        #[allow(irrefutable_let_patterns)]
        let Active::Nfs(server) = active else {
            return Ok(());
        };
        // Uploads land before the volume goes away.
        let flushed = server.vfs().flush_all().await;
        let p = path.to_string_lossy().to_string();
        if run("/sbin/umount", &[&p]).await.is_err() {
            // Something still has a file open: uploads are done, force it.
            run("/sbin/umount", &["-f", &p]).await?;
        }
        let stopped = server.stop().await;
        let _ = std::fs::remove_dir(path);
        flushed.and(stopped)
    }
}

#[cfg(all(feature = "fuse", target_os = "linux"))]
mod platform {
    use super::*;

    pub async fn mount(vfs: Arc<Vfs>, path: &Path) -> Result<Active> {
        std::fs::create_dir_all(path)?;
        Ok(Active::Fuse(crate::fuse::FuseMount::mount(vfs, path)?))
    }

    pub async fn unmount(active: Active, path: &Path) -> Result<()> {
        #[allow(irrefutable_let_patterns)]
        let Active::Fuse(m) = active else {
            return Ok(());
        };
        let r = m.unmount().await;
        let _ = std::fs::remove_dir(path);
        r
    }
}

#[cfg(not(any(
    all(feature = "nfs", target_os = "macos"),
    all(feature = "fuse", target_os = "linux")
)))]
mod platform {
    use super::*;

    pub async fn mount(_vfs: Arc<Vfs>, _path: &Path) -> Result<Active> {
        Err(Error::Invalid(
            "this build cannot mount the drive on this system".into(),
        ))
    }

    pub async fn unmount(_active: Active, _path: &Path) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn storage_cache_and_sync_answer_the_app() {
        let home = tempfile::tempdir().unwrap();
        let drive = Drive::open_local(home.path());
        let svc = DriveService::new(drive.clone(), home.path(), Arc::new(EnvKeys)).unwrap();
        let info = svc.storage().unwrap();
        assert_eq!(info.backend, "fs");
        assert!(!info.cloud_available);
        // The cloud backend has ended.
        let e = svc
            .set_storage(StorageUpdate {
                backend: "cloud".into(),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert_eq!(e.tag(), "invalid_argument");
        assert!(e.to_string().contains("has ended"), "{e}");
        // S3 without keys says so before touching the network.
        let e = svc
            .set_storage(StorageUpdate {
                backend: "s3".into(),
                s3: Some(S3Settings {
                    bucket: "b".into(),
                    region: "us-east-1".into(),
                    ..Default::default()
                }),
                access_key_id: Some("only-one".into()),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert_eq!(e.tag(), "invalid_argument");
        // fs: tested, then applied live.
        let c = svc
            .set_storage(StorageUpdate {
                backend: "fs".into(),
                dry_run: true,
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(c.ok && !c.applied, "{c:?}");
        let c = svc
            .set_storage(StorageUpdate {
                backend: "fs".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(c.ok && c.applied, "{c:?}");
        // The probe object never stays behind.
        let keys: Vec<String> = drive
            .backend()
            .list("")
            .await
            .unwrap()
            .into_iter()
            .map(|m| m.key)
            .collect();
        assert!(keys.iter().all(|k| !k.contains(".cua-probe")), "{keys:?}");
        // Cache settings persist.
        assert!(svc.cache_set(1).is_err());
        let st = svc.cache_set(512 << 20).unwrap();
        assert_eq!(st.capacity_bytes, 512 << 20);
        let again = DriveService::new(drive, home.path(), Arc::new(EnvKeys)).unwrap();
        assert_eq!(again.cache_stats().capacity_bytes, 512 << 20);
        assert_eq!(svc.cache_clear().blocks, 0);
        // Sync status before the feed runs.
        let s = svc.sync_status().await;
        assert_eq!(s.feed, "off");
        assert_eq!(s.devices.len(), 1);
        // A build that cannot mount here (Windows) says so instead of "off".
        let idle = if mount_method() == "none" {
            "unsupported"
        } else {
            "off"
        };
        assert_eq!((s.backend.as_str(), s.mount.as_str()), ("fs", idle));
        assert!(
            s.cache.is_none(),
            "no cache in front of a store on this computer"
        );
        assert!(s.pending.is_empty());
        assert!(svc.sync_resolve("nothing").is_err());
        let m = svc.mount_status().await;
        assert!(!m.enabled);
        assert_eq!(m.volume_name, "Cua Volume");
    }

    #[test]
    fn the_default_mount_folder_follows_the_channel() {
        let home = Path::new("/Users/ada");
        // Stable is what it always was.
        assert_eq!(
            default_mount_path(CuaChannel::Stable, home, true),
            home.join("Cua Volume")
        );
        assert_eq!(
            default_mount_path(CuaChannel::Stable, home, false),
            home.join("cua-volume")
        );
        // Local has its own folder beside it, on every platform.
        assert_eq!(
            default_mount_path(CuaChannel::Local, home, true),
            home.join("Cua Volume Local")
        );
        assert_eq!(
            default_mount_path(CuaChannel::Local, home, false),
            home.join("cua-volume-local")
        );
        for macos in [true, false] {
            let stable = default_mount_path(CuaChannel::Stable, home, macos);
            let local = default_mount_path(CuaChannel::Local, home, macos);
            assert_ne!(stable, local);
            assert!(!local.starts_with(&stable) && !stable.starts_with(&local));
        }
        // On macOS the channel's own mount path is this default.
        if let Some(user_home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
            for c in CuaChannel::ALL {
                assert_eq!(
                    default_mount_path(c, Path::new(&user_home), true),
                    c.volume_mount_path()
                );
            }
        }
    }

    #[test]
    fn the_mount_path_override_beats_a_saved_path_and_the_channel() {
        use std::ffi::OsStr;
        let home = Path::new("/Users/ada");
        let saved = Path::new("/Volumes/Big/Cua");
        for c in CuaChannel::ALL {
            // CUA_DRIVE_MOUNT_PATH wins over everything, for both channels.
            assert_eq!(
                resolve_mount_path(Some(OsStr::new("/mnt/x")), Some(saved), c, home, true),
                Path::new("/mnt/x")
            );
            // Then the path saved in the app's settings.
            assert_eq!(resolve_mount_path(None, Some(saved), c, home, true), saved);
            // An empty override is no override.
            assert_eq!(
                resolve_mount_path(Some(OsStr::new("")), None, c, home, true),
                default_mount_path(c, home, true)
            );
            assert_eq!(
                resolve_mount_path(None, None, c, home, true),
                default_mount_path(c, home, true)
            );
        }
    }

    #[tokio::test]
    async fn each_channel_names_and_mounts_its_own_volume() {
        // The default mount of this process, unless the environment moves it.
        let moved = std::env::var_os(MOUNT_PATH_VAR).is_some_and(|p| !p.is_empty());
        let user_home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        for (channel, name) in [
            (CuaChannel::Stable, "Cua Volume"),
            (CuaChannel::Local, "Cua Volume Local"),
        ] {
            let home = tempfile::tempdir().unwrap();
            let drive = Drive::open_local(home.path());
            let svc = DriveService::new_for_channel(drive, home.path(), Arc::new(EnvKeys), channel)
                .unwrap();
            assert_eq!(svc.mount_status().await.volume_name, name);
            if !moved {
                assert_eq!(
                    svc.mount_path(),
                    default_mount_path(channel, &user_home, cfg!(target_os = "macos"))
                );
                // A path the user chose still wins.
                let chosen = home.path().join("elsewhere");
                svc.set_mount_path(Some(chosen.clone())).unwrap();
                assert_eq!(svc.mount_path(), chosen);
                svc.set_mount_path(None).unwrap();
                assert_eq!(
                    svc.mount_path(),
                    default_mount_path(channel, &user_home, cfg!(target_os = "macos"))
                );
            }
        }
    }

    /// Opt-in (CUA_DRIVE_NFS_TEST=1, macOS): mounts through the service,
    /// uses the volume like any program, unmounts.
    #[cfg(all(feature = "nfs", target_os = "macos"))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_macos_mount_comes_up_and_goes_away() {
        if std::env::var("CUA_DRIVE_NFS_TEST").as_deref() != Ok("1") {
            eprintln!("skipped: set CUA_DRIVE_NFS_TEST=1");
            return;
        }
        let base = PathBuf::from(std::env::var("CUA_DRIVE_TEST_DIR").expect("CUA_DRIVE_TEST_DIR"));
        let home = base.join(format!("svc-{}", new_id()));
        let mnt = home.join("mnt").join(CuaChannel::Stable.volume_name());
        let drive = Drive::open_local(&home);
        let svc = DriveService::new(drive.clone(), &home, Arc::new(EnvKeys)).unwrap();
        svc.set_mount_path(Some(mnt.clone())).unwrap();
        let st = svc.mount().await.unwrap();
        assert_eq!(st.state, "mounted", "{st:?}");
        assert_eq!(st.method, "nfs");
        let p = mnt.clone();
        tokio::task::spawn_blocking(move || {
            std::fs::write(p.join("public/from-finder.txt"), b"hi").unwrap();
            assert_eq!(
                std::fs::read(p.join("public/from-finder.txt")).unwrap(),
                b"hi"
            );
        })
        .await
        .unwrap();
        // The write is still pending (no close in NFS); unmount lands it.
        let st = svc.unmount().await.unwrap();
        assert_eq!(st.state, "off");
        assert!(!st.enabled);
        assert!(!mnt.exists(), "the empty mount folder is removed");
        assert_eq!(
            drive
                .session(Context::user())
                .read("public/from-finder.txt", None)
                .await
                .unwrap()
                .0,
            b"hi"
        );
        std::fs::remove_dir_all(&home).unwrap();
    }
}
