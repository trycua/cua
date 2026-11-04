//! App icons, cached once for every cua UI (the Spaces apps, the notch, the
//! OpenKoalaBots samples, infinite-canvas): a small in-memory LRU over an
//! on-disk cache under `$CUA_HOME/cache/icons`.
//!
//! - **Keys** are a stable app identity ([`IconKey`]): a scope (the guest's
//!   OS image digest or OS version, or this host), the app's id (bundle id,
//!   `.desktop` id, or a hash of its path) and its version when known. Never
//!   a pid: every window of an app, in every Space from the same image,
//!   shares one entry.
//! - **Entries** are normalized PNGs at 1x and 2x ([`SIZE_1X`], [`SIZE_2X`]
//!   pixels, the icon centered on a transparent square). An SVG the guest
//!   could not rasterize is kept as is.
//! - **Fetches** are single-flight: concurrent requests for one key share
//!   one fetch, and [`IconCache::get_or_fetch_many`] hands every miss of a
//!   batch to one fetch call (one guest round trip for a window list).
//! - **Negative results** (the app has no icon) are remembered for
//!   [`Config::negative_ttl`], in memory and as an empty `<key>.none` file
//!   (so a new process does not ask again); a failed fetch is not
//!   remembered at all.
//! - **Disk** use is capped ([`Config::disk_bytes`]): the least recently
//!   used entries go first. `cua cache ls|du|prune` lists and evicts them
//!   (category `icons`). Writes go through the cua-home test guard, so a
//!   test never writes the user's real `~/.cua`.

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::future::Future;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::watch;

/// 1x icon size, pixels.
pub const SIZE_1X: u32 = 32;
/// 2x icon size, pixels.
pub const SIZE_2X: u32 = 64;
/// Largest source image accepted.
pub const MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;

/// A stable app identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct IconKey(String);

impl IconKey {
    /// `scope` is where the app lives (`guest-linux:<image digest>`,
    /// `host-macos`), `app` its id (bundle id, `.desktop` id or a path
    /// hash) and `version` its version, when known. Case and surrounding
    /// space do not matter.
    pub fn new(scope: &str, app: &str, version: &str) -> Self {
        let norm = |s: &str| s.trim().to_lowercase();
        IconKey(format!(
            "{}\u{1f}{}\u{1f}{}",
            norm(scope),
            norm(app),
            norm(version)
        ))
    }

    /// The readable key.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The file name stem on disk (hex of the key's SHA-256, 32 chars).
    pub fn stem(&self) -> String {
        hex::encode(&Sha256::digest(self.0.as_bytes())[..16])
    }
}

/// A cached icon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Icon {
    /// `image/png` or `image/svg+xml`.
    pub content_type: &'static str,
    /// The 2x PNG ([`SIZE_2X`] px), or the SVG document.
    pub bytes: Vec<u8>,
    /// The 1x PNG ([`SIZE_1X`] px); empty for an SVG.
    pub bytes_1x: Vec<u8>,
}

impl Icon {
    /// The bytes for a `size` pixel request: 1x up to [`SIZE_1X`], else 2x.
    pub fn for_size(&self, size: u32) -> &[u8] {
        if size <= SIZE_1X && !self.bytes_1x.is_empty() {
            &self.bytes_1x
        } else {
            &self.bytes
        }
    }
}

/// What the source bytes are: a PNG or an SVG document (`None` for XPM,
/// ICO or a truncated file).
pub fn source_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]).to_lowercase();
    let t = head.trim_start_matches('\u{feff}').trim_start();
    (t.starts_with('<') && head.contains("<svg")).then_some("image/svg+xml")
}

/// Normalizes a source icon: a PNG becomes 1x and 2x PNGs, centered on a
/// transparent square; an SVG is kept as is. `None` for anything else.
pub fn normalize(source: &[u8]) -> Option<Icon> {
    if source.is_empty() || source.len() > MAX_SOURCE_BYTES {
        return None;
    }
    match source_type(source)? {
        "image/svg+xml" => Some(Icon {
            content_type: "image/svg+xml",
            bytes: source.to_vec(),
            bytes_1x: Vec::new(),
        }),
        _ => {
            let img = image::load_from_memory_with_format(source, image::ImageFormat::Png).ok()?;
            Some(Icon {
                content_type: "image/png",
                bytes: square_png(&img, SIZE_2X)?,
                bytes_1x: square_png(&img, SIZE_1X)?,
            })
        }
    }
}

fn square_png(img: &image::DynamicImage, size: u32) -> Option<Vec<u8>> {
    use image::GenericImageView;
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    let fitted = if w == size && h == size {
        img.to_rgba8()
    } else {
        img.resize(size, size, image::imageops::FilterType::Lanczos3)
            .to_rgba8()
    };
    let mut canvas = image::RgbaImage::new(size, size);
    let x = (size - fitted.width()) / 2;
    let y = (size - fitted.height()) / 2;
    image::imageops::overlay(&mut canvas, &fitted, x as i64, y as i64);
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(canvas)
        .write_to(&mut out, image::ImageFormat::Png)
        .ok()?;
    Some(out.into_inner())
}

/// Cache limits.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// Icons kept decoded in memory.
    pub memory_entries: usize,
    /// Disk budget, bytes.
    pub disk_bytes: u64,
    /// How long "this app has no icon" is remembered.
    pub negative_ttl: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            memory_entries: 512,
            disk_bytes: 32 * 1024 * 1024,
            negative_ttl: Duration::from_secs(5 * 60),
        }
    }
}

/// What the cache knows about a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup {
    /// Cached.
    Hit(Arc<Icon>),
    /// The app has no icon (remembered for the negative TTL).
    Negative,
    /// Unknown: fetch it.
    Miss,
}

/// Counters, for tests and benchmarks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Answered from memory.
    pub memory_hits: u64,
    /// Answered from disk.
    pub disk_hits: u64,
    /// Answered by a remembered negative.
    pub negative_hits: u64,
    /// Keys handed to a fetch.
    pub fetched: u64,
    /// Fetch calls made (a batch is one).
    pub fetch_calls: u64,
}

/// A fetch in flight: `None` until it lands, then its result.
type Outcome = Option<Option<Arc<Icon>>>;
type Pending = watch::Receiver<Outcome>;

/// The icon cache. [`IconCache::shared`] is the process-wide one every
/// binding uses; tests make their own with [`IconCache::new`].
pub struct IconCache {
    dir: Option<PathBuf>,
    config: Config,
    memory: Mutex<Lru>,
    negatives: Mutex<HashMap<IconKey, Instant>>,
    inflight: Mutex<HashMap<IconKey, Pending>>,
    stats: Mutex<Stats>,
}

#[derive(Default)]
struct Lru {
    tick: u64,
    entries: HashMap<IconKey, (Arc<Icon>, u64)>,
}

impl IconCache {
    /// A cache over `dir` (`None`: memory only).
    pub fn new(dir: Option<PathBuf>, config: Config) -> Self {
        IconCache {
            dir,
            config,
            memory: Default::default(),
            negatives: Default::default(),
            inflight: Default::default(),
            stats: Default::default(),
        }
    }

    /// The process-wide cache under `$CUA_HOME/cache/icons`.
    pub fn shared() -> &'static IconCache {
        static SHARED: OnceLock<IconCache> = OnceLock::new();
        SHARED.get_or_init(|| IconCache::new(Some(default_dir()), Config::default()))
    }

    /// The directory, when the cache has one.
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// Counters since creation.
    pub fn stats(&self) -> Stats {
        *self.stats.lock().expect("stats")
    }

    /// Drops what is in memory (the disk stays): a "new process" for tests
    /// and benchmarks.
    pub fn clear_memory(&self) {
        *self.memory.lock().expect("memory") = Lru::default();
        self.negatives.lock().expect("negatives").clear();
    }

    /// Memory, then a remembered negative, then disk.
    pub fn lookup(&self, key: &IconKey) -> Lookup {
        {
            let mut m = self.memory.lock().expect("memory");
            m.tick += 1;
            let tick = m.tick;
            if let Some((icon, used)) = m.entries.get_mut(key) {
                *used = tick;
                let icon = icon.clone();
                drop(m);
                self.stats.lock().expect("stats").memory_hits += 1;
                return Lookup::Hit(icon);
            }
        }
        {
            let mut n = self.negatives.lock().expect("negatives");
            match n.get(key) {
                Some(at) if at.elapsed() < self.config.negative_ttl => {
                    drop(n);
                    self.stats.lock().expect("stats").negative_hits += 1;
                    return Lookup::Negative;
                }
                Some(_) => {
                    n.remove(key);
                }
                None => {}
            }
        }
        if self.disk_negative(key) {
            self.negatives
                .lock()
                .expect("negatives")
                .insert(key.clone(), Instant::now());
            self.stats.lock().expect("stats").negative_hits += 1;
            return Lookup::Negative;
        }
        if let Some(icon) = self.read_disk(key) {
            let icon = Arc::new(icon);
            self.remember(key, icon.clone());
            self.stats.lock().expect("stats").disk_hits += 1;
            return Lookup::Hit(icon);
        }
        Lookup::Miss
    }

    /// Stores a fetched source (`None`: the app has no icon, remembered for
    /// the negative TTL). Returns the normalized icon.
    pub fn put(&self, key: &IconKey, source: Option<&[u8]>) -> Option<Arc<Icon>> {
        let Some(icon) = source.and_then(normalize) else {
            self.negatives
                .lock()
                .expect("negatives")
                .insert(key.clone(), Instant::now());
            self.write_negative(key);
            return None;
        };
        self.write_disk(key, &icon);
        let icon = Arc::new(icon);
        self.remember(key, icon.clone());
        Some(icon)
    }

    /// One icon: cached, or fetched once however many callers ask at the
    /// same time. A failed fetch is returned and not remembered.
    pub async fn get_or_fetch<F, Fut, E>(
        &self,
        key: &IconKey,
        fetch: F,
    ) -> Result<Option<Arc<Icon>>, E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Option<Vec<u8>>, E>>,
    {
        let mut out = self
            .get_or_fetch_many(std::slice::from_ref(key), |_| async move {
                fetch().await.map(|b| vec![b])
            })
            .await?;
        Ok(out.pop().flatten())
    }

    /// Many icons at once, in `keys` order. Every key that is neither
    /// cached nor already being fetched goes to one `fetch` call, which
    /// gets their indices into `keys` and returns one source per index
    /// (`None`: no icon). Keys another caller is fetching wait for it.
    pub async fn get_or_fetch_many<F, Fut, E>(
        &self,
        keys: &[IconKey],
        fetch: F,
    ) -> Result<Vec<Option<Arc<Icon>>>, E>
    where
        F: FnOnce(Vec<usize>) -> Fut,
        Fut: Future<Output = Result<Vec<Option<Vec<u8>>>, E>>,
    {
        let mut results: Vec<Option<Arc<Icon>>> = vec![None; keys.len()];
        let mut owned: Vec<(usize, watch::Sender<Outcome>)> = Vec::new();
        let mut waits: Vec<(usize, Pending)> = Vec::new();
        for (i, key) in keys.iter().enumerate() {
            match self.lookup(key) {
                Lookup::Hit(icon) => results[i] = Some(icon),
                Lookup::Negative => {}
                Lookup::Miss => {
                    let mut inflight = self.inflight.lock().expect("inflight");
                    if let Some(rx) = inflight.get(key) {
                        waits.push((i, rx.clone()));
                    } else {
                        let (tx, rx) = watch::channel(None);
                        inflight.insert(key.clone(), rx);
                        owned.push((i, tx));
                    }
                }
            }
        }
        if !owned.is_empty() {
            let indices: Vec<usize> = owned.iter().map(|(i, _)| *i).collect();
            {
                let mut s = self.stats.lock().expect("stats");
                s.fetch_calls += 1;
                s.fetched += indices.len() as u64;
            }
            let fetched = fetch(indices).await;
            match fetched {
                Ok(sources) => {
                    let mut sources = sources.into_iter();
                    for (i, tx) in owned {
                        let source = sources.next().flatten();
                        let icon = self.put(&keys[i], source.as_deref());
                        self.inflight.lock().expect("inflight").remove(&keys[i]);
                        let _ = tx.send(Some(icon.clone()));
                        results[i] = icon;
                    }
                }
                Err(e) => {
                    // Waiters see the sender dropped: no icon this time.
                    for (i, _) in owned {
                        self.inflight.lock().expect("inflight").remove(&keys[i]);
                    }
                    return Err(e);
                }
            }
        }
        for (i, mut rx) in waits {
            if let Ok(v) = rx.wait_for(|v| v.is_some()).await {
                results[i] = v.clone().flatten();
            }
        }
        Ok(results)
    }

    fn remember(&self, key: &IconKey, icon: Arc<Icon>) {
        let mut m = self.memory.lock().expect("memory");
        m.tick += 1;
        let tick = m.tick;
        m.entries.insert(key.clone(), (icon, tick));
        while m.entries.len() > self.config.memory_entries.max(1) {
            let Some(oldest) = m
                .entries
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            m.entries.remove(&oldest);
        }
    }

    fn paths(&self, key: &IconKey) -> Option<(PathBuf, PathBuf, PathBuf)> {
        let dir = self.dir.as_ref()?;
        let stem = key.stem();
        Some((
            dir.join(format!("{stem}.png")),
            dir.join(format!("{stem}@1x.png")),
            dir.join(format!("{stem}.svg")),
        ))
    }

    fn negative_path(&self, key: &IconKey) -> Option<PathBuf> {
        Some(self.dir.as_ref()?.join(format!("{}.none", key.stem())))
    }

    /// A `<key>.none` file younger than the negative TTL.
    fn disk_negative(&self, key: &IconKey) -> bool {
        let Some(path) = self.negative_path(key) else {
            return false;
        };
        let Some(age) = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
        else {
            return false;
        };
        if age < self.config.negative_ttl {
            return true;
        }
        let _ = std::fs::remove_file(&path);
        false
    }

    fn write_negative(&self, key: &IconKey) {
        let (Some(dir), Some(path)) = (self.dir.as_ref(), self.negative_path(key)) else {
            return;
        };
        if cua_home::guard_write(dir).is_err() {
            return;
        }
        if let Err(error) = std::fs::create_dir_all(dir).and_then(|_| std::fs::write(&path, b"")) {
            tracing::debug!(%error, "icon cache: could not remember a missing icon");
        }
    }

    fn read_disk(&self, key: &IconKey) -> Option<Icon> {
        let (png2, png1, svg) = self.paths(key)?;
        let icon = if let Ok(bytes) = std::fs::read(&png2) {
            touch(&png2);
            Icon {
                content_type: "image/png",
                bytes,
                bytes_1x: std::fs::read(&png1)
                    .inspect(|_| touch(&png1))
                    .unwrap_or_default(),
            }
        } else {
            let bytes = std::fs::read(&svg).ok()?;
            touch(&svg);
            Icon {
                content_type: "image/svg+xml",
                bytes,
                bytes_1x: Vec::new(),
            }
        };
        (!icon.bytes.is_empty()).then_some(icon)
    }

    fn write_disk(&self, key: &IconKey, icon: &Icon) {
        let Some(dir) = self.dir.as_ref() else {
            return;
        };
        if cua_home::guard_write(dir).is_err() {
            return;
        }
        let Some((png2, png1, svg)) = self.paths(key) else {
            return;
        };
        let written = std::fs::create_dir_all(dir).and_then(|_| {
            if icon.content_type == "image/svg+xml" {
                write_atomic(&svg, &icon.bytes)
            } else {
                write_atomic(&png2, &icon.bytes)?;
                write_atomic(&png1, &icon.bytes_1x)
            }
        });
        if let Some(none) = self.negative_path(key) {
            let _ = std::fs::remove_file(none);
        }
        match written {
            Ok(()) => self.trim_disk(),
            Err(error) => tracing::debug!(%error, dir = %dir.display(), "icon cache: write failed"),
        }
    }

    /// Evicts the least recently used files until the directory fits the
    /// disk budget.
    fn trim_disk(&self) {
        let Some(dir) = self.dir.as_ref() else {
            return;
        };
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut files: Vec<(SystemTime, u64, PathBuf)> = rd
            .flatten()
            .filter_map(|e| {
                let m = e.metadata().ok()?;
                m.is_file().then(|| {
                    (
                        m.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                        m.len(),
                        e.path(),
                    )
                })
            })
            .collect();
        let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
        if total <= self.config.disk_bytes {
            return;
        }
        files.sort_by_key(|(t, _, _)| *t);
        for (_, len, path) in files {
            if total <= self.config.disk_bytes {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(len);
            }
        }
    }
}

/// `$CUA_HOME/cache/icons`.
pub fn default_dir() -> PathBuf {
    cua_home::cua_home().join("cache").join("icons")
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Marks a file used (its mtime orders the disk LRU).
fn touch(path: &Path) {
    if let Ok(f) = std::fs::File::options().append(true).open(path) {
        let _ = f.set_modified(SystemTime::now());
    }
}

#[cfg(test)]
mod tests;

/// Live window previews (the teleport picker's tiles, window-drag
/// previews): kept in memory only and only for a few seconds, so a grid
/// that redraws or scrolls asks the window once, and a preview is never
/// more than [`Thumbnails::ttl`] old.
pub struct Thumbnails {
    ttl: Duration,
    capacity: usize,
    entries: Mutex<HashMap<String, Preview>>,
}

/// When a preview was captured, and its PNG (`None`: nothing to show).
type Preview = (Instant, Option<Arc<Vec<u8>>>);

impl Thumbnails {
    /// A cache keeping previews for `ttl`, at most `capacity` of them.
    pub fn new(ttl: Duration, capacity: usize) -> Self {
        Thumbnails {
            ttl,
            capacity: capacity.max(1),
            entries: Default::default(),
        }
    }

    /// The process-wide one: five seconds, 128 previews.
    pub fn shared() -> &'static Thumbnails {
        static SHARED: OnceLock<Thumbnails> = OnceLock::new();
        SHARED.get_or_init(|| Thumbnails::new(Duration::from_secs(5), 128))
    }

    /// How long a preview is reused.
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// A fresh cached preview for `key`, if any (`Some(None)`: captured,
    /// nothing to show).
    pub fn get(&self, key: &str) -> Option<Option<Arc<Vec<u8>>>> {
        let entries = self.entries.lock().expect("thumbnails");
        entries
            .get(key)
            .filter(|(at, _)| at.elapsed() < self.ttl)
            .map(|(_, v)| v.clone())
    }

    /// Stores a capture (`None`: the window gave nothing).
    pub fn put(&self, key: &str, png: Option<Vec<u8>>) -> Option<Arc<Vec<u8>>> {
        let value = png.map(Arc::new);
        let mut entries = self.entries.lock().expect("thumbnails");
        entries.retain(|_, (at, _)| at.elapsed() < self.ttl);
        while entries.len() >= self.capacity {
            let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            entries.remove(&oldest);
        }
        entries.insert(key.to_string(), (Instant::now(), value.clone()));
        value
    }

    /// The cached preview, or `capture`'s (an error is returned, not kept).
    pub fn get_or_capture<E>(
        &self,
        key: &str,
        capture: impl FnOnce() -> Result<Option<Vec<u8>>, E>,
    ) -> Result<Option<Arc<Vec<u8>>>, E> {
        if let Some(hit) = self.get(key) {
            return Ok(hit);
        }
        Ok(self.put(key, capture()?))
    }
}
