//! Driver-independent desktop side-effect observations for E2E tests.

use std::fmt;
use std::time::Duration;

use crate::e2e::OracleKind;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TargetWindow {
    pub pid: u32,
    pub native_id: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ObserverCapabilities {
    pub focus: bool,
    pub z_order: bool,
    pub cursor: bool,
    pub leaked_input: bool,
}

impl ObserverCapabilities {
    fn supports(self, oracle: OracleKind) -> bool {
        match oracle {
            OracleKind::Focus => self.focus,
            OracleKind::ZOrder => self.z_order,
            OracleKind::Cursor => self.cursor,
            OracleKind::NoLeakedInput => self.leaked_input,
            _ => true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetZ {
    BackgroundOccluded,
    BackgroundVisible,
    Foreground,
    Minimized,
    NotFound,
}

impl TargetZ {
    fn rank(self) -> Option<u8> {
        match self {
            Self::BackgroundOccluded => Some(0),
            Self::BackgroundVisible => Some(1),
            Self::Foreground => Some(2),
            Self::Minimized | Self::NotFound => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DesktopSnapshot {
    pub foreground: Option<u64>,
    pub input_focus: Option<u64>,
    pub target_z: TargetZ,
    pub cursor_pos: Option<(f64, f64)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FocusEvent {
    pub from: Option<u64>,
    pub to: Option<u64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DesktopJournal {
    pub focus_events: Vec<FocusEvent>,
    pub leaked_input_events: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObserverError {
    message: String,
}

impl ObserverError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ObserverError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ObserverError {}

pub trait ObserverBackend {
    fn capabilities(&self) -> ObserverCapabilities;
    fn snapshot(&self, target: TargetWindow) -> Result<DesktopSnapshot, ObserverError>;
    fn start_journal(&mut self) -> Result<(), ObserverError>;
    fn drain_journal(&mut self) -> Result<DesktopJournal, ObserverError>;
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObservationDelta {
    pub before: DesktopSnapshot,
    pub after: DesktopSnapshot,
    pub journal: DesktopJournal,
    passed: Vec<OracleKind>,
    unsupported: Vec<OracleKind>,
    violations: Vec<String>,
}

impl ObservationDelta {
    pub fn passed(&self) -> &[OracleKind] {
        &self.passed
    }

    pub fn unsupported(&self) -> &[OracleKind] {
        &self.unsupported
    }

    pub fn violations(&self) -> &[String] {
        &self.violations
    }

    pub fn ensure_supported(&self) -> Result<(), ObserverError> {
        if self.unsupported.is_empty() {
            Ok(())
        } else {
            Err(ObserverError::new(format!(
                "desktop observer does not support required oracles: {:?}",
                self.unsupported
            )))
        }
    }
}

pub struct DesktopObserver<B> {
    backend: B,
    target: TargetWindow,
    settle: Duration,
}

impl<B: ObserverBackend> DesktopObserver<B> {
    pub fn new(backend: B, target: TargetWindow) -> Self {
        Self {
            backend,
            target,
            settle: Duration::from_millis(150),
        }
    }

    pub fn with_settle(mut self, settle: Duration) -> Self {
        self.settle = settle;
        self
    }

    pub fn snapshot(&self) -> Result<DesktopSnapshot, ObserverError> {
        self.backend.snapshot(self.target)
    }

    pub fn observe<R>(
        &mut self,
        requested: &[OracleKind],
        action: impl FnOnce() -> R,
    ) -> Result<(R, ObservationDelta), ObserverError> {
        let before = self.backend.snapshot(self.target)?;
        self.backend.start_journal()?;
        let result = action();
        if !self.settle.is_zero() {
            std::thread::sleep(self.settle);
        }
        let journal = self.backend.drain_journal()?;
        let after = self.backend.snapshot(self.target)?;
        let delta = evaluate(
            self.backend.capabilities(),
            requested,
            before,
            after,
            journal,
        );
        Ok((result, delta))
    }
}

fn evaluate(
    capabilities: ObserverCapabilities,
    requested: &[OracleKind],
    before: DesktopSnapshot,
    after: DesktopSnapshot,
    journal: DesktopJournal,
) -> ObservationDelta {
    let mut passed = Vec::new();
    let mut unsupported = Vec::new();
    let mut violations = Vec::new();

    for oracle in requested.iter().copied() {
        if !capabilities.supports(oracle) {
            unsupported.push(oracle);
            continue;
        }
        let violation = match oracle {
            OracleKind::Focus => {
                if before.foreground != after.foreground {
                    Some(format!(
                        "foreground changed from {:?} to {:?}",
                        before.foreground, after.foreground
                    ))
                } else if before.input_focus != after.input_focus {
                    Some(format!(
                        "input focus changed from {:?} to {:?}",
                        before.input_focus, after.input_focus
                    ))
                } else if !journal.focus_events.is_empty() {
                    Some(format!(
                        "foreground changed transiently: {:?}",
                        journal.focus_events
                    ))
                } else {
                    None
                }
            }
            OracleKind::ZOrder => match (before.target_z.rank(), after.target_z.rank()) {
                (Some(before_rank), Some(after_rank)) if after_rank <= before_rank => None,
                (Some(_), Some(_)) => Some(format!(
                    "target rose from {:?} to {:?}",
                    before.target_z, after.target_z
                )),
                _ => Some(format!(
                    "target z-order could not be compared: {:?} -> {:?}",
                    before.target_z, after.target_z
                )),
            },
            OracleKind::Cursor => match (before.cursor_pos, after.cursor_pos) {
                (Some((before_x, before_y)), Some((after_x, after_y)))
                    if (before_x - after_x).abs() <= 1.0 && (before_y - after_y).abs() <= 1.0 =>
                {
                    None
                }
                (Some(before_pos), Some(after_pos)) => Some(format!(
                    "real cursor moved from {before_pos:?} to {after_pos:?}"
                )),
                _ => Some("real cursor position was unavailable".to_owned()),
            },
            OracleKind::NoLeakedInput => {
                if journal.leaked_input_events.is_empty() {
                    None
                } else {
                    Some(format!(
                        "foreground sentinel received input: {:?}",
                        journal.leaked_input_events
                    ))
                }
            }
            _ => continue,
        };
        if let Some(violation) = violation {
            violations.push(violation);
        } else {
            passed.push(oracle);
        }
    }

    ObservationDelta {
        before,
        after,
        journal,
        passed,
        unsupported,
        violations,
    }
}

#[cfg(target_os = "windows")]
pub mod windows {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use std::time::Duration;

    use windows::Win32::Foundation::{HWND, POINT, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetAncestor, GetCursorPos, GetForegroundWindow, GetWindowRect, IsIconic, IsWindow,
        WindowFromPoint, GA_ROOT,
    };

    use super::{
        DesktopJournal, DesktopSnapshot, FocusEvent, ObserverBackend, ObserverCapabilities,
        ObserverError, TargetWindow, TargetZ,
    };

    pub struct WindowsObserver {
        stop: Arc<AtomicBool>,
        events: Arc<Mutex<Vec<FocusEvent>>>,
        sampler: Option<JoinHandle<()>>,
    }

    impl WindowsObserver {
        pub fn new() -> Self {
            Self {
                stop: Arc::new(AtomicBool::new(false)),
                events: Arc::new(Mutex::new(Vec::new())),
                sampler: None,
            }
        }
    }

    impl Default for WindowsObserver {
        fn default() -> Self {
            Self::new()
        }
    }

    impl ObserverBackend for WindowsObserver {
        fn capabilities(&self) -> ObserverCapabilities {
            ObserverCapabilities {
                focus: true,
                z_order: true,
                cursor: true,
                leaked_input: false,
            }
        }

        fn snapshot(&self, target: TargetWindow) -> Result<DesktopSnapshot, ObserverError> {
            let hwnd = HWND(target.native_id as *mut _);
            let target_z = unsafe {
                if !IsWindow(Some(hwnd)).as_bool() {
                    TargetZ::NotFound
                } else if IsIconic(hwnd).as_bool() {
                    TargetZ::Minimized
                } else if root(GetForegroundWindow()) == root(hwnd) {
                    TargetZ::Foreground
                } else if is_occluded(hwnd)? {
                    TargetZ::BackgroundOccluded
                } else {
                    TargetZ::BackgroundVisible
                }
            };
            let foreground = unsafe { raw(root(GetForegroundWindow())) };
            let mut cursor = POINT::default();
            let cursor_pos = unsafe {
                GetCursorPos(&mut cursor)
                    .ok()
                    .map(|_| (f64::from(cursor.x), f64::from(cursor.y)))
            };
            Ok(DesktopSnapshot {
                foreground,
                input_focus: foreground,
                target_z,
                cursor_pos,
            })
        }

        fn start_journal(&mut self) -> Result<(), ObserverError> {
            if self.sampler.is_some() {
                return Err(ObserverError::new("Windows focus journal already active"));
            }
            self.stop.store(false, Ordering::Release);
            self.events.lock().expect("focus journal lock").clear();
            let stop = Arc::clone(&self.stop);
            let events = Arc::clone(&self.events);
            self.sampler = Some(std::thread::spawn(move || {
                let mut previous = unsafe { raw(root(GetForegroundWindow())) };
                while !stop.load(Ordering::Acquire) {
                    let current = unsafe { raw(root(GetForegroundWindow())) };
                    if current != previous {
                        events.lock().expect("focus journal lock").push(FocusEvent {
                            from: previous,
                            to: current,
                        });
                        previous = current;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }));
            Ok(())
        }

        fn drain_journal(&mut self) -> Result<DesktopJournal, ObserverError> {
            self.stop.store(true, Ordering::Release);
            if let Some(sampler) = self.sampler.take() {
                sampler
                    .join()
                    .map_err(|_| ObserverError::new("Windows focus journal panicked"))?;
            }
            Ok(DesktopJournal {
                focus_events: self.events.lock().expect("focus journal lock").clone(),
                leaked_input_events: Vec::new(),
            })
        }
    }

    impl Drop for WindowsObserver {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(sampler) = self.sampler.take() {
                let _ = sampler.join();
            }
        }
    }

    unsafe fn root(hwnd: HWND) -> HWND {
        if hwnd.0.is_null() {
            hwnd
        } else {
            unsafe { GetAncestor(hwnd, GA_ROOT) }
        }
    }

    fn raw(hwnd: HWND) -> Option<u64> {
        (!hwnd.0.is_null()).then_some(hwnd.0 as usize as u64)
    }

    unsafe fn is_occluded(hwnd: HWND) -> Result<bool, ObserverError> {
        let mut rect = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut rect) }
            .map_err(|error| ObserverError::new(format!("GetWindowRect failed: {error}")))?;
        if rect.right - rect.left <= 4 || rect.bottom - rect.top <= 4 {
            return Ok(false);
        }
        let points = [
            POINT {
                x: rect.left + 2,
                y: rect.top + 2,
            },
            POINT {
                x: rect.right - 3,
                y: rect.top + 2,
            },
            POINT {
                x: rect.left + 2,
                y: rect.bottom - 3,
            },
            POINT {
                x: rect.right - 3,
                y: rect.bottom - 3,
            },
            POINT {
                x: (rect.left + rect.right) / 2,
                y: (rect.top + rect.bottom) / 2,
            },
        ];
        let target_root = unsafe { root(hwnd) };
        let mut cover_root = None;
        for point in points {
            let owner = unsafe { root(WindowFromPoint(point)) };
            if owner.0.is_null() || owner == target_root {
                return Ok(false);
            }
            match cover_root {
                Some(expected) if expected != owner => return Ok(false),
                None => cover_root = Some(owner),
                _ => {}
            }
        }
        let Some(cover_root) = cover_root else {
            return Ok(false);
        };
        let mut cover = RECT::default();
        unsafe { GetWindowRect(cover_root, &mut cover) }
            .map_err(|error| ObserverError::new(format!("read covering window bounds: {error}")))?;
        Ok(rect_fully_contains(cover, rect, 2))
    }

    fn rect_fully_contains(cover: RECT, target: RECT, tolerance: i32) -> bool {
        cover.left <= target.left + tolerance
            && cover.top <= target.top + tolerance
            && cover.right >= target.right - tolerance
            && cover.bottom >= target.bottom - tolerance
    }
}

#[cfg(target_os = "macos")]
pub mod macos {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use std::time::Duration;

    use core_foundation::array::{CFArray, CFArrayRef};
    use core_foundation::base::{CFGetTypeID, CFTypeRef, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use objc2_app_kit::NSWorkspace;

    use super::{
        DesktopJournal, DesktopSnapshot, FocusEvent, ObserverBackend, ObserverCapabilities,
        ObserverError, TargetWindow, TargetZ,
    };

    const WINDOW_LIST_ON_SCREEN: u32 = 1;
    const WINDOW_LIST_EXCLUDE_DESKTOP: u32 = 16;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> CFArrayRef;
        fn CGEventCreate(source: *mut c_void) -> *mut c_void;
        fn CGEventGetLocation(event: *mut c_void) -> CGPoint;
        fn CGMainDisplayID() -> u32;
        fn CGDisplayBounds(display: u32) -> CGRect;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(value: *const c_void);
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGSize {
        width: f64,
        height: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGRect {
        origin: CGPoint,
        size: CGSize,
    }

    #[derive(Clone, Copy)]
    struct Bounds {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    }

    impl Bounds {
        fn area(self) -> f64 {
            self.width.max(0.0) * self.height.max(0.0)
        }

        fn fully_contains(self, other: Self, tolerance: f64) -> bool {
            self.x <= other.x + tolerance
                && self.y <= other.y + tolerance
                && self.x + self.width >= other.x + other.width - tolerance
                && self.y + self.height >= other.y + other.height - tolerance
        }

        fn intersection(self, other: Self) -> Option<Self> {
            let x = self.x.max(other.x);
            let y = self.y.max(other.y);
            let right = (self.x + self.width).min(other.x + other.width);
            let bottom = (self.y + self.height).min(other.y + other.height);
            (right > x && bottom > y).then_some(Self {
                x,
                y,
                width: right - x,
                height: bottom - y,
            })
        }
    }

    struct WindowRow {
        id: u64,
        pid: u32,
        on_screen: bool,
        bounds: Bounds,
    }

    pub struct MacosObserver {
        stop: Arc<AtomicBool>,
        events: Arc<Mutex<Vec<FocusEvent>>>,
        sampler: Option<JoinHandle<()>>,
    }

    impl MacosObserver {
        pub fn new() -> Self {
            Self {
                stop: Arc::new(AtomicBool::new(false)),
                events: Arc::new(Mutex::new(Vec::new())),
                sampler: None,
            }
        }
    }

    impl Default for MacosObserver {
        fn default() -> Self {
            Self::new()
        }
    }

    impl ObserverBackend for MacosObserver {
        fn capabilities(&self) -> ObserverCapabilities {
            ObserverCapabilities {
                focus: true,
                z_order: true,
                cursor: true,
                leaked_input: false,
            }
        }

        fn snapshot(&self, target: TargetWindow) -> Result<DesktopSnapshot, ObserverError> {
            let rows = window_rows();
            let target_index = rows.iter().position(|row| row.id == target.native_id);
            let foreground = frontmost_pid();
            let target_z = match target_index {
                None => TargetZ::NotFound,
                Some(index) if !rows[index].on_screen => TargetZ::Minimized,
                Some(_) if foreground == Some(target.pid as u64) => TargetZ::Foreground,
                Some(index) => {
                    let target_bounds = rows[index].bounds;
                    let display_bounds = main_display_bounds();
                    let full_cover = target_bounds.area() > 0.0
                        && rows[..index].iter().any(|row| {
                            row.pid != target.pid
                                && row_occludes_target(
                                    row.bounds,
                                    target_bounds,
                                    display_bounds,
                                    2.0,
                                )
                        });
                    if full_cover {
                        TargetZ::BackgroundOccluded
                    } else {
                        TargetZ::BackgroundVisible
                    }
                }
            };
            Ok(DesktopSnapshot {
                foreground,
                input_focus: foreground,
                target_z,
                cursor_pos: cursor_position(),
            })
        }

        fn start_journal(&mut self) -> Result<(), ObserverError> {
            if self.sampler.is_some() {
                return Err(ObserverError::new("macOS focus journal already active"));
            }
            self.stop.store(false, Ordering::Release);
            self.events.lock().expect("focus journal lock").clear();
            let stop = Arc::clone(&self.stop);
            let events = Arc::clone(&self.events);
            self.sampler = Some(std::thread::spawn(move || {
                let mut previous = frontmost_pid();
                while !stop.load(Ordering::Acquire) {
                    let current = frontmost_pid();
                    if current != previous {
                        events.lock().expect("focus journal lock").push(FocusEvent {
                            from: previous,
                            to: current,
                        });
                        previous = current;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }));
            Ok(())
        }

        fn drain_journal(&mut self) -> Result<DesktopJournal, ObserverError> {
            self.stop.store(true, Ordering::Release);
            if let Some(sampler) = self.sampler.take() {
                sampler
                    .join()
                    .map_err(|_| ObserverError::new("macOS focus journal panicked"))?;
            }
            Ok(DesktopJournal {
                focus_events: self.events.lock().expect("focus journal lock").clone(),
                leaked_input_events: Vec::new(),
            })
        }
    }

    impl Drop for MacosObserver {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(sampler) = self.sampler.take() {
                let _ = sampler.join();
            }
        }
    }

    fn frontmost_pid() -> Option<u64> {
        unsafe {
            Some(
                NSWorkspace::sharedWorkspace()
                    .frontmostApplication()?
                    .processIdentifier() as u64,
            )
        }
    }

    fn main_display_bounds() -> Bounds {
        let bounds = unsafe { CGDisplayBounds(CGMainDisplayID()) };
        Bounds {
            x: bounds.origin.x,
            y: bounds.origin.y,
            width: bounds.size.width,
            height: bounds.size.height,
        }
    }

    fn row_occludes_target(cover: Bounds, target: Bounds, display: Bounds, tolerance: f64) -> bool {
        if cover.fully_contains(target, tolerance) {
            return true;
        }

        // A maximized foreground sentinel covers the application workspace,
        // but macOS excludes the Dock from its CGWindow bounds. Oversized test
        // fixtures can extend behind that system-owned area on a 1024x768 VM.
        // Accept the portion inside the workspace only when the covering row
        // spans the full display width and is anchored at its top edge; a
        // normal partially-overlapping window still fails the z-order oracle.
        let spans_workspace = cover.x <= display.x + tolerance
            && cover.x + cover.width >= display.x + display.width - tolerance
            && cover.y <= display.y + tolerance;
        spans_workspace
            && target
                .intersection(cover)
                .is_some_and(|visible_target| visible_target.area() > 0.0)
    }

    fn cursor_position() -> Option<(f64, f64)> {
        unsafe {
            let event = CGEventCreate(std::ptr::null_mut());
            if event.is_null() {
                return None;
            }
            let point = CGEventGetLocation(event);
            CFRelease(event);
            Some((point.x, point.y))
        }
    }

    fn window_rows() -> Vec<WindowRow> {
        let raw = unsafe {
            CGWindowListCopyWindowInfo(WINDOW_LIST_ON_SCREEN | WINDOW_LIST_EXCLUDE_DESKTOP, 0)
        };
        if raw.is_null() {
            return Vec::new();
        }
        let array: CFArray<CFTypeRef> = unsafe { CFArray::wrap_under_create_rule(raw.cast()) };
        array
            .iter()
            .filter_map(|item| parse_window_row(*item))
            .collect()
    }

    fn parse_window_row(item: CFTypeRef) -> Option<WindowRow> {
        let dictionary_type = CFDictionary::<*const c_void, *const c_void>::type_id();
        if unsafe { CFGetTypeID(item) } != dictionary_type {
            return None;
        }
        let dictionary: CFDictionary<*const c_void, *const c_void> =
            unsafe { CFDictionary::wrap_under_get_rule(item.cast()) };
        let id = number(&dictionary, "kCGWindowNumber")? as u64;
        let pid = number(&dictionary, "kCGWindowOwnerPID")? as u32;
        let on_screen = boolean(&dictionary, "kCGWindowIsOnscreen").unwrap_or(false);
        let bounds = dictionary_value(&dictionary, "kCGWindowBounds")
            .and_then(|value| {
                if unsafe { CFGetTypeID(value) } != dictionary_type {
                    return None;
                }
                let bounds: CFDictionary<*const c_void, *const c_void> =
                    unsafe { CFDictionary::wrap_under_get_rule(value.cast()) };
                Some(Bounds {
                    x: number(&bounds, "X").unwrap_or(0) as f64,
                    y: number(&bounds, "Y").unwrap_or(0) as f64,
                    width: number(&bounds, "Width").unwrap_or(0) as f64,
                    height: number(&bounds, "Height").unwrap_or(0) as f64,
                })
            })
            .unwrap_or(Bounds {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
            });
        Some(WindowRow {
            id,
            pid,
            on_screen,
            bounds,
        })
    }

    fn dictionary_value(
        dictionary: &CFDictionary<*const c_void, *const c_void>,
        key: &str,
    ) -> Option<CFTypeRef> {
        let key = CFString::new(key);
        dictionary
            .find(key.as_concrete_TypeRef().cast())
            .map(|value| (*value).cast())
    }

    fn number(dictionary: &CFDictionary<*const c_void, *const c_void>, key: &str) -> Option<i64> {
        let value = dictionary_value(dictionary, key)?;
        if unsafe { CFGetTypeID(value) } != CFNumber::type_id() {
            return None;
        }
        unsafe { CFNumber::wrap_under_get_rule(value.cast()) }.to_i64()
    }

    fn boolean(dictionary: &CFDictionary<*const c_void, *const c_void>, key: &str) -> Option<bool> {
        let value = dictionary_value(dictionary, key)?;
        if unsafe { CFGetTypeID(value) } != CFBoolean::type_id() {
            return None;
        }
        Some(bool::from(unsafe {
            CFBoolean::wrap_under_get_rule(value.cast())
        }))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn partial_overlap_is_not_full_occlusion() {
            let target = Bounds {
                x: 100.0,
                y: 100.0,
                width: 400.0,
                height: 300.0,
            };
            let partial = Bounds {
                x: 0.0,
                y: 0.0,
                width: 350.0,
                height: 900.0,
            };
            let full = Bounds {
                x: 0.0,
                y: 0.0,
                width: 1280.0,
                height: 900.0,
            };
            assert!(!partial.fully_contains(target, 2.0));
            assert!(full.fully_contains(target, 2.0));
        }

        #[test]
        fn maximized_workspace_occludes_the_visible_part_of_an_oversized_target() {
            let target = Bounds {
                x: 162.0,
                y: 30.0,
                width: 700.0,
                height: 852.0,
            };
            let display = Bounds {
                x: 0.0,
                y: 0.0,
                width: 1024.0,
                height: 768.0,
            };
            let workspace = Bounds {
                x: 0.0,
                y: 0.0,
                width: 1024.0,
                height: 682.0,
            };
            assert!(row_occludes_target(workspace, target, display, 2.0));

            let partial = Bounds {
                x: 0.0,
                y: 0.0,
                width: 900.0,
                height: 682.0,
            };
            assert!(!row_occludes_target(partial, target, display, 2.0));
        }

        #[test]
        fn native_snapshot_reads_desktop_without_a_target() {
            let snapshot = MacosObserver::new()
                .snapshot(TargetWindow {
                    native_id: u64::MAX,
                    pid: u32::MAX,
                })
                .expect("macOS desktop snapshot");
            assert_eq!(snapshot.target_z, TargetZ::NotFound);
            assert!(snapshot.foreground.is_some());
            assert!(snapshot.cursor_pos.is_some());
        }
    }
}

#[cfg(target_os = "linux")]
pub mod linux {
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use std::time::Duration;

    use serde::Deserialize;
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{
        AtomEnum, ConnectionExt, MapState, Rectangle, Window, WindowClass,
    };
    use x11rb::rust_connection::RustConnection;

    use super::{
        DesktopJournal, DesktopSnapshot, FocusEvent, ObserverBackend, ObserverCapabilities,
        ObserverError, TargetWindow, TargetZ,
    };

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum SessionKind {
        X11,
        Sway,
        Gnome,
        CuaCompositor,
        Missing,
    }

    pub struct LinuxObserver {
        session: SessionKind,
        stop: Arc<AtomicBool>,
        events: Arc<Mutex<Vec<FocusEvent>>>,
        sampler: Option<JoinHandle<()>>,
    }

    impl LinuxObserver {
        pub fn new() -> Self {
            let explicit_wayland = std::env::var("XDG_SESSION_TYPE")
                .map(|value| value.eq_ignore_ascii_case("wayland"))
                .unwrap_or(false)
                || std::env::var_os("WAYLAND_DISPLAY").is_some();
            let session = if explicit_wayland {
                if cua_compositor_available() {
                    SessionKind::CuaCompositor
                } else if sway_available() {
                    SessionKind::Sway
                } else if gnome_windows().is_ok() {
                    SessionKind::Gnome
                } else {
                    SessionKind::Missing
                }
            } else if std::env::var_os("DISPLAY").is_some() {
                if x11_window_manager_ready() {
                    SessionKind::X11
                } else {
                    SessionKind::Missing
                }
            } else {
                SessionKind::Missing
            };
            Self {
                session,
                stop: Arc::new(AtomicBool::new(false)),
                events: Arc::new(Mutex::new(Vec::new())),
                sampler: None,
            }
        }
    }

    impl Default for LinuxObserver {
        fn default() -> Self {
            Self::new()
        }
    }

    impl ObserverBackend for LinuxObserver {
        fn capabilities(&self) -> ObserverCapabilities {
            match self.session {
                SessionKind::X11 => ObserverCapabilities {
                    focus: true,
                    z_order: true,
                    cursor: true,
                    leaked_input: false,
                },
                SessionKind::Sway | SessionKind::Gnome | SessionKind::CuaCompositor => {
                    ObserverCapabilities {
                        focus: true,
                        z_order: true,
                        cursor: false,
                        leaked_input: false,
                    }
                }
                SessionKind::Missing => ObserverCapabilities::default(),
            }
        }

        fn snapshot(&self, target: TargetWindow) -> Result<DesktopSnapshot, ObserverError> {
            match self.session {
                SessionKind::X11 => x11_snapshot(target),
                SessionKind::Sway => sway_snapshot(target),
                SessionKind::Gnome => gnome_snapshot(target),
                SessionKind::CuaCompositor => cua_compositor_snapshot(target),
                SessionKind::Missing => Ok(DesktopSnapshot {
                    foreground: None,
                    input_focus: None,
                    target_z: TargetZ::NotFound,
                    cursor_pos: None,
                }),
            }
        }

        fn start_journal(&mut self) -> Result<(), ObserverError> {
            if self.sampler.is_some() {
                return Err(ObserverError::new("Linux focus journal already active"));
            }
            self.stop.store(false, Ordering::Release);
            self.events.lock().expect("focus journal lock").clear();
            if self.session == SessionKind::Missing {
                return Ok(());
            }
            let session = self.session;
            let stop = Arc::clone(&self.stop);
            let events = Arc::clone(&self.events);
            self.sampler = Some(std::thread::spawn(move || {
                let focus_identity = || match session {
                    SessionKind::X11 => x11_focus_identity(),
                    SessionKind::Sway => sway_focus_identity(),
                    SessionKind::Gnome => gnome_focus_identity(),
                    SessionKind::CuaCompositor => cua_compositor_focus_identity(),
                    SessionKind::Missing => Ok(None),
                };
                let mut previous = focus_identity().ok().flatten();
                while !stop.load(Ordering::Acquire) {
                    if let Ok(current) = focus_identity() {
                        if current != previous {
                            events.lock().expect("focus journal lock").push(FocusEvent {
                                from: previous,
                                to: current,
                            });
                            previous = current;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }));
            Ok(())
        }

        fn drain_journal(&mut self) -> Result<DesktopJournal, ObserverError> {
            self.stop.store(true, Ordering::Release);
            if let Some(sampler) = self.sampler.take() {
                sampler
                    .join()
                    .map_err(|_| ObserverError::new("Linux focus journal panicked"))?;
            }
            Ok(DesktopJournal {
                focus_events: self.events.lock().expect("focus journal lock").clone(),
                leaked_input_events: Vec::new(),
            })
        }
    }

    impl Drop for LinuxObserver {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(sampler) = self.sampler.take() {
                let _ = sampler.join();
            }
        }
    }

    fn x11_connection() -> Result<(RustConnection, usize), ObserverError> {
        x11rb::connect(None)
            .map_err(|error| ObserverError::new(format!("X11 connect failed: {error}")))
    }

    #[derive(Default)]
    struct SwayTreeState {
        focused: Option<u64>,
        focused_workspace: Option<u64>,
        focused_fullscreen: Option<u64>,
        target: Option<(u64, bool, Option<u64>)>,
    }

    fn sway_tree() -> Result<serde_json::Value, ObserverError> {
        let output = Command::new("swaymsg")
            .args(["-r", "-t", "get_tree"])
            .output()
            .map_err(|error| ObserverError::new(format!("swaymsg get_tree failed: {error}")))?;
        if !output.status.success() {
            return Err(ObserverError::new(format!(
                "swaymsg get_tree exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        serde_json::from_slice(&output.stdout)
            .map_err(|error| ObserverError::new(format!("invalid sway tree JSON: {error}")))
    }

    fn sway_available() -> bool {
        std::env::var_os("SWAYSOCK").is_some_and(|value| !value.is_empty()) && sway_tree().is_ok()
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct CuaCompositorState {
        focused_pid: Option<u64>,
        target_z: TargetZ,
    }

    fn parse_cua_compositor_state(line: &str) -> Result<CuaCompositorState, ObserverError> {
        let mut fields = line.split_whitespace();
        if fields.next() != Some("state") {
            return Err(ObserverError::new(format!(
                "invalid cua-compositor observer response: {line:?}"
            )));
        }
        let focused_pid = fields
            .next()
            .ok_or_else(|| ObserverError::new("cua-compositor state omitted focused pid"))?
            .parse::<u64>()
            .map_err(|error| ObserverError::new(format!("invalid focused pid: {error}")))?;
        let target_z = match fields.next() {
            Some("foreground") => TargetZ::Foreground,
            Some("background_occluded") => TargetZ::BackgroundOccluded,
            Some("background_visible") => TargetZ::BackgroundVisible,
            Some("not_found") => TargetZ::NotFound,
            Some(value) => {
                return Err(ObserverError::new(format!(
                    "invalid cua-compositor target state: {value}"
                )))
            }
            None => {
                return Err(ObserverError::new(
                    "cua-compositor state omitted target state",
                ))
            }
        };
        if fields.next().is_some() {
            return Err(ObserverError::new(
                "cua-compositor state contained trailing fields",
            ));
        }
        Ok(CuaCompositorState {
            focused_pid: (focused_pid != 0).then_some(focused_pid),
            target_z,
        })
    }

    fn cua_compositor_state(target_pid: u32) -> Result<CuaCompositorState, ObserverError> {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixStream;

        let socket = std::env::var("CUA_INJECT_SOCKET")
            .map_err(|_| ObserverError::new("CUA_INJECT_SOCKET is not set"))?;
        let stream = UnixStream::connect(&socket).map_err(|error| {
            ObserverError::new(format!("connect cua-compositor observer socket: {error}"))
        })?;
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .map_err(|error| ObserverError::new(format!("set observer timeout: {error}")))?;
        let mut writer = stream
            .try_clone()
            .map_err(|error| ObserverError::new(format!("clone observer socket: {error}")))?;
        let mut reader = BufReader::new(stream);

        writeln!(writer, "cua-inject v1")
            .and_then(|_| writer.flush())
            .map_err(|error| ObserverError::new(format!("write observer handshake: {error}")))?;
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|error| ObserverError::new(format!("read observer handshake: {error}")))?;
        if line.trim() != "cua-inject v1" {
            return Err(ObserverError::new(format!(
                "cua-compositor observer handshake mismatch: {:?}",
                line.trim()
            )));
        }

        writeln!(writer, "q {target_pid}")
            .and_then(|_| writer.flush())
            .map_err(|error| ObserverError::new(format!("write observer query: {error}")))?;
        line.clear();
        let read = reader
            .read_line(&mut line)
            .map_err(|error| ObserverError::new(format!("read observer query: {error}")))?;
        if read == 0 {
            return Err(ObserverError::new(
                "cua-compositor closed the observer query",
            ));
        }
        parse_cua_compositor_state(&line)
    }

    fn cua_compositor_available() -> bool {
        std::env::var_os("CUA_INJECT_SOCKET").is_some() && cua_compositor_state(0).is_ok()
    }

    fn cua_compositor_snapshot(target: TargetWindow) -> Result<DesktopSnapshot, ObserverError> {
        let state = cua_compositor_state(target.pid)?;
        Ok(DesktopSnapshot {
            foreground: state.focused_pid,
            input_focus: state.focused_pid,
            target_z: state.target_z,
            cursor_pos: None,
        })
    }

    fn cua_compositor_focus_identity() -> Result<Option<u64>, ObserverError> {
        Ok(cua_compositor_state(0)?.focused_pid)
    }

    #[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
    struct GnomeWindow {
        id: u64,
        pid: u32,
        #[serde(rename = "title")]
        _title: String,
        x: i64,
        y: i64,
        w: i64,
        h: i64,
        focused: bool,
        minimized: bool,
        visible: bool,
        stacking: u64,
    }

    fn gnome_windows() -> Result<Vec<GnomeWindow>, ObserverError> {
        let mut child = Command::new("gdbus")
            .args([
                "call",
                "--session",
                "--dest",
                "org.cua.WinRects",
                "--object-path",
                "/org/cua/WinRects",
                "--method",
                "org.cua.WinRects.GetRects",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| ObserverError::new(format!("gdbus GetRects failed: {error}")))?;
        let deadline = std::time::Instant::now() + Duration::from_millis(800);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => {
                    let output = child.wait_with_output().map_err(|error| {
                        ObserverError::new(format!("gdbus GetRects output failed: {error}"))
                    })?;
                    if !output.status.success() {
                        return Err(ObserverError::new(format!(
                            "gdbus GetRects exited with {}: {}",
                            output.status,
                            String::from_utf8_lossy(&output.stderr)
                        )));
                    }
                    return parse_gnome_windows(&String::from_utf8_lossy(&output.stdout));
                }
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(15))
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(ObserverError::new("gdbus GetRects timed out"));
                }
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(ObserverError::new(format!(
                        "gdbus GetRects wait failed: {error}"
                    )));
                }
            }
        }
    }

    fn parse_gnome_windows(raw: &str) -> Result<Vec<GnomeWindow>, ObserverError> {
        let start = raw
            .find('[')
            .ok_or_else(|| ObserverError::new("GetRects response had no JSON array"))?;
        let end = raw
            .rfind(']')
            .filter(|end| *end >= start)
            .ok_or_else(|| ObserverError::new("GetRects response had no complete JSON array"))?;
        serde_json::from_str(&raw[start..=end])
            .map_err(|error| ObserverError::new(format!("invalid GetRects JSON: {error}")))
    }

    fn gnome_target<'a>(
        windows: &'a [GnomeWindow],
        target: TargetWindow,
    ) -> Option<&'a GnomeWindow> {
        let matching = windows.iter().filter(|window| window.pid == target.pid);
        matching
            .clone()
            .find(|window| window.id == target.native_id)
            .or_else(|| matching.max_by_key(|window| window.stacking))
    }

    fn fully_covers(cover: &GnomeWindow, target: &GnomeWindow) -> bool {
        target.w > 0
            && target.h > 0
            && cover.w > 0
            && cover.h > 0
            && cover.x <= target.x
            && cover.y <= target.y
            && cover.x.saturating_add(cover.w) >= target.x.saturating_add(target.w)
            && cover.y.saturating_add(cover.h) >= target.y.saturating_add(target.h)
    }

    fn classify_gnome_target(windows: &[GnomeWindow], target: TargetWindow) -> TargetZ {
        let Some(target) = gnome_target(windows, target) else {
            return TargetZ::NotFound;
        };
        if target.focused {
            TargetZ::Foreground
        } else if target.minimized || !target.visible {
            TargetZ::Minimized
        } else if windows.iter().any(|window| {
            window.stacking > target.stacking
                && window.visible
                && !window.minimized
                && fully_covers(window, target)
        }) {
            TargetZ::BackgroundOccluded
        } else {
            TargetZ::BackgroundVisible
        }
    }

    fn gnome_focus_identity_from(windows: &[GnomeWindow]) -> Option<u64> {
        windows
            .iter()
            .find(|window| window.focused)
            .map(|window| window.id)
    }

    fn gnome_snapshot(target: TargetWindow) -> Result<DesktopSnapshot, ObserverError> {
        let windows = gnome_windows()?;
        let focused = gnome_focus_identity_from(&windows);
        Ok(DesktopSnapshot {
            foreground: focused,
            input_focus: focused,
            target_z: classify_gnome_target(&windows, target),
            cursor_pos: None,
        })
    }

    fn gnome_focus_identity() -> Result<Option<u64>, ObserverError> {
        Ok(gnome_focus_identity_from(&gnome_windows()?))
    }

    fn walk_sway_tree(
        node: &serde_json::Value,
        target_pid: u32,
        workspace: Option<u64>,
        state: &mut SwayTreeState,
    ) -> bool {
        let id = node["id"].as_u64();
        let workspace = if node["type"].as_str() == Some("workspace") {
            id
        } else {
            workspace
        };
        let focused = node["focused"].as_bool().unwrap_or(false);
        if focused {
            state.focused = id;
            state.focused_workspace = workspace;
        }
        if node["pid"].as_u64() == Some(u64::from(target_pid)) {
            if let Some(id) = id {
                state.target = Some((id, node["visible"].as_bool().unwrap_or(true), workspace));
            }
        }
        let mut subtree_focused = focused;
        for child in ["nodes", "floating_nodes"]
            .into_iter()
            .flat_map(|key| node[key].as_array().into_iter().flatten())
        {
            subtree_focused |= walk_sway_tree(child, target_pid, workspace, state);
        }
        // Sway commonly puts fullscreen_mode on a container while focus belongs
        // to a descendant surface. Treat the focused subtree as fullscreen so a
        // sibling hidden behind that container is classified as occluded.
        if subtree_focused && node["fullscreen_mode"].as_i64().unwrap_or(0) != 0 {
            state.focused_fullscreen = id;
        }
        subtree_focused
    }

    fn classify_sway_target(state: &SwayTreeState) -> TargetZ {
        match state.target {
            None => TargetZ::NotFound,
            Some((id, _, _)) if state.focused == Some(id) => TargetZ::Foreground,
            Some((_, _, target_workspace))
                if state.focused_fullscreen.is_some()
                    && target_workspace == state.focused_workspace =>
            {
                // Sway reports windows hidden behind a fullscreen sibling as
                // visible=false. They are occluded, not minimized.
                TargetZ::BackgroundOccluded
            }
            Some((_, false, _)) => TargetZ::Minimized,
            Some(_) => TargetZ::BackgroundVisible,
        }
    }

    fn sway_snapshot(target: TargetWindow) -> Result<DesktopSnapshot, ObserverError> {
        let tree = sway_tree()?;
        let mut state = SwayTreeState::default();
        let _ = walk_sway_tree(&tree, target.pid, None, &mut state);
        let target_z = classify_sway_target(&state);
        Ok(DesktopSnapshot {
            foreground: state.focused,
            input_focus: state.focused,
            target_z,
            cursor_pos: None,
        })
    }

    fn sway_focus_identity() -> Result<Option<u64>, ObserverError> {
        let tree = sway_tree()?;
        let mut state = SwayTreeState::default();
        let _ = walk_sway_tree(&tree, 0, None, &mut state);
        Ok(state.focused)
    }

    fn x11_snapshot(target: TargetWindow) -> Result<DesktopSnapshot, ObserverError> {
        let (connection, screen_index) = x11_connection()?;
        let root = connection.setup().roots[screen_index].root;
        let target = u32::try_from(target.native_id)
            .map_err(|_| ObserverError::new("X11 window id does not fit in u32"))?;
        let target_root = match top_level(&connection, target, root) {
            Ok(window) => window,
            Err(_) => {
                return Ok(DesktopSnapshot {
                    foreground: active_window(&connection, root)?.map(u64::from),
                    input_focus: input_focus(&connection, root)?.map(u64::from),
                    target_z: TargetZ::NotFound,
                    cursor_pos: query_pointer(&connection, root)?,
                });
            }
        };
        let active = active_window(&connection, root)?;
        let focus = input_focus(&connection, root)?;
        let target_z = if !is_viewable(&connection, target_root)? {
            TargetZ::Minimized
        } else if active == Some(target_root) || focus == Some(target_root) {
            TargetZ::Foreground
        } else if is_occluded(&connection, root, target_root)? {
            TargetZ::BackgroundOccluded
        } else {
            TargetZ::BackgroundVisible
        };
        Ok(DesktopSnapshot {
            foreground: active.map(u64::from),
            input_focus: focus.map(u64::from),
            target_z,
            cursor_pos: query_pointer(&connection, root)?,
        })
    }

    fn x11_focus_identity() -> Result<Option<u64>, ObserverError> {
        let (connection, screen_index) = x11_connection()?;
        let root = connection.setup().roots[screen_index].root;
        Ok(active_window(&connection, root)?
            .or(input_focus(&connection, root)?)
            .map(u64::from))
    }

    fn x11_window_manager_ready() -> bool {
        let Ok((connection, screen_index)) = x11_connection() else {
            return false;
        };
        let root = connection.setup().roots[screen_index].root;
        let Ok(atom_cookie) = connection.intern_atom(false, b"_NET_SUPPORTING_WM_CHECK") else {
            return false;
        };
        let Ok(atom_reply) = atom_cookie.reply() else {
            return false;
        };
        let atom = atom_reply.atom;
        let read_window = |window| {
            connection
                .get_property(false, window, atom, AtomEnum::WINDOW, 0, 1)
                .ok()?
                .reply()
                .ok()?
                .value32()?
                .next()
        };
        let Some(manager) = read_window(root) else {
            return false;
        };
        manager != 0 && read_window(manager) == Some(manager)
    }

    fn active_window(
        connection: &RustConnection,
        root: Window,
    ) -> Result<Option<Window>, ObserverError> {
        let atom = connection
            .intern_atom(false, b"_NET_ACTIVE_WINDOW")
            .map_err(x11_error("intern _NET_ACTIVE_WINDOW"))?
            .reply()
            .map_err(x11_error("read _NET_ACTIVE_WINDOW atom"))?
            .atom;
        let reply = connection
            .get_property(false, root, atom, AtomEnum::WINDOW, 0, 1)
            .map_err(x11_error("request _NET_ACTIVE_WINDOW"))?
            .reply()
            .map_err(x11_error("read _NET_ACTIVE_WINDOW"))?;
        let active = reply
            .value32()
            .and_then(|mut values| values.next())
            .filter(|window| *window != 0);
        active
            .map(|window| top_level(connection, window, root))
            .transpose()
    }

    fn input_focus(
        connection: &RustConnection,
        root: Window,
    ) -> Result<Option<Window>, ObserverError> {
        let window = connection
            .get_input_focus()
            .map_err(x11_error("request input focus"))?
            .reply()
            .map_err(x11_error("read input focus"))?
            .focus;
        if window == 0 || window == 1 {
            Ok(None)
        } else {
            top_level(connection, window, root).map(Some)
        }
    }

    fn top_level(
        connection: &RustConnection,
        mut window: Window,
        root: Window,
    ) -> Result<Window, ObserverError> {
        for _ in 0..32 {
            let tree = connection
                .query_tree(window)
                .map_err(x11_error("request X11 window tree"))?
                .reply()
                .map_err(x11_error("read X11 window tree"))?;
            if tree.parent == root || window == root {
                return Ok(window);
            }
            window = tree.parent;
        }
        Err(ObserverError::new("X11 window ancestry exceeded 32 levels"))
    }

    fn is_viewable(connection: &RustConnection, window: Window) -> Result<bool, ObserverError> {
        let attributes = connection
            .get_window_attributes(window)
            .map_err(x11_error("request X11 window attributes"))?
            .reply()
            .map_err(x11_error("read X11 window attributes"))?;
        Ok(attributes.class == WindowClass::INPUT_OUTPUT
            && attributes.map_state == MapState::VIEWABLE)
    }

    fn absolute_bounds(
        connection: &RustConnection,
        window: Window,
        root: Window,
    ) -> Result<Rectangle, ObserverError> {
        let geometry = connection
            .get_geometry(window)
            .map_err(x11_error("request X11 window geometry"))?
            .reply()
            .map_err(x11_error("read X11 window geometry"))?;
        let translated = connection
            .translate_coordinates(window, root, 0, 0)
            .map_err(x11_error("request X11 translated coordinates"))?
            .reply()
            .map_err(x11_error("read X11 translated coordinates"))?;
        Ok(Rectangle {
            x: translated.dst_x,
            y: translated.dst_y,
            width: geometry.width,
            height: geometry.height,
        })
    }

    fn is_occluded(
        connection: &RustConnection,
        root: Window,
        target: Window,
    ) -> Result<bool, ObserverError> {
        let target_bounds = absolute_bounds(connection, target, root)?;
        if target_bounds.width <= 4 || target_bounds.height <= 4 {
            return Ok(false);
        }
        let mut cover = None;
        for (x, y) in sample_points(target_bounds) {
            let child = connection
                .translate_coordinates(root, root, x, y)
                .map_err(x11_error("request X11 occlusion point"))?
                .reply()
                .map_err(x11_error("read X11 occlusion point"))?
                .child;
            if child == 0 || child == target {
                return Ok(false);
            }
            match cover {
                Some(expected) if expected != child => return Ok(false),
                None => cover = Some(child),
                _ => {}
            }
        }
        let Some(cover) = cover else {
            return Ok(false);
        };
        let cover_bounds = absolute_bounds(connection, cover, root)?;
        Ok(rectangle_fully_contains(cover_bounds, target_bounds, 2))
    }

    fn rectangle_fully_contains(cover: Rectangle, target: Rectangle, tolerance: i32) -> bool {
        let cover_left = i32::from(cover.x);
        let cover_top = i32::from(cover.y);
        let cover_right = cover_left + i32::from(cover.width);
        let cover_bottom = cover_top + i32::from(cover.height);
        let target_left = i32::from(target.x);
        let target_top = i32::from(target.y);
        let target_right = target_left + i32::from(target.width);
        let target_bottom = target_top + i32::from(target.height);
        cover_left <= target_left + tolerance
            && cover_top <= target_top + tolerance
            && cover_right >= target_right - tolerance
            && cover_bottom >= target_bottom - tolerance
    }

    fn sample_points(bounds: Rectangle) -> [(i16, i16); 5] {
        let left = bounds.x.saturating_add(2);
        let top = bounds.y.saturating_add(2);
        let right = i32::from(bounds.x)
            .saturating_add(i32::from(bounds.width))
            .saturating_sub(3)
            .clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
        let bottom = i32::from(bounds.y)
            .saturating_add(i32::from(bounds.height))
            .saturating_sub(3)
            .clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
        let center_x = i32::from(bounds.x)
            .saturating_add(i32::from(bounds.width) / 2)
            .clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
        let center_y = i32::from(bounds.y)
            .saturating_add(i32::from(bounds.height) / 2)
            .clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
        [
            (left, top),
            (right, top),
            (left, bottom),
            (right, bottom),
            (center_x, center_y),
        ]
    }

    fn query_pointer(
        connection: &RustConnection,
        root: Window,
    ) -> Result<Option<(f64, f64)>, ObserverError> {
        let pointer = connection
            .query_pointer(root)
            .map_err(x11_error("request X11 pointer"))?
            .reply()
            .map_err(x11_error("read X11 pointer"))?;
        Ok(Some((f64::from(pointer.root_x), f64::from(pointer.root_y))))
    }

    fn x11_error<E: std::fmt::Display>(operation: &'static str) -> impl FnOnce(E) -> ObserverError {
        move |error| ObserverError::new(format!("{operation} failed: {error}"))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn gnome_payload() -> &'static str {
            r#"('[{"id":10,"pid":100,"title":"target","x":10,"y":20,"w":300,"h":200,"focused":false,"minimized":false,"visible":true,"stacking":0},{"id":20,"pid":200,"title":"sentinel","x":0,"y":0,"w":800,"h":600,"focused":true,"minimized":false,"visible":true,"stacking":1}]',)"#
        }

        #[test]
        fn occlusion_samples_corners_and_center() {
            let bounds = Rectangle {
                x: -100,
                y: 20,
                width: 200,
                height: 100,
            };
            assert_eq!(
                sample_points(bounds),
                [(-98, 22), (97, 22), (-98, 117), (97, 117), (0, 70)]
            );
        }

        #[test]
        fn x11_partial_overlap_is_not_full_occlusion() {
            let target = Rectangle {
                x: 100,
                y: 100,
                width: 400,
                height: 300,
            };
            let partial = Rectangle {
                x: 0,
                y: 0,
                width: 350,
                height: 900,
            };
            let full = Rectangle {
                x: 0,
                y: 0,
                width: 1280,
                height: 900,
            };
            assert!(!rectangle_fully_contains(partial, target, 2));
            assert!(rectangle_fully_contains(full, target, 2));
        }

        #[test]
        fn sway_hidden_target_behind_fullscreen_sibling_is_occluded() {
            let state = SwayTreeState {
                focused: Some(30),
                focused_workspace: Some(10),
                focused_fullscreen: Some(30),
                target: Some((20, false, Some(10))),
            };
            assert_eq!(classify_sway_target(&state), TargetZ::BackgroundOccluded);
        }

        #[test]
        fn sway_fullscreen_ancestor_of_focused_surface_occludes_target() {
            let tree = serde_json::json!({
                "id": 1,
                "type": "root",
                "nodes": [{
                    "id": 10,
                    "type": "workspace",
                    "nodes": [
                        {
                            "id": 20,
                            "pid": 100,
                            "visible": false,
                            "focused": false,
                            "fullscreen_mode": 0,
                            "nodes": []
                        },
                        {
                            "id": 30,
                            "focused": false,
                            "fullscreen_mode": 1,
                            "nodes": [{
                                "id": 31,
                                "pid": 200,
                                "visible": true,
                                "focused": true,
                                "fullscreen_mode": 0,
                                "nodes": []
                            }]
                        }
                    ]
                }]
            });
            let mut state = SwayTreeState::default();
            assert!(walk_sway_tree(&tree, 100, None, &mut state));
            assert_eq!(state.focused, Some(31));
            assert_eq!(state.focused_fullscreen, Some(30));
            assert_eq!(classify_sway_target(&state), TargetZ::BackgroundOccluded);
        }

        #[test]
        fn sway_hidden_target_on_another_workspace_is_not_occluded() {
            let state = SwayTreeState {
                focused: Some(30),
                focused_workspace: Some(10),
                focused_fullscreen: Some(30),
                target: Some((20, false, Some(11))),
            };
            assert_eq!(classify_sway_target(&state), TargetZ::Minimized);
        }

        #[test]
        fn cua_compositor_state_is_strict_and_pid_based() {
            assert_eq!(
                parse_cua_compositor_state("state 200 background_occluded\n")
                    .expect("valid compositor state"),
                CuaCompositorState {
                    focused_pid: Some(200),
                    target_z: TargetZ::BackgroundOccluded,
                }
            );
            assert_eq!(
                parse_cua_compositor_state("state 0 not_found")
                    .expect("zero means no focused client"),
                CuaCompositorState {
                    focused_pid: None,
                    target_z: TargetZ::NotFound,
                }
            );
            assert!(parse_cua_compositor_state("ok").is_err());
            assert!(parse_cua_compositor_state("state 1 unknown").is_err());
            assert!(parse_cua_compositor_state("state 1 foreground trailing").is_err());
        }

        #[test]
        fn gnome_json_and_focus_identity_are_compositor_derived() {
            let windows = parse_gnome_windows(gnome_payload()).expect("GNOME JSON");
            assert_eq!(windows.len(), 2);
            assert_eq!(gnome_focus_identity_from(&windows), Some(20));
            assert!(parse_gnome_windows(r#"('[{"pid":100}]',)"#).is_err());
        }

        #[test]
        fn gnome_only_full_higher_cover_occludes() {
            let mut windows = parse_gnome_windows(gnome_payload()).expect("GNOME JSON");
            let target = TargetWindow {
                pid: 100,
                native_id: 10,
            };
            assert_eq!(
                classify_gnome_target(&windows, target),
                TargetZ::BackgroundOccluded
            );
            windows[1].w = 200;
            assert_eq!(
                classify_gnome_target(&windows, target),
                TargetZ::BackgroundVisible
            );
            windows[1].w = 800;
            windows[1].visible = false;
            assert_eq!(
                classify_gnome_target(&windows, target),
                TargetZ::BackgroundVisible
            );
        }

        #[test]
        fn gnome_focused_and_minimized_are_direct() {
            let mut windows = parse_gnome_windows(gnome_payload()).expect("GNOME JSON");
            let target = TargetWindow {
                pid: 100,
                native_id: 10,
            };
            windows[0].focused = true;
            assert_eq!(classify_gnome_target(&windows, target), TargetZ::Foreground);
            windows[0].focused = false;
            windows[0].minimized = true;
            assert_eq!(classify_gnome_target(&windows, target), TargetZ::Minimized);
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::LinuxObserver as NativeObserver;
#[cfg(target_os = "macos")]
pub use macos::MacosObserver as NativeObserver;
#[cfg(target_os = "windows")]
pub use windows::WindowsObserver as NativeObserver;

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(foreground: u64, target_z: TargetZ, cursor: (f64, f64)) -> DesktopSnapshot {
        DesktopSnapshot {
            foreground: Some(foreground),
            input_focus: Some(foreground),
            target_z,
            cursor_pos: Some(cursor),
        }
    }

    #[test]
    fn transient_focus_change_fails_even_when_pre_and_post_match() {
        let before = snapshot(10, TargetZ::BackgroundOccluded, (100.0, 100.0));
        let after = before.clone();
        let delta = evaluate(
            ObserverCapabilities {
                focus: true,
                ..ObserverCapabilities::default()
            },
            &[OracleKind::Focus],
            before,
            after,
            DesktopJournal {
                focus_events: vec![
                    FocusEvent {
                        from: Some(10),
                        to: Some(20),
                    },
                    FocusEvent {
                        from: Some(20),
                        to: Some(10),
                    },
                ],
                leaked_input_events: Vec::new(),
            },
        );
        assert!(delta.passed().is_empty());
        assert!(delta.violations()[0].contains("transiently"));
    }

    #[test]
    fn input_focus_change_fails_when_foreground_is_stable() {
        let before = snapshot(10, TargetZ::BackgroundOccluded, (100.0, 100.0));
        let mut after = before.clone();
        after.input_focus = Some(20);
        let delta = evaluate(
            ObserverCapabilities {
                focus: true,
                ..ObserverCapabilities::default()
            },
            &[OracleKind::Focus],
            before,
            after,
            DesktopJournal::default(),
        );
        assert!(delta.passed().is_empty());
        assert!(delta.violations()[0].contains("input focus changed"));
    }

    #[test]
    fn target_raise_cursor_move_and_input_leak_are_independent_failures() {
        let delta = evaluate(
            ObserverCapabilities {
                focus: true,
                z_order: true,
                cursor: true,
                leaked_input: true,
            },
            &[
                OracleKind::Focus,
                OracleKind::ZOrder,
                OracleKind::Cursor,
                OracleKind::NoLeakedInput,
            ],
            snapshot(10, TargetZ::BackgroundOccluded, (100.0, 100.0)),
            snapshot(10, TargetZ::BackgroundVisible, (102.0, 100.0)),
            DesktopJournal {
                focus_events: Vec::new(),
                leaked_input_events: vec!["keydown:A".to_owned()],
            },
        );
        assert_eq!(delta.passed(), &[OracleKind::Focus]);
        assert_eq!(delta.violations().len(), 3);
    }

    #[test]
    fn unsupported_oracle_is_never_counted_as_passed() {
        let state = snapshot(10, TargetZ::BackgroundOccluded, (100.0, 100.0));
        let delta = evaluate(
            ObserverCapabilities::default(),
            &[OracleKind::NoLeakedInput],
            state.clone(),
            state,
            DesktopJournal::default(),
        );
        assert_eq!(delta.unsupported(), &[OracleKind::NoLeakedInput]);
        assert!(delta.ensure_supported().is_err());
        assert!(delta.passed().is_empty());
    }
}
