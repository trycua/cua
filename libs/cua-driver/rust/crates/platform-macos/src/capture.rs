//! Window / display screenshot capture for macOS.
//!
//! ## Window capture (primary: ScreenCaptureKit)
//!
//! Single-frame window capture prefers native ScreenCaptureKit via
//! `SCScreenshotManager::capture_image` with a desktop-independent window
//! filter, then encodes PNG in memory. No subprocess, temp file, or base64
//! on the native success path.
//!
//! A process-local bounded warm cache (TTL 2s, capacity 32) reuses
//! `SCContentFilter` + `SCStreamConfiguration` plans across rapid captures of
//! the same window id so warm hits skip `SCShareableContent::get` and plan
//! construction. This is not a persistent `SCStream` or frame cache.
//!
//! Sources:
//! - https://developer.apple.com/documentation/screencapturekit/scscreenshotmanager
//! - https://developer.apple.com/documentation/screencapturekit/sccontentfilter/init(desktopindependentwindow:)
//! - https://docs.rs/screencapturekit/6.0.1/screencapturekit/
//!
//! Compatibility fallback: `screencapture -l <windowID> -x -o <file>` when
//! the native path errors or returns empty bytes.
//!
//! ## Display capture
//!
//! `screencapture -x <file>` still captures the full main display (unchanged
//! in this slice).

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use std::collections::HashMap;
use std::hash::Hash;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Bounded TTL map keyed by insertion time. Std-only; used for the warm SCK
/// window filter/config cache and unit-tested independently of ScreenCaptureKit.
struct TimedCache<K, V> {
    ttl: Duration,
    capacity: usize,
    next_seq: u64,
    map: HashMap<K, TimedCacheEntry<V>>,
}

struct TimedCacheEntry<V> {
    value: V,
    inserted_at: Instant,
    /// Monotonic insertion sequence for oldest-first eviction (stable ties).
    seq: u64,
}

impl<K, V> TimedCache<K, V>
where
    K: Eq + Hash + Clone,
{
    fn new(ttl: Duration, capacity: usize) -> Self {
        Self {
            ttl,
            capacity,
            next_seq: 0,
            map: HashMap::new(),
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.map.len()
    }

    /// Insert or replace `key`. Capacity 0 stores nothing. At capacity, a new
    /// key evicts the oldest insertion. Replacing an existing key refreshes
    /// its timestamp and insertion order.
    fn insert_at(&mut self, key: K, value: V, now: Instant) {
        if self.capacity == 0 {
            return;
        }

        // Do not let expired entries occupy capacity merely because their
        // individual keys have not been queried again.
        self.map.retain(|_, entry| {
            now.checked_duration_since(entry.inserted_at)
                .unwrap_or(Duration::ZERO)
                < self.ttl
        });

        if self.map.contains_key(&key) {
            let seq = self.alloc_seq();
            // Key present: refresh value, timestamp, and insertion order.
            let entry = self.map.get_mut(&key).expect("contains_key just true");
            entry.value = value;
            entry.inserted_at = now;
            entry.seq = seq;
            return;
        }

        while self.map.len() >= self.capacity {
            let oldest_key = self
                .map
                .iter()
                .min_by_key(|(_, entry)| entry.seq)
                .map(|(k, _)| k.clone());
            match oldest_key {
                Some(k) => {
                    self.map.remove(&k);
                }
                None => break,
            }
        }

        let seq = self.alloc_seq();
        self.map.insert(
            key,
            TimedCacheEntry {
                value,
                inserted_at: now,
                seq,
            },
        );
    }

    /// Clone a fresh value for `key`. Removes the entry when its age is
    /// greater than or equal to TTL. If `now` is earlier than insertion, age is
    /// treated as zero (still fresh).
    fn get_cloned_at(&mut self, key: &K, now: Instant) -> Option<V>
    where
        V: Clone,
    {
        let expired = match self.map.get(key) {
            Some(entry) => {
                let age = now
                    .checked_duration_since(entry.inserted_at)
                    .unwrap_or(Duration::ZERO);
                age >= self.ttl
            }
            None => return None,
        };

        if expired {
            self.map.remove(key);
            None
        } else {
            self.map.get(key).map(|entry| entry.value.clone())
        }
    }

    /// Remove `key` only when `pred` accepts the stored value. Used to drop a
    /// stale plan without clobbering a concurrently refreshed entry.
    fn remove_if<F>(&mut self, key: &K, pred: F) -> bool
    where
        F: FnOnce(&V) -> bool,
    {
        let should_remove = match self.map.get(key) {
            Some(entry) => pred(&entry.value),
            None => return false,
        };
        if should_remove {
            self.map.remove(key);
            true
        } else {
            false
        }
    }

    fn alloc_seq(&mut self) -> u64 {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        seq
    }
}

struct SecureCapturePath {
    directory: std::path::PathBuf,
    file: std::path::PathBuf,
}

impl SecureCapturePath {
    fn new(file_name: &str) -> anyhow::Result<Self> {
        use std::os::unix::fs::DirBuilderExt;

        let directory = std::env::temp_dir().join(format!(
            "cua-driver-rs-capture-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::DirBuilder::new().mode(0o700).create(&directory)?;
        let file = directory.join(file_name);
        Ok(Self { directory, file })
    }
}

impl Drop for SecureCapturePath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.file);
        let _ = std::fs::remove_dir(&self.directory);
    }
}

/// Prefer `native`; on native error or empty bytes, call `fallback`.
///
/// On non-empty native success, returns those bytes without invoking
/// fallback. When both paths fail, the error message preserves both
/// contexts. Closures are not required to be `Send`/`Sync`.
fn capture_window_with_backends<N, F>(
    window_id: u32,
    native: N,
    fallback: F,
) -> anyhow::Result<Vec<u8>>
where
    N: FnOnce(u32) -> anyhow::Result<Vec<u8>>,
    F: FnOnce(u32) -> anyhow::Result<Vec<u8>>,
{
    match native(window_id) {
        Ok(bytes) if !bytes.is_empty() => Ok(bytes),
        Ok(_) => match fallback(window_id) {
            Ok(bytes) => Ok(bytes),
            Err(fallback_err) => Err(anyhow::anyhow!(
                "window {window_id} capture failed: native produced empty bytes; \
                 shell fallback: {fallback_err:#}"
            )),
        },
        Err(native_err) => match fallback(window_id) {
            Ok(bytes) => Ok(bytes),
            Err(fallback_err) => Err(anyhow::anyhow!(
                "window {window_id} capture failed: native: {native_err:#}; \
                 shell fallback: {fallback_err:#}"
            )),
        },
    }
}

/// Shell compatibility path: `screencapture -l <id> -x -o <tmp.png>`.
fn screenshot_window_bytes_shell(window_id: u32) -> anyhow::Result<Vec<u8>> {
    let capture = SecureCapturePath::new("window.png")?;
    let tmp_path = capture.file.to_string_lossy().into_owned();

    let output = Command::new("screencapture")
        .args([
            "-l",
            &window_id.to_string(),
            "-x", // no sound
            "-o", // no shadow
            &tmp_path,
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        if stderr.is_empty() {
            anyhow::bail!(
                "screencapture failed for window {window_id} with status {}",
                output.status
            );
        }
        anyhow::bail!(
            "screencapture failed for window {window_id} with status {}: {stderr}",
            output.status
        );
    }

    let bytes = std::fs::read(&capture.file)?;

    if bytes.is_empty() {
        anyhow::bail!("screencapture produced empty output for window {window_id}");
    }
    Ok(bytes)
}

/// Hard ceiling on capture dimensions to avoid unbounded allocations.
const MAX_CAPTURE_DIM: u32 = 16384;

fn content_rect_usable(rect: screencapturekit::cg::CGRect) -> bool {
    let w = rect.size.width;
    let h = rect.size.height;
    w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0
}

/// Round a positive finite pixel extent into `1..=MAX_CAPTURE_DIM` as `u32`.
fn rounded_pixel_dim(value: f64, label: &str) -> anyhow::Result<u32> {
    if !value.is_finite() || value <= 0.0 {
        anyhow::bail!("invalid capture {label}: {value}");
    }
    let rounded = value.round();
    if !(rounded.is_finite()) || rounded < 1.0 || rounded > f64::from(MAX_CAPTURE_DIM) {
        anyhow::bail!("capture {label} {rounded} out of allowed range 1..={MAX_CAPTURE_DIM}");
    }
    // After the range check the value fits in u32.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let dim = rounded as u32;
    if dim == 0 || dim > MAX_CAPTURE_DIM {
        anyhow::bail!("capture {label} {dim} out of allowed range 1..={MAX_CAPTURE_DIM}");
    }
    Ok(dim)
}

fn checked_image_dim(value: usize, label: &str) -> anyhow::Result<u32> {
    if value == 0 || value > MAX_CAPTURE_DIM as usize {
        anyhow::bail!("{label} {value} out of allowed range 1..={MAX_CAPTURE_DIM}");
    }
    u32::try_from(value).map_err(|_| anyhow::anyhow!("{label} {value} does not fit u32"))
}

/// Owned ScreenCaptureKit filter + stream config for one window id.
struct WindowCapturePlan {
    filter: screencapturekit::prelude::SCContentFilter,
    config: screencapturekit::prelude::SCStreamConfiguration,
    identity: WindowCaptureIdentity,
}

/// Cheap WindowServer fingerprint used to reject a cached filter after a
/// resize/move, owner change, layer change, or CGWindowID reuse. Float fields
/// are kept as their exact bit patterns because both CGWindowList and
/// ScreenCaptureKit report the same WindowServer frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WindowCaptureIdentity {
    pid: i32,
    layer: i32,
    x: u64,
    y: u64,
    width: u64,
    height: u64,
}

impl WindowCaptureIdentity {
    fn new(pid: i32, layer: i32, x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            pid,
            layer,
            x: x.to_bits(),
            y: y.to_bits(),
            width: width.to_bits(),
            height: height.to_bits(),
        }
    }
}

fn current_window_capture_identity(window_id: u32) -> anyhow::Result<WindowCaptureIdentity> {
    let info = crate::windows::window_info_by_id(window_id)
        .ok_or_else(|| anyhow::anyhow!("WindowServer no longer lists window id {window_id}"))?;
    Ok(WindowCaptureIdentity::new(
        info.pid,
        info.layer,
        info.bounds.x,
        info.bounds.y,
        info.bounds.width,
        info.bounds.height,
    ))
}

const WINDOW_PLAN_CACHE_TTL: Duration = Duration::from_secs(2);
const WINDOW_PLAN_CACHE_CAPACITY: usize = 32;
/// Keep native capture below the public observation deadline after the AX
/// walk's own 20-second bound. The shell fallback retains its prior behavior.
const WINDOW_CAPTURE_NATIVE_TIMEOUT: Duration = Duration::from_secs(3);

/// Permit at most one ScreenCaptureKit worker. If an Apple completion callback
/// never fires, the timed-out worker keeps this permit, and every later request
/// goes straight to the compatibility fallback instead of leaking more
/// blocked threads.
struct NativeCaptureGate {
    active: AtomicBool,
}

impl NativeCaptureGate {
    const fn new() -> Self {
        Self {
            active: AtomicBool::new(false),
        }
    }

    fn try_acquire(&'static self) -> Option<NativeCapturePermit> {
        self.active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| NativeCapturePermit { gate: self })
    }
}

struct NativeCapturePermit {
    gate: &'static NativeCaptureGate,
}

impl Drop for NativeCapturePermit {
    fn drop(&mut self) {
        self.gate.active.store(false, Ordering::Release);
    }
}

fn native_capture_gate() -> &'static NativeCaptureGate {
    static GATE: NativeCaptureGate = NativeCaptureGate::new();
    &GATE
}

fn run_native_capture_worker<T, F>(
    gate: &'static NativeCaptureGate,
    timeout: Duration,
    work: F,
) -> anyhow::Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> anyhow::Result<T> + Send + 'static,
{
    let permit = gate
        .try_acquire()
        .ok_or_else(|| anyhow::anyhow!("ScreenCaptureKit capture already in flight"))?;
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("cua-sck-window-capture".into())
        .spawn(move || {
            let result = work();
            drop(permit);
            let _ = sender.send(result);
        })
        .map_err(|error| anyhow::anyhow!("failed to start ScreenCaptureKit worker: {error}"))?;

    match receiver.recv_timeout(timeout) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            anyhow::bail!(
                "ScreenCaptureKit capture timed out after {} ms",
                timeout.as_millis()
            )
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            anyhow::bail!("ScreenCaptureKit worker ended without a result")
        }
    }
}

fn window_plan_cache() -> &'static Mutex<TimedCache<u32, std::sync::Arc<WindowCapturePlan>>> {
    static CACHE: OnceLock<Mutex<TimedCache<u32, std::sync::Arc<WindowCapturePlan>>>> =
        OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(TimedCache::new(
            WINDOW_PLAN_CACHE_TTL,
            WINDOW_PLAN_CACHE_CAPACITY,
        ))
    })
}

fn lock_window_plan_cache(
) -> std::sync::MutexGuard<'static, TimedCache<u32, std::sync::Arc<WindowCapturePlan>>> {
    window_plan_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Look up shareable content, build desktop-independent filter + pixel config.
/// Runs outside the cache lock.
fn build_window_capture_plan(
    window_id: u32,
    expected_identity: WindowCaptureIdentity,
) -> anyhow::Result<std::sync::Arc<WindowCapturePlan>> {
    use screencapturekit::prelude::{SCContentFilter, SCShareableContent, SCStreamConfiguration};

    let content = SCShareableContent::get()
        .map_err(|e| anyhow::anyhow!("SCShareableContent::get failed: {e}"))?;

    let window = content
        .windows()
        .into_iter()
        .find(|w| w.window_id() == window_id)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "ScreenCaptureKit: window id {window_id} not found in shareable content \
                 (closed, not capturable, or no longer listed)"
            )
        })?;

    let frame = window.frame();
    let owner_pid = window
        .owning_application()
        .map(|application| application.process_id())
        .ok_or_else(|| {
            anyhow::anyhow!("ScreenCaptureKit: window id {window_id} has no owning application")
        })?;
    let actual_identity = WindowCaptureIdentity::new(
        owner_pid,
        window.window_layer(),
        frame.origin.x,
        frame.origin.y,
        frame.size.width,
        frame.size.height,
    );
    if actual_identity != expected_identity {
        anyhow::bail!(
            "ScreenCaptureKit: window id {window_id} changed identity while capture was prepared"
        );
    }

    // Desktop-independent window filter (captures only the specified window):
    // https://developer.apple.com/documentation/screencapturekit/sccontentfilter/init(desktopindependentwindow:)
    let filter = SCContentFilter::create().with_window(&window).build();

    // Pixel output size = content_rect * point_pixel_scale (macOS 14+ filter info).
    // https://docs.rs/screencapturekit/6.0.1/screencapturekit/
    let scale = f64::from(filter.point_pixel_scale());
    let (width_pts, height_pts) = {
        let rect = filter.content_rect();
        if content_rect_usable(rect) {
            (rect.size.width, rect.size.height)
        } else {
            (frame.size.width, frame.size.height)
        }
    };

    let out_w = rounded_pixel_dim(width_pts * scale, "width")?;
    let out_h = rounded_pixel_dim(height_pts * scale, "height")?;

    let config = SCStreamConfiguration::new()
        .with_width(out_w)
        .with_height(out_h);

    Ok(std::sync::Arc::new(WindowCapturePlan {
        filter,
        config,
        identity: expected_identity,
    }))
}

/// Single-frame capture + RGBA/PNG encode from an existing plan.
///
/// https://developer.apple.com/documentation/screencapturekit/scscreenshotmanager
fn capture_window_from_plan(window_id: u32, plan: &WindowCapturePlan) -> anyhow::Result<Vec<u8>> {
    use screencapturekit::screenshot_manager::{CGImageExt, SCScreenshotManager};

    let image = SCScreenshotManager::capture_image(&plan.filter, &plan.config).map_err(|e| {
        anyhow::anyhow!("SCScreenshotManager::capture_image failed for window {window_id}: {e}")
    })?;

    let w = checked_image_dim(image.width(), "CGImage width")?;
    let h = checked_image_dim(image.height(), "CGImage height")?;

    let rgba = image
        .rgba_data()
        .map_err(|e| anyhow::anyhow!("CGImage::rgba_data failed for window {window_id}: {e}"))?;

    let expected_len = (w as u64)
        .checked_mul(h as u64)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| anyhow::anyhow!("RGBA byte length overflow for {w}x{h}"))?;
    if rgba.len() as u64 != expected_len {
        anyhow::bail!(
            "CGImage RGBA length {} != {w}*{h}*4 ({expected_len}) for window {window_id}",
            rgba.len()
        );
    }

    cua_driver_core::image_utils::encode_rgba_to_png(&rgba, w, h)
}

enum CaptureIdentityValidation {
    Matched(Vec<u8>),
    Changed(WindowCaptureIdentity),
}

/// Do not release bytes until WindowServer confirms that the window still has
/// the identity the plan was built for. The injected form keeps the TOCTOU
/// contract deterministic and independently testable.
fn capture_then_validate_identity<C, I>(
    expected_identity: WindowCaptureIdentity,
    capture: C,
    current_identity: I,
) -> anyhow::Result<CaptureIdentityValidation>
where
    C: FnOnce() -> anyhow::Result<Vec<u8>>,
    I: FnOnce() -> anyhow::Result<WindowCaptureIdentity>,
{
    let bytes = capture()?;
    let actual_identity = current_identity()?;
    if actual_identity == expected_identity {
        Ok(CaptureIdentityValidation::Matched(bytes))
    } else {
        Ok(CaptureIdentityValidation::Changed(actual_identity))
    }
}

fn capture_window_from_plan_validated(
    window_id: u32,
    plan: &WindowCapturePlan,
) -> anyhow::Result<CaptureIdentityValidation> {
    capture_then_validate_identity(
        plan.identity,
        || capture_window_from_plan(window_id, plan),
        || current_window_capture_identity(window_id),
    )
}

fn evict_window_capture_plan(window_id: u32, plan: &std::sync::Arc<WindowCapturePlan>) {
    let mut cache = lock_window_plan_cache();
    cache.remove_if(&window_id, |stored| std::sync::Arc::ptr_eq(stored, plan));
}

/// A successful capture discovered that the window mutated. Rebuild once for
/// the observed identity; a second mutation fails closed so stale bytes can
/// never escape and the shell compatibility path can take over.
fn retry_after_identity_change(
    window_id: u32,
    stale_plan: &std::sync::Arc<WindowCapturePlan>,
    actual_identity: WindowCaptureIdentity,
) -> anyhow::Result<Vec<u8>> {
    evict_window_capture_plan(window_id, stale_plan);
    let rebuilt = build_window_capture_plan(window_id, actual_identity)?;
    {
        let mut cache = lock_window_plan_cache();
        cache.insert_at(window_id, std::sync::Arc::clone(&rebuilt), Instant::now());
    }

    match capture_window_from_plan_validated(window_id, &rebuilt) {
        Ok(CaptureIdentityValidation::Matched(bytes)) => Ok(bytes),
        Ok(CaptureIdentityValidation::Changed(_)) => {
            evict_window_capture_plan(window_id, &rebuilt);
            anyhow::bail!(
                "ScreenCaptureKit: window id {window_id} changed identity again during retry"
            )
        }
        Err(error) => {
            evict_window_capture_plan(window_id, &rebuilt);
            Err(error)
        }
    }
}

/// Native ScreenCaptureKit single-frame window capture (in-process PNG).
///
/// Uses a desktop-independent window filter and `SCScreenshotManager` —
/// see module docs for Apple + crate source URLs. No subprocess/temp/base64.
/// Warm hits reuse a bounded two-second filter/config plan cache.
fn screenshot_window_bytes_sck_inner(window_id: u32) -> anyhow::Result<Vec<u8>> {
    let identity = current_window_capture_identity(window_id)?;
    let cached = {
        let mut cache = lock_window_plan_cache();
        cache.get_cloned_at(&window_id, Instant::now())
    };

    if let Some(plan) = cached {
        if plan.identity != identity {
            let mut cache = lock_window_plan_cache();
            cache.remove_if(&window_id, |stored| std::sync::Arc::ptr_eq(stored, &plan));
        } else {
            match capture_window_from_plan_validated(window_id, &plan) {
                Ok(CaptureIdentityValidation::Matched(bytes)) => return Ok(bytes),
                Ok(CaptureIdentityValidation::Changed(actual_identity)) => {
                    return retry_after_identity_change(window_id, &plan, actual_identity);
                }
                Err(first_err) => {
                    // Drop only this Arc if it is still the cached value.
                    evict_window_capture_plan(window_id, &plan);

                    let rebuilt = match build_window_capture_plan(window_id, identity) {
                        Ok(plan) => plan,
                        Err(build_err) => {
                            return Err(anyhow::anyhow!(
                                "ScreenCaptureKit window {window_id}: cached plan capture failed: \
                             {first_err:#}; rebuild failed: {build_err:#}"
                            ));
                        }
                    };

                    {
                        let mut cache = lock_window_plan_cache();
                        cache.insert_at(window_id, std::sync::Arc::clone(&rebuilt), Instant::now());
                    }

                    match capture_window_from_plan_validated(window_id, &rebuilt) {
                        Ok(CaptureIdentityValidation::Matched(bytes)) => return Ok(bytes),
                        Ok(CaptureIdentityValidation::Changed(_)) => {
                            evict_window_capture_plan(window_id, &rebuilt);
                            return Err(anyhow::anyhow!(
                                "ScreenCaptureKit window {window_id}: cached plan capture failed: \
                             {first_err:#}; window changed identity during retry"
                            ));
                        }
                        Err(retry_err) => {
                            evict_window_capture_plan(window_id, &rebuilt);
                            return Err(anyhow::anyhow!(
                                "ScreenCaptureKit window {window_id}: cached plan capture failed: \
                             {first_err:#}; retry after rebuild failed: {retry_err:#}"
                            ));
                        }
                    }
                }
            }
        }
    }

    // Cache miss: build outside the lock, then insert, then capture.
    // Failed builds are never inserted. Capture failure returns as-is (no
    // pointless second rebuild) so the shell fallback can run.
    let plan = build_window_capture_plan(window_id, identity)?;
    {
        let mut cache = lock_window_plan_cache();
        cache.insert_at(window_id, std::sync::Arc::clone(&plan), Instant::now());
    }
    match capture_window_from_plan_validated(window_id, &plan) {
        Ok(CaptureIdentityValidation::Matched(bytes)) => Ok(bytes),
        Ok(CaptureIdentityValidation::Changed(actual_identity)) => {
            retry_after_identity_change(window_id, &plan, actual_identity)
        }
        Err(error) => {
            evict_window_capture_plan(window_id, &plan);
            Err(error)
        }
    }
}

/// Bound the dependency's synchronous callback wrappers without permitting an
/// unbounded number of detached workers. Ownership of the gate permit and all
/// ScreenCaptureKit/CF objects stays on the worker until it really returns.
fn screenshot_window_bytes_sck(window_id: u32) -> anyhow::Result<Vec<u8>> {
    run_native_capture_worker(
        native_capture_gate(),
        WINDOW_CAPTURE_NATIVE_TIMEOUT,
        move || screenshot_window_bytes_sck_inner(window_id),
    )
    .map_err(|error| {
        anyhow::anyhow!(
            "ScreenCaptureKit capture failed for window {window_id}: {error:#}; \
             using shell fallback"
        )
    })
}

/// Capture a window by its `window_id` (CGWindowID).
/// Returns raw PNG bytes or an error.
///
/// Tries ScreenCaptureKit first; falls back to the `screencapture` CLI on
/// native error or empty output.
pub fn screenshot_window_bytes(window_id: u32) -> anyhow::Result<Vec<u8>> {
    capture_window_with_backends(
        window_id,
        screenshot_window_bytes_sck,
        screenshot_window_bytes_shell,
    )
}

/// Capture a window by its `window_id` (CGWindowID).
/// Returns (base64-encoded PNG, width, height) or an error.
pub fn screenshot_window(window_id: u32) -> anyhow::Result<(String, u32, u32)> {
    let bytes = screenshot_window_bytes(window_id)?;
    let (w, h) = png_dimensions(&bytes)?;
    let b64 = BASE64.encode(&bytes);
    Ok((b64, w, h))
}

/// Capture the full main display.
/// Returns raw PNG bytes or an error.
pub fn screenshot_display_bytes() -> anyhow::Result<Vec<u8>> {
    let capture = SecureCapturePath::new("display.png")?;
    let tmp_path = capture.file.to_string_lossy().into_owned();

    let output = Command::new("screencapture")
        .args(["-x", &tmp_path])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        if stderr.is_empty() {
            anyhow::bail!(
                "screencapture failed for main display with status {}",
                output.status
            );
        }
        anyhow::bail!(
            "screencapture failed for main display with status {}: {stderr}",
            output.status
        );
    }

    let bytes = std::fs::read(&capture.file)?;

    if bytes.is_empty() {
        anyhow::bail!("screencapture produced empty output for main display");
    }
    Ok(bytes)
}

/// Capture the main display and return (base64-encoded PNG, width, height).
pub fn screenshot_display() -> anyhow::Result<(String, u32, u32)> {
    let bytes = screenshot_display_bytes()?;
    let (w, h) = png_dimensions(&bytes)?;
    let b64 = BASE64.encode(&bytes);
    Ok((b64, w, h))
}

// PNG/JPEG/resize/crosshair helpers — re-exports of the shared
// `cua_driver_core::image_utils` module. The previous file-local copies were
// near-identical to the Windows and Linux versions; the dedup-audit
// (2026-05) moved them all to one place. See
// `CUA_DRIVER_RS_DEDUP_AUDIT.md` for the audit trail.

/// Convert raw PNG bytes to JPEG at the given quality (1-95).
pub fn png_bytes_to_jpeg(png_bytes: &[u8], quality: u8) -> anyhow::Result<Vec<u8>> {
    cua_driver_core::image_utils::png_bytes_to_jpeg(png_bytes, quality)
}

/// Downscale `png_bytes` so neither dimension exceeds `max_dim`.
/// If `max_dim == 0` or the image already fits, returns the original
/// bytes unchanged.
pub fn resize_png_if_needed(png_bytes: &[u8], max_dim: u32) -> anyhow::Result<Vec<u8>> {
    cua_driver_core::image_utils::resize_png_if_needed(png_bytes, max_dim)
}

/// Draw a red crosshair at pixel (cx, cy) on a PNG image and write to
/// `path`. Used by `click`'s `debug_image_out` param to verify
/// coordinate spaces. The crosshair uses top-left-origin coords
/// matching the click tool's convention.
pub fn write_crosshair_png(png_bytes: &[u8], cx: f64, cy: f64, path: &str) -> anyhow::Result<()> {
    cua_driver_core::image_utils::write_crosshair_png(png_bytes, cx, cy, path)
}

/// Draw a red crosshair at pixel (cx, cy) on a PNG image and return the
/// modified PNG bytes. Used by recording's click-marker callback to
/// produce click.png.
pub fn crosshair_png_bytes(png_bytes: &[u8], cx: f64, cy: f64) -> anyhow::Result<Vec<u8>> {
    cua_driver_core::image_utils::crosshair_png_bytes(png_bytes, cx, cy)
}

/// Parse width and height from a PNG file's IHDR chunk.
pub fn png_dimensions(data: &[u8]) -> anyhow::Result<(u32, u32)> {
    cua_driver_core::image_utils::png_dimensions(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::Arc;

    #[test]
    fn native_window_capture_short_circuits_shell_fallback() {
        let png = cua_driver_core::image_utils::encode_rgba_to_png(&[0, 0, 0, 255], 1, 1)
            .expect("encode 1x1 PNG");
        assert!(!png.is_empty(), "PNG bytes must be non-empty");

        let native_calls = Rc::new(Cell::new(0u32));
        let fallback_calls = Rc::new(Cell::new(0u32));
        let native_window_id = Rc::new(Cell::new(None::<u32>));

        let native_calls_n = Rc::clone(&native_calls);
        let native_window_id_n = Rc::clone(&native_window_id);
        let png_n = png.clone();
        let fallback_calls_f = Rc::clone(&fallback_calls);

        let got = capture_window_with_backends(
            42,
            move |window_id| {
                native_calls_n.set(native_calls_n.get() + 1);
                native_window_id_n.set(Some(window_id));
                Ok(png_n)
            },
            move |_window_id| {
                fallback_calls_f.set(fallback_calls_f.get() + 1);
                anyhow::bail!("shell fallback must not run when native succeeds");
            },
        )
        .expect("native capture should succeed");

        assert_eq!(native_calls.get(), 1, "native backend called once");
        assert_eq!(
            native_window_id.get(),
            Some(42),
            "native backend receives window id 42"
        );
        assert_eq!(fallback_calls.get(), 0, "shell fallback must not run");
        assert_eq!(got, png, "helper returns native PNG bytes verbatim");
    }

    #[test]
    fn empty_native_capture_uses_shell_fallback() {
        let got = capture_window_with_backends(
            42,
            |_| Ok(Vec::new()),
            |window_id| Ok(format!("fallback-{window_id}").into_bytes()),
        )
        .expect("empty native capture should fall back");

        assert_eq!(got, b"fallback-42");
    }

    #[test]
    fn capture_error_preserves_native_and_fallback_contexts() {
        let error = capture_window_with_backends(
            42,
            |_| anyhow::bail!("native denied"),
            |_| anyhow::bail!("fallback denied"),
        )
        .expect_err("both capture backends should fail")
        .to_string();

        assert!(error.contains("native denied"), "{error}");
        assert!(error.contains("fallback denied"), "{error}");
    }

    #[test]
    fn native_window_capture_cache_reuses_fresh_and_expires_stale_entries() {
        use std::time::{Duration, Instant};

        let t0 = Instant::now();
        let mut cache: TimedCache<u32, Arc<&'static str>> =
            TimedCache::new(Duration::from_secs(2), 4);

        let descriptor = Arc::new("descriptor");
        cache.insert_at(42, Arc::clone(&descriptor), t0);
        assert_eq!(cache.len(), 1);

        let fresh = cache
            .get_cloned_at(&42, t0 + Duration::from_millis(1500))
            .expect("fresh cache hit within TTL");
        assert!(
            Arc::ptr_eq(&fresh, &descriptor),
            "fresh hit must return the same Arc"
        );
        assert_eq!(cache.len(), 1);

        let stale = cache.get_cloned_at(&42, t0 + Duration::from_secs(2));
        assert!(stale.is_none(), "entry must expire at TTL");
        assert_eq!(cache.len(), 0, "stale entry must be removed on get");
    }

    #[test]
    fn native_window_capture_cache_purges_expired_before_capacity_eviction() {
        let t0 = Instant::now();
        let mut cache = TimedCache::new(Duration::from_secs(2), 2);
        cache.insert_at(1, "expired", t0);
        cache.insert_at(2, "fresh", t0 + Duration::from_millis(1500));

        cache.insert_at(3, "new", t0 + Duration::from_secs(2));

        assert_eq!(cache.len(), 2);
        assert!(cache
            .get_cloned_at(&1, t0 + Duration::from_secs(2))
            .is_none());
        assert_eq!(
            cache.get_cloned_at(&2, t0 + Duration::from_secs(2)),
            Some("fresh")
        );
        assert_eq!(
            cache.get_cloned_at(&3, t0 + Duration::from_secs(2)),
            Some("new")
        );
    }

    #[test]
    fn native_window_capture_cache_remove_if_does_not_remove_replacement() {
        let t0 = Instant::now();
        let mut cache = TimedCache::new(Duration::from_secs(2), 2);
        let old = Arc::new("old");
        let replacement = Arc::new("replacement");
        cache.insert_at(42, Arc::clone(&old), t0);
        cache.insert_at(42, Arc::clone(&replacement), t0);

        assert!(!cache.remove_if(&42, |stored| Arc::ptr_eq(stored, &old)));
        let stored = cache
            .get_cloned_at(&42, t0)
            .expect("replacement remains cached");
        assert!(Arc::ptr_eq(&stored, &replacement));
    }

    #[test]
    fn window_capture_identity_changes_with_owner_layer_or_frame() {
        let original = WindowCaptureIdentity::new(10, 0, 1.0, 2.0, 800.0, 600.0);
        assert_ne!(
            original,
            WindowCaptureIdentity::new(11, 0, 1.0, 2.0, 800.0, 600.0)
        );
        assert_ne!(
            original,
            WindowCaptureIdentity::new(10, 1, 1.0, 2.0, 800.0, 600.0)
        );
        assert_ne!(
            original,
            WindowCaptureIdentity::new(10, 0, 1.0, 2.0, 801.0, 600.0)
        );
    }

    #[test]
    fn post_capture_identity_change_never_returns_stale_bytes() {
        let before = WindowCaptureIdentity::new(10, 0, 1.0, 2.0, 800.0, 600.0);
        let after = WindowCaptureIdentity::new(10, 0, 5.0, 2.0, 800.0, 600.0);

        let outcome = capture_then_validate_identity(before, || Ok(vec![1, 2, 3]), || Ok(after))
            .expect("capture and identity read succeed");

        assert!(matches!(
            outcome,
            CaptureIdentityValidation::Changed(identity) if identity == after
        ));
    }

    #[test]
    fn second_identity_change_during_retry_also_fails_closed() {
        let first = WindowCaptureIdentity::new(10, 0, 1.0, 2.0, 800.0, 600.0);
        let second = WindowCaptureIdentity::new(10, 0, 5.0, 2.0, 800.0, 600.0);
        let third = WindowCaptureIdentity::new(10, 0, 5.0, 2.0, 900.0, 600.0);

        let first_attempt = capture_then_validate_identity(first, || Ok(vec![1]), || Ok(second))
            .expect("first attempt completes");
        assert!(matches!(
            first_attempt,
            CaptureIdentityValidation::Changed(identity) if identity == second
        ));

        let retry = capture_then_validate_identity(second, || Ok(vec![2]), || Ok(third))
            .expect("retry completes");
        assert!(matches!(
            retry,
            CaptureIdentityValidation::Changed(identity) if identity == third
        ));
    }

    #[test]
    fn identity_mismatch_eviction_does_not_clobber_replacement_cache_entry() {
        let now = Instant::now();
        let mut cache = TimedCache::new(Duration::from_secs(2), 1);
        let stale = std::sync::Arc::new(1u8);
        let replacement = std::sync::Arc::new(2u8);
        cache.insert_at(42, std::sync::Arc::clone(&stale), now);

        // Model another capture refreshing the same key before this stale
        // attempt handles its post-capture identity mismatch.
        cache.insert_at(42, std::sync::Arc::clone(&replacement), now);
        assert!(!cache.remove_if(&42, |stored| { std::sync::Arc::ptr_eq(stored, &stale) }));

        let stored = cache
            .get_cloned_at(&42, now)
            .expect("replacement remains cached");
        assert!(std::sync::Arc::ptr_eq(&stored, &replacement));
    }

    #[test]
    fn native_capture_gate_allows_only_one_worker_until_permit_drops() {
        static GATE: NativeCaptureGate = NativeCaptureGate::new();

        let permit = GATE.try_acquire().expect("first worker acquires gate");
        assert!(
            GATE.try_acquire().is_none(),
            "a second worker must fall back while the first is active"
        );

        drop(permit);
        assert!(
            GATE.try_acquire().is_some(),
            "gate reopens only when the worker-owned permit drops"
        );
    }

    #[test]
    fn timed_out_native_worker_blocks_replacement_until_original_returns() {
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

        static GATE: NativeCaptureGate = NativeCaptureGate::new();
        static SPAWNED_WORK: AtomicUsize = AtomicUsize::new(0);
        let (started_sender, started_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();

        let first_caller = std::thread::spawn(move || {
            run_native_capture_worker(&GATE, Duration::from_millis(10), move || {
                SPAWNED_WORK.fetch_add(1, AtomicOrdering::SeqCst);
                started_sender.send(()).expect("worker reports it started");
                release_receiver
                    .recv()
                    .expect("test releases stalled worker");
                Ok(1u8)
            })
        });
        started_receiver
            .recv()
            .expect("first worker starts before its timeout is observed");
        let first = first_caller.join().expect("first caller joins");
        assert!(first
            .expect_err("stalled worker times out")
            .to_string()
            .contains("timed out"));

        let second = run_native_capture_worker(&GATE, Duration::from_secs(1), || {
            SPAWNED_WORK.fetch_add(1, AtomicOrdering::SeqCst);
            Ok(2u8)
        });
        assert!(second
            .expect_err("replacement is refused while original remains blocked")
            .to_string()
            .contains("already in flight"));
        assert_eq!(
            SPAWNED_WORK.load(AtomicOrdering::SeqCst),
            1,
            "refused replacement must not spawn"
        );

        release_sender.send(()).expect("release original worker");
        let deadline = Instant::now() + Duration::from_secs(1);
        while GATE.active.load(AtomicOrdering::Acquire) && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(!GATE.active.load(AtomicOrdering::Acquire));

        assert_eq!(
            run_native_capture_worker(&GATE, Duration::from_secs(1), || {
                SPAWNED_WORK.fetch_add(1, AtomicOrdering::SeqCst);
                Ok(3u8)
            })
            .expect("gate reopens after original worker returns"),
            3
        );
        assert_eq!(SPAWNED_WORK.load(AtomicOrdering::SeqCst), 2);
    }
}
