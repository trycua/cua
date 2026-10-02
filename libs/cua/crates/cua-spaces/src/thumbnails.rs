//! Space thumbnails, cached once for every client on this machine (the
//! Spaces apps' notch tiles and previews, the CLI, SDK scripts): the latest
//! small JPEG of each Space's primary display, in memory and on disk under
//! `$CUA_HOME/cache/thumbnails`.
//!
//! - [`crate::Spaces::thumbnail`] answers from the cache when the entry is
//!   younger than the caller's `max_age`, else captures a fresh one through
//!   cua-spacesd (and falls back to the older entry when that fails).
//! - Asking marks interest. While someone asked within
//!   [`INTEREST_WINDOW`], [`crate::Spaces::refresh_thumbnails`] (the
//!   daemon runs it every [`REFRESH_TICK`]) captures each running Space
//!   whose entry is older than [`BACKGROUND_INTERVAL`], so a preview is
//!   seldom more than a couple of minutes old. Nobody asking: no guest load.
//! - Entries survive restarts (`<stem>.jpg` plus `<stem>.json`, 0600), a
//!   deleted or forgotten Space's entry goes with it, and the disk use is
//!   capped ([`DISK_BYTES`]): the oldest captures go first. Writes go
//!   through the cua-home test guard, so a test never writes the user's
//!   real `~/.cua`.

use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// A running Space's thumbnail is captured again once it is this old
/// (while someone is interested).
pub const BACKGROUND_INTERVAL: Duration = Duration::from_secs(90);
/// How long one request keeps the background refresh going.
pub const INTEREST_WINDOW: Duration = Duration::from_secs(10 * 60);
/// How often the daemon checks which thumbnails are due.
pub const REFRESH_TICK: Duration = Duration::from_secs(30);
/// Long edge of a captured thumbnail, pixels.
pub const MAX_DIMENSION: u32 = 320;
/// JPEG quality of a captured thumbnail.
pub const QUALITY: u32 = 70;
/// Disk cap for the whole cache.
pub const DISK_BYTES: u64 = 32 * 1024 * 1024;

/// One Space's latest thumbnail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thumbnail {
    /// Encoded image.
    pub image: Vec<u8>,
    /// `jpeg` (or `png`).
    pub format: String,
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
    /// When it was captured.
    pub captured_at: SystemTime,
}

impl Thumbnail {
    /// Whether it is younger than `max_age` at `now` (none: any age).
    pub fn is_fresh(&self, max_age: Option<Duration>, now: SystemTime) -> bool {
        match max_age {
            None => true,
            Some(max) => now
                .duration_since(self.captured_at)
                .map(|age| age < max)
                // Captured "in the future" (the clock moved back): fresh.
                .unwrap_or(true),
        }
    }

    fn ms(&self) -> u64 {
        self.captured_at
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Meta {
    space: String,
    format: String,
    width: u32,
    height: u32,
    captured_at_ms: u64,
}

#[derive(Default)]
struct State {
    entries: HashMap<String, Thumbnail>,
    loaded: bool,
    interest: Option<Instant>,
}

/// The cache. Cheap to share behind the runtime's `Arc`.
pub struct ThumbnailCache {
    dir: Option<PathBuf>,
    disk_bytes: u64,
    state: Mutex<State>,
}

impl std::fmt::Debug for ThumbnailCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThumbnailCache")
            .field("dir", &self.dir)
            .finish()
    }
}

/// The file stem of a Space id (hex of its SHA-256, 32 chars).
fn stem(space: &str) -> String {
    hex::encode(&Sha256::digest(space.as_bytes())[..16])
}

impl ThumbnailCache {
    /// A cache kept in `dir` (none: memory only), capped at `disk_bytes`.
    pub fn new(dir: Option<PathBuf>, disk_bytes: u64) -> Self {
        Self {
            dir,
            disk_bytes,
            state: Mutex::new(State::default()),
        }
    }

    /// The cache under the cua home `home` (`cache/thumbnails`).
    pub fn in_home(home: &Path) -> Self {
        Self::new(Some(home.join("cache").join("thumbnails")), DISK_BYTES)
    }

    /// Where it keeps its files.
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        let mut s = self.state.lock().expect("thumbnail cache");
        if !s.loaded {
            s.loaded = true;
            if let Some(dir) = &self.dir {
                s.entries = load(dir);
            }
        }
        s
    }

    /// The Space's cached thumbnail, of any age.
    pub fn get(&self, space: &str) -> Option<Thumbnail> {
        self.state().entries.get(space).cloned()
    }

    /// Every cached Space id.
    pub fn spaces(&self) -> Vec<String> {
        self.state().entries.keys().cloned().collect()
    }

    /// Keeps `t` as the Space's thumbnail (on disk too), then evicts the
    /// oldest captures past the disk cap.
    pub fn put(&self, space: &str, t: Thumbnail) {
        let mut s = self.state();
        if let Some(dir) = &self.dir
            && let Err(e) = write(dir, space, &t)
        {
            tracing::debug!(space, error = %e, "thumbnail not written");
        }
        s.entries.insert(space.to_string(), t);
        self.enforce_cap(&mut s);
    }

    /// Forgets the Space's thumbnail (a deleted or forgotten Space).
    pub fn remove(&self, space: &str) {
        let mut s = self.state();
        s.entries.remove(space);
        if let Some(dir) = &self.dir {
            unlink(dir, space);
        }
    }

    /// Forgets every thumbnail of a Space not in `keep`.
    pub fn retain(&self, keep: &HashSet<String>) {
        let gone: Vec<String> = self
            .state()
            .entries
            .keys()
            .filter(|k| !keep.contains(*k))
            .cloned()
            .collect();
        for id in gone {
            self.remove(&id);
        }
    }

    /// Someone asked for a thumbnail at `now`.
    pub fn note_interest(&self, now: Instant) {
        self.state.lock().expect("thumbnail cache").interest = Some(now);
    }

    /// Whether someone asked within [`INTEREST_WINDOW`] of `now`.
    pub fn interested(&self, now: Instant) -> bool {
        self.state
            .lock()
            .expect("thumbnail cache")
            .interest
            .is_some_and(|t| now.saturating_duration_since(t) < INTEREST_WINDOW)
    }

    /// Whether the Space's thumbnail is due for a background capture at
    /// `now` (none yet, or older than [`BACKGROUND_INTERVAL`]).
    pub fn due(&self, space: &str, now: SystemTime) -> bool {
        self.get(space)
            .is_none_or(|t| !t.is_fresh(Some(BACKGROUND_INTERVAL), now))
    }

    fn enforce_cap(&self, s: &mut State) {
        let mut total: u64 = s.entries.values().map(|t| t.image.len() as u64).sum();
        if total <= self.disk_bytes {
            return;
        }
        let mut by_age: Vec<(u64, String)> =
            s.entries.iter().map(|(k, t)| (t.ms(), k.clone())).collect();
        by_age.sort();
        for (_, id) in by_age {
            if total <= self.disk_bytes {
                break;
            }
            if let Some(t) = s.entries.remove(&id) {
                total -= t.image.len() as u64;
                if let Some(dir) = &self.dir {
                    unlink(dir, &id);
                }
            }
        }
    }
}

fn load(dir: &Path) -> HashMap<String, Thumbnail> {
    let mut out = HashMap::new();
    let Ok(read) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in read.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(meta) = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Meta>(&b).ok())
        else {
            continue;
        };
        let Ok(image) = std::fs::read(path.with_extension("img")) else {
            continue;
        };
        out.insert(
            meta.space,
            Thumbnail {
                image,
                format: meta.format,
                width: meta.width,
                height: meta.height,
                captured_at: UNIX_EPOCH + Duration::from_millis(meta.captured_at_ms),
            },
        );
    }
    out
}

fn write(dir: &Path, space: &str, t: &Thumbnail) -> std::io::Result<()> {
    let base = dir.join(stem(space));
    cua_home::guard_write(&base)?;
    std::fs::create_dir_all(dir)?;
    let meta = Meta {
        space: space.to_string(),
        format: t.format.clone(),
        width: t.width,
        height: t.height,
        captured_at_ms: t.ms(),
    };
    // The image first: a reader that finds the metadata finds its image.
    cua_home::write_private(&base.with_extension("img"), &t.image)?;
    cua_home::write_private(
        &base.with_extension("json"),
        &serde_json::to_vec(&meta).map_err(std::io::Error::other)?,
    )
}

fn unlink(dir: &Path, space: &str) {
    let base = dir.join(stem(space));
    if cua_home::guard_write(&base).is_err() {
        return;
    }
    let _ = std::fs::remove_file(base.with_extension("json"));
    let _ = std::fs::remove_file(base.with_extension("img"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shot(bytes: usize, at_ms: u64) -> Thumbnail {
        Thumbnail {
            image: vec![7; bytes],
            format: "jpeg".into(),
            width: 320,
            height: 200,
            captured_at: UNIX_EPOCH + Duration::from_millis(at_ms),
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cua-thumbnails-{name}-{}-{}",
            std::process::id(),
            rand::random::<u32>()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn freshness_follows_max_age() {
        let t = shot(10, 1_000_000);
        let now = UNIX_EPOCH + Duration::from_millis(1_060_000);
        assert!(t.is_fresh(None, now), "no max age: any age");
        assert!(t.is_fresh(Some(Duration::from_secs(90)), now));
        assert!(!t.is_fresh(Some(Duration::from_secs(60)), now));
        assert!(
            !t.is_fresh(Some(Duration::ZERO), now),
            "zero: always capture"
        );
        // A clock that moved back: still fresh.
        assert!(t.is_fresh(Some(Duration::from_secs(1)), UNIX_EPOCH));
        let cache = ThumbnailCache::new(None, DISK_BYTES);
        assert!(cache.due("local:a", now), "nothing cached: due");
        cache.put("local:a", t);
        assert!(!cache.due("local:a", now));
        assert!(cache.due("local:a", now + BACKGROUND_INTERVAL));
    }

    #[test]
    fn interest_lapses_after_the_window() {
        let cache = ThumbnailCache::new(None, DISK_BYTES);
        let t0 = Instant::now();
        assert!(!cache.interested(t0), "nobody asked");
        cache.note_interest(t0);
        assert!(cache.interested(t0 + Duration::from_secs(60)));
        assert!(!cache.interested(t0 + INTEREST_WINDOW));
    }

    #[test]
    fn entries_persist_across_a_restart() {
        let dir = temp("persist");
        let cache = ThumbnailCache::new(Some(dir.clone()), DISK_BYTES);
        cache.put("local:a", shot(100, 5_000));
        cache.put("relay:b", shot(50, 6_000));
        drop(cache);
        let again = ThumbnailCache::new(Some(dir.clone()), DISK_BYTES);
        assert_eq!(again.get("local:a"), Some(shot(100, 5_000)));
        assert_eq!(again.get("relay:b"), Some(shot(50, 6_000)));
        // Removed: gone from disk too.
        again.remove("relay:b");
        let third = ThumbnailCache::new(Some(dir.clone()), DISK_BYTES);
        assert_eq!(third.get("relay:b"), None);
        assert!(third.get("local:a").is_some());
        // Files are owner-only.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let img = dir.join(stem("local:a")).with_extension("img");
            let mode = std::fs::metadata(img).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "0600");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn deleted_spaces_and_the_oldest_past_the_cap_are_evicted() {
        let dir = temp("evict");
        let cache = ThumbnailCache::new(Some(dir.clone()), 250);
        cache.put("a", shot(100, 1_000));
        cache.put("b", shot(100, 2_000));
        cache.put("c", shot(100, 3_000));
        // 300 bytes > 250: the oldest (a) went.
        assert_eq!(cache.get("a"), None);
        assert!(cache.get("b").is_some() && cache.get("c").is_some());
        // A new capture of b makes c the oldest.
        cache.put("b", shot(100, 4_000));
        cache.put("d", shot(100, 5_000));
        assert_eq!(cache.get("c"), None);
        // Spaces that no longer exist go.
        cache.retain(&HashSet::from(["d".to_string()]));
        assert_eq!(cache.spaces(), vec!["d".to_string()]);
        let reloaded = ThumbnailCache::new(Some(dir.clone()), 250);
        assert_eq!(reloaded.spaces(), vec!["d".to_string()]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn damaged_files_are_skipped() {
        let dir = temp("damaged");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("x.json"), b"{nope").unwrap();
        std::fs::write(
            dir.join("y.json"),
            br#"{"space":"y","format":"jpeg","width":1,"height":1,"capturedAtMs":1}"#,
        )
        .unwrap();
        // y has no image: skipped.
        let cache = ThumbnailCache::new(Some(dir.clone()), DISK_BYTES);
        assert!(cache.spaces().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
