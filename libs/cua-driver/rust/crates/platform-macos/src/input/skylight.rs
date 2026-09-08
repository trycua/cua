//! SkyLight SPI bridge — Rust port of Swift's `SkyLightEventPost`.
//!
//! Two-layer story (matches Swift reference exactly):
//!
//! 1. **Post path** — `SLEventPostToPid` goes through `SLEventPostToPSN` →
//!    `CGSTickleActivityMonitor` → `SLSUpdateSystemActivityWithLocation` →
//!    `IOHIDPostEvent`. The public `CGEventPostToPid` skips the activity-monitor
//!    tickle so Chromium/Catalyst targets don't accept those events as live input.
//!
//! 2. **Authentication** (keyboard only) — on macOS 14+, WindowServer gates
//!    synthetic keyboard events on Chromium-like targets on an attached
//!    `SLSEventAuthenticationMessage`. We build one via the ObjC factory and
//!    attach it with `SLEventSetAuthenticationMessage` before posting.
//!
//! All symbols are resolved once at first use via `dlopen` + `dlsym`.
//! If anything fails to resolve the functions return `false` and callers
//! fall back to the public `CGEvent::post_to_pid`.

use libc::pid_t;
use std::ffi::{c_void, CStr};
use std::os::raw::{c_char, c_int, c_uint};
use std::sync::OnceLock;

// ── Function-pointer typedefs ──────────────────────────────────────────────

/// `void SLEventPostToPid(pid_t, CGEventRef)`
type PostToPidFn = unsafe extern "C" fn(pid_t, *mut c_void);

/// `void SLEventSetAuthenticationMessage(CGEventRef, id)`
type SetAuthMsgFn = unsafe extern "C" fn(*mut c_void, *mut c_void);

/// `void CGEventSetWindowLocation(CGEventRef, double x, double y)`
///
/// NOTE: CGPoint on 64-bit ARM/x86 is two f64 values packed consecutively.
/// We pass them as two separate f64 arguments which has identical ABI.
type SetWindowLocFn = unsafe extern "C" fn(*mut c_void, f64, f64);

/// `void SLEventSetIntegerValueField(CGEventRef, uint32_t field, int64_t value)`
type SetIntFieldFn = unsafe extern "C" fn(*mut c_void, u32, i64);

/// `uint32_t CGSMainConnectionID(void)`
type ConnectionIDFn = unsafe extern "C" fn() -> u32;

/// `uint64_t CGSGetActiveSpace(uint32_t cid)`
type GetActiveSpaceFn = unsafe extern "C" fn(u32) -> u64;

/// `CFArrayRef SLSCopySpacesForWindows(uint32_t cid, int selector, CFArrayRef windowIDs)`
type CopySpacesForWindowsFn = unsafe extern "C" fn(u32, i32, *const c_void) -> *mut c_void;

/// `CFStringRef SLSCopyManagedDisplayForWindow(uint32_t cid, uint32_t wid)`
type CopyManagedDisplayForWindowFn = unsafe extern "C" fn(u32, u32) -> *mut c_void;

/// `uint64_t SLSManagedDisplayGetCurrentSpace(uint32_t cid, CFStringRef display)`
type ManagedDisplayGetCurrentSpaceFn = unsafe extern "C" fn(u32, *const c_void) -> u64;

// ── NSMenu shortcut activation SPIs ──────────────────────────────────────────

/// `OSStatus SLPSSetFrontProcessWithOptions(const void *psn, uint32_t windowID, uint32_t options)`
type SetFrontProcessFn = unsafe extern "C" fn(*const c_void, u32, u32) -> i32;

/// `OSStatus SLSGetWindowOwner(uint32_t cid, uint32_t wid, uint32_t *out_cid)`
type GetWindowOwnerFn = unsafe extern "C" fn(u32, u32, *mut u32) -> i32;

/// `OSStatus SLSGetConnectionPSN(uint32_t cid, void *psn)`
type GetConnectionPSNFn = unsafe extern "C" fn(u32, *mut c_void) -> i32;

// ── Focus-without-raise SPIs ──────────────────────────────────────────────────

/// `OSStatus SLPSPostEventRecordTo(const void *psn, const uint8_t *bytes)`
/// Posts a 248-byte synthetic event record into the target process's Carbon
/// event queue. Build the buffer with bytes[0x04]=0xf8, bytes[0x08]=0x0d,
/// target window id at bytes 0x3c–0x3f (little-endian), focus/defocus marker
/// at bytes[0x8a] (0x01 = focus, 0x02 = defocus), all other bytes zero.
type PostEventRecordToFn = unsafe extern "C" fn(*const c_void, *const u8) -> i32;

/// `OSStatus _SLPSGetFrontProcess(void *psn)`
/// Writes the current frontmost process's 8-byte PSN into `psn`.
type GetFrontProcessFn = unsafe extern "C" fn(*mut c_void) -> i32;

/// `OSStatus GetProcessForPID(pid_t, void *psn)`
/// Deprecated but still resolves. Writes the target pid's 8-byte PSN.
type GetProcessForPIDFn = unsafe extern "C" fn(pid_t, *mut c_void) -> i32;

/// Factory: `+[SLSEventAuthenticationMessage messageWithEventRecord:pid:version:]`
/// ObjC send: `(id self, SEL _cmd, void* record, int32 pid, uint32 version) -> id`
type FactoryMsgSendFn = unsafe extern "C" fn(
    *mut c_void, // Class (receiver)
    *mut c_void, // SEL
    *mut c_void, // SLSEventRecord*
    c_int,       // pid
    c_uint,      // version
) -> *mut c_void;

// ── Symbol resolution ──────────────────────────────────────────────────────

/// Load SkyLight once so all dlsym lookups via RTLD_DEFAULT find it.
fn ensure_skylight_loaded() {
    static LOADED: OnceLock<()> = OnceLock::new();
    LOADED.get_or_init(|| {
        let path = b"/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight\0";
        unsafe {
            libc::dlopen(
                path.as_ptr() as *const c_char,
                libc::RTLD_LAZY | libc::RTLD_GLOBAL,
            );
        }
    });
}

/// Look up a symbol by name via RTLD_DEFAULT (after loading SkyLight).
/// Returns `None` when the symbol doesn't resolve.
fn find_sym(name: &[u8]) -> Option<*mut c_void> {
    ensure_skylight_loaded();
    let ptr = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr() as *const c_char) };
    if ptr.is_null() {
        None
    } else {
        Some(ptr)
    }
}

/// Reinterpret a raw symbol pointer as a function pointer of type `T`.
/// Safety: caller guarantees T matches the symbol's actual signature.
unsafe fn as_fn<T: Copy>(ptr: *mut c_void) -> T {
    std::mem::transmute_copy::<*mut c_void, T>(&ptr)
}

// ── Lazily-resolved handles ────────────────────────────────────────────────

fn post_to_pid_fn() -> Option<PostToPidFn> {
    static SYM: OnceLock<Option<PostToPidFn>> = OnceLock::new();
    *SYM.get_or_init(|| find_sym(b"SLEventPostToPid\0").map(|p| unsafe { as_fn(p) }))
}

fn set_auth_msg_fn() -> Option<SetAuthMsgFn> {
    static SYM: OnceLock<Option<SetAuthMsgFn>> = OnceLock::new();
    *SYM.get_or_init(|| find_sym(b"SLEventSetAuthenticationMessage\0").map(|p| unsafe { as_fn(p) }))
}

fn set_window_loc_fn() -> Option<SetWindowLocFn> {
    static SYM: OnceLock<Option<SetWindowLocFn>> = OnceLock::new();
    *SYM.get_or_init(|| find_sym(b"CGEventSetWindowLocation\0").map(|p| unsafe { as_fn(p) }))
}

fn set_int_field_fn() -> Option<SetIntFieldFn> {
    static SYM: OnceLock<Option<SetIntFieldFn>> = OnceLock::new();
    *SYM.get_or_init(|| find_sym(b"SLEventSetIntegerValueField\0").map(|p| unsafe { as_fn(p) }))
}

fn connection_id_fn() -> Option<ConnectionIDFn> {
    static SYM: OnceLock<Option<ConnectionIDFn>> = OnceLock::new();
    *SYM.get_or_init(|| find_sym(b"CGSMainConnectionID\0").map(|p| unsafe { as_fn(p) }))
}

fn get_active_space_fn() -> Option<GetActiveSpaceFn> {
    static SYM: OnceLock<Option<GetActiveSpaceFn>> = OnceLock::new();
    *SYM.get_or_init(|| {
        find_sym(b"SLSGetActiveSpace\0")
            .or_else(|| find_sym(b"CGSGetActiveSpace\0"))
            .map(|p| unsafe { as_fn(p) })
    })
}

fn copy_spaces_for_windows_fn() -> Option<CopySpacesForWindowsFn> {
    static SYM: OnceLock<Option<CopySpacesForWindowsFn>> = OnceLock::new();
    *SYM.get_or_init(|| {
        find_sym(b"SLSCopySpacesForWindows\0")
            .or_else(|| find_sym(b"CGSCopySpacesForWindows\0"))
            .map(|p| unsafe { as_fn(p) })
    })
}

fn copy_managed_display_for_window_fn() -> Option<CopyManagedDisplayForWindowFn> {
    static SYM: OnceLock<Option<CopyManagedDisplayForWindowFn>> = OnceLock::new();
    *SYM.get_or_init(|| {
        find_sym(b"SLSCopyManagedDisplayForWindow\0")
            .or_else(|| find_sym(b"CGSCopyManagedDisplayForWindow\0"))
            .map(|p| unsafe { as_fn(p) })
    })
}

fn managed_display_get_current_space_fn() -> Option<ManagedDisplayGetCurrentSpaceFn> {
    static SYM: OnceLock<Option<ManagedDisplayGetCurrentSpaceFn>> = OnceLock::new();
    *SYM.get_or_init(|| {
        find_sym(b"SLSManagedDisplayGetCurrentSpace\0")
            .or_else(|| find_sym(b"CGSManagedDisplayGetCurrentSpace\0"))
            .map(|p| unsafe { as_fn(p) })
    })
}

fn factory_msg_send_fn() -> Option<FactoryMsgSendFn> {
    static SYM: OnceLock<Option<FactoryMsgSendFn>> = OnceLock::new();
    *SYM.get_or_init(|| find_sym(b"objc_msgSend\0").map(|p| unsafe { as_fn(p) }))
}

fn set_front_process_fn() -> Option<SetFrontProcessFn> {
    static SYM: OnceLock<Option<SetFrontProcessFn>> = OnceLock::new();
    *SYM.get_or_init(|| find_sym(b"SLPSSetFrontProcessWithOptions\0").map(|p| unsafe { as_fn(p) }))
}

fn get_window_owner_fn() -> Option<GetWindowOwnerFn> {
    static SYM: OnceLock<Option<GetWindowOwnerFn>> = OnceLock::new();
    *SYM.get_or_init(|| find_sym(b"SLSGetWindowOwner\0").map(|p| unsafe { as_fn(p) }))
}

fn get_connection_psn_fn() -> Option<GetConnectionPSNFn> {
    static SYM: OnceLock<Option<GetConnectionPSNFn>> = OnceLock::new();
    *SYM.get_or_init(|| find_sym(b"SLSGetConnectionPSN\0").map(|p| unsafe { as_fn(p) }))
}

fn post_event_record_to_fn() -> Option<PostEventRecordToFn> {
    static SYM: OnceLock<Option<PostEventRecordToFn>> = OnceLock::new();
    *SYM.get_or_init(|| find_sym(b"SLPSPostEventRecordTo\0").map(|p| unsafe { as_fn(p) }))
}

fn get_front_process_fn() -> Option<GetFrontProcessFn> {
    static SYM: OnceLock<Option<GetFrontProcessFn>> = OnceLock::new();
    *SYM.get_or_init(|| find_sym(b"_SLPSGetFrontProcess\0").map(|p| unsafe { as_fn(p) }))
}

fn get_process_for_pid_fn() -> Option<GetProcessForPIDFn> {
    static SYM: OnceLock<Option<GetProcessForPIDFn>> = OnceLock::new();
    *SYM.get_or_init(|| find_sym(b"GetProcessForPID\0").map(|p| unsafe { as_fn(p) }))
}

/// `true` when `SLEventPostToPid` resolved.
pub fn is_available() -> bool {
    post_to_pid_fn().is_some()
}

/// `true` when the focus-without-raise SPIs resolved, including either the
/// modern window-owner PSN lookup or the deprecated pid fallback.
pub fn is_focus_without_raise_available() -> bool {
    let has_psn_lookup = (connection_id_fn().is_some()
        && get_window_owner_fn().is_some()
        && get_connection_psn_fn().is_some())
        || get_process_for_pid_fn().is_some();
    get_front_process_fn().is_some() && has_psn_lookup && post_event_record_to_fn().is_some()
}

// ── ObjC runtime helpers ───────────────────────────────────────────────────

/// Look up an ObjC class by C-string name via `objc_getClass`.
fn objc_class(name: &CStr) -> *mut c_void {
    type GetClassFn = unsafe extern "C" fn(*const c_char) -> *mut c_void;
    static SYM: OnceLock<Option<GetClassFn>> = OnceLock::new();
    let f = *SYM.get_or_init(|| find_sym(b"objc_getClass\0").map(|p| unsafe { as_fn(p) }));
    match f {
        Some(f) => unsafe { f(name.as_ptr()) },
        None => std::ptr::null_mut(),
    }
}

/// Register / look up an ObjC selector by C-string name via `sel_registerName`.
fn sel_register(name: &CStr) -> *mut c_void {
    type SelRegFn = unsafe extern "C" fn(*const c_char) -> *mut c_void;
    static SYM: OnceLock<Option<SelRegFn>> = OnceLock::new();
    let f = *SYM.get_or_init(|| find_sym(b"sel_registerName\0").map(|p| unsafe { as_fn(p) }));
    match f {
        Some(f) => unsafe { f(name.as_ptr()) },
        None => std::ptr::null_mut(),
    }
}

/// Whether `cls` actually implements `sel`, via `class_respondsToSelector`.
///
/// macOS 14 (Sonoma) compatibility guard: `SLSEventAuthenticationMessage`
/// exists on macOS 14, but `messageWithEventRecord:pid:version:` was only
/// added in macOS 15 (Sequoia). `sel_registerName` always succeeds (it just
/// interns the string), so a `!sel.is_null()` check is not enough — we must
/// confirm the class responds before calling `objc_msgSend`, or the runtime
/// raises `NSInvalidArgumentException: unrecognized selector`. See #1503.
fn class_responds_to_selector(cls: *mut c_void, sel: *mut c_void) -> bool {
    if cls.is_null() || sel.is_null() {
        return false;
    }
    type RespondsToFn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> bool;
    static SYM: OnceLock<Option<RespondsToFn>> = OnceLock::new();
    let f =
        *SYM.get_or_init(|| find_sym(b"class_respondsToSelector\0").map(|p| unsafe { as_fn(p) }));
    match f {
        Some(f) => unsafe { f(cls, sel) },
        None => false,
    }
}

// ── SLSEventRecord extraction ──────────────────────────────────────────────

/// Extract the embedded `SLSEventRecord *` from a `CGEvent`.
///
/// Layout of `__CGEvent` (SkyLight ObjC type encodings):
///   `{CFRuntimeBase, uint32_t, SLSEventRecord *}`
/// On 64-bit: CFRuntimeBase=16, uint32=4, 4 bytes pad → record pointer at offset 24.
/// We probe offsets 24, 32, 16 for resilience across OS versions (same as Swift).
unsafe fn extract_event_record(event_ptr: *mut c_void) -> *mut c_void {
    for &offset in &[24usize, 32, 16] {
        let slot = (event_ptr as *const u8).add(offset).cast::<*mut c_void>();
        let p = std::ptr::read_unaligned(slot);
        if !p.is_null() {
            return p;
        }
    }
    std::ptr::null_mut()
}

// ── Public entry points ────────────────────────────────────────────────────

/// Post `event_ptr` (raw `CGEventRef`) to `pid` via `SLEventPostToPid`.
///
/// `attach_auth_message`: pass `true` for keyboard events (Chromium path),
/// `false` for mouse events (see Swift doc comment on `postToPid`).
///
/// Returns `true` when `SLEventPostToPid` resolved and the post was attempted.
/// Returns `false` when the SPI is absent — caller falls back to `CGEvent::post_to_pid`.
pub(super) fn post_to_pid(pid: pid_t, event_ptr: *mut c_void, attach_auth_message: bool) -> bool {
    let post_fn = match post_to_pid_fn() {
        Some(f) => f,
        None => return false,
    };

    if attach_auth_message {
        // Build and attach SLSEventAuthenticationMessage.
        //
        // macOS 14 (Sonoma) compatibility: the class exists on macOS 14 but
        // `messageWithEventRecord:pid:version:` was added in macOS 15. Guard
        // with `class_respondsToSelector` (a `!sel.is_null()` check is not
        // enough — `sel_registerName` interns any name); when the selector is
        // absent we skip the auth envelope and fall through to the plain
        // `SLEventPostToPid` below. Chromium-class targets may not receive the
        // event on macOS 14, but the daemon no longer crashes. See #1503.
        let cls = objc_class(c"SLSEventAuthenticationMessage");
        let sel = sel_register(c"messageWithEventRecord:pid:version:");
        let factory = factory_msg_send_fn();

        if class_responds_to_selector(cls, sel) {
            if let Some(factory_fn) = factory {
                let record = unsafe { extract_event_record(event_ptr) };
                if !record.is_null() {
                    let msg = unsafe { factory_fn(cls, sel, record, pid as c_int, 0u32) };
                    if !msg.is_null() {
                        if let Some(set_auth) = set_auth_msg_fn() {
                            unsafe { set_auth(event_ptr, msg) };
                        }
                    }
                }
            }
        }
    }

    unsafe { post_fn(pid, event_ptr) };
    true
}

/// Stamp a window-local `(x, y)` point onto `event_ptr` via the private
/// `CGEventSetWindowLocation` SPI. Returns `true` when the SPI resolved.
pub(super) fn set_window_location(event_ptr: *mut c_void, x: f64, y: f64) -> bool {
    match set_window_loc_fn() {
        Some(f) => {
            unsafe { f(event_ptr, x, y) };
            true
        }
        None => false,
    }
}

/// Stamp `value` onto `event_ptr` at raw SkyLight field index `field` via
/// `SLEventSetIntegerValueField`. Returns `false` when SPI absent.
pub(super) fn set_integer_field(event_ptr: *mut c_void, field: u32, value: i64) -> bool {
    match set_int_field_fn() {
        Some(f) => {
            unsafe { f(event_ptr, field, value) };
            true
        }
        None => false,
    }
}

/// Return the Skylight main connection ID for the current process.
pub fn main_connection_id() -> Option<u32> {
    connection_id_fn().map(|f| unsafe { f() })
}

/// Return the current active macOS Space (desktop) ID.
///
/// Uses the private `CGSGetActiveSpace` SPI from SkyLight.  Returns `None`
/// when the symbol is unavailable (future macOS version that removes it).
pub fn get_active_space() -> Option<u64> {
    let cid = main_connection_id()?;
    nonzero_space_id(get_active_space_fn().map(|f| unsafe { f(cid) }))
}

fn nonzero_space_id(space_id: Option<u64>) -> Option<u64> {
    space_id.filter(|id| *id != 0)
}

/// A consistent WindowServer connection and active-Space snapshot for one
/// enumeration. Space membership must be queried one window at a time:
/// `SLSCopySpacesForWindows` returns the set union for its input window list,
/// not a positionally aligned result.
pub(crate) struct SpaceQuery {
    connection_id: u32,
    current_space_id: Option<u64>,
    copy_spaces_for_windows: Option<CopySpacesForWindowsFn>,
    copy_managed_display_for_window: Option<CopyManagedDisplayForWindowFn>,
    managed_display_get_current_space: Option<ManagedDisplayGetCurrentSpaceFn>,
}

impl SpaceQuery {
    pub(crate) fn new() -> Option<Self> {
        let connection_id = main_connection_id()?;
        let current_space_id =
            nonzero_space_id(get_active_space_fn().map(|f| unsafe { f(connection_id) }));
        Some(Self {
            connection_id,
            current_space_id,
            copy_spaces_for_windows: copy_spaces_for_windows_fn(),
            copy_managed_display_for_window: copy_managed_display_for_window_fn(),
            managed_display_get_current_space: managed_display_get_current_space_fn(),
        })
    }

    pub(crate) fn current_space_id(&self) -> Option<u64> {
        self.current_space_id
    }

    /// Return every Space containing `window_id`.
    pub(crate) fn window_space_ids(&self, window_id: u32) -> Option<Vec<u64>> {
        use core_foundation::{
            array::CFArray,
            base::{CFGetTypeID, CFTypeRef, TCFType},
            number::CFNumber,
        };

        let copy_spaces = self.copy_spaces_for_windows?;
        let window_number = CFNumber::from(window_id as i64);
        let window_ref = window_number.as_concrete_TypeRef() as *const c_void;
        let windows = CFArray::<CFTypeRef>::from_copyable(&[window_ref]);
        let result_ptr = unsafe {
            copy_spaces(
                self.connection_id,
                0x7,
                windows.as_concrete_TypeRef() as *const c_void,
            )
        };
        if result_ptr.is_null() {
            return None;
        }

        let result: CFArray<CFTypeRef> =
            unsafe { CFArray::wrap_under_create_rule(result_ptr as _) };
        let mut space_ids = Vec::with_capacity(result.len() as usize);
        for item in result.iter() {
            let item = *item;
            if unsafe { CFGetTypeID(item) } != CFNumber::type_id() {
                return None;
            }
            let number = unsafe { CFNumber::wrap_under_get_rule(item as _) };
            let space_id = u64::try_from(number.to_i64()?).ok()?;
            if space_id != 0 {
                space_ids.push(space_id);
            }
        }

        (!space_ids.is_empty()).then_some(space_ids)
    }

    /// Return the active Space on the display WindowServer associates with
    /// `window_id`. This avoids comparing every window against the main
    /// display's active Space when displays use independent Spaces.
    pub(crate) fn current_space_for_window(&self, window_id: u32) -> Option<u64> {
        use core_foundation::{base::TCFType, string::CFString};

        let copy_display = self.copy_managed_display_for_window?;
        let get_current_space = self.managed_display_get_current_space?;
        let display_ptr = unsafe { copy_display(self.connection_id, window_id) };
        if display_ptr.is_null() {
            return None;
        }
        let display = unsafe { CFString::wrap_under_create_rule(display_ptr as _) };
        nonzero_space_id(Some(unsafe {
            get_current_space(
                self.connection_id,
                display.as_concrete_TypeRef() as *const c_void,
            )
        }))
    }
}

// ── Focus-without-raise ───────────────────────────────────────────────────────

/// Activate `target_pid`'s window `target_wid` without raising any windows
/// or triggering Space-follow. Ported from yabai's
/// `window_manager_focus_window_without_raise`.
///
/// Recipe:
/// 1. `_SLPSGetFrontProcess` → capture current front PSN.
/// 2. `SLSGetWindowOwner + SLSGetConnectionPSN` → target PSN, with
///    `GetProcessForPID(target_pid)` as an older-system fallback.
/// 3. Post 248-byte defocus record to front PSN (`bytes[0x8a] = 0x02`).
/// 4. Post 248-byte focus record to target PSN (`bytes[0x8a] = 0x01`,
///    `bytes[0x3c..0x3f]` = `target_wid` little-endian).
///
/// Deliberately skips `SLPSSetFrontProcessWithOptions` — see the Swift
/// reference `FocusWithoutRaise.swift` for why omitting it keeps
/// Chromium's user-activation gate open.
///
/// Returns `true` when all SPIs resolved and both posts succeeded.
pub fn activate_without_raise(target_pid: pid_t, target_wid: u32) -> bool {
    let post_fn = match post_event_record_to_fn() {
        Some(f) => f,
        None => return false,
    };
    let get_front = match get_front_process_fn() {
        Some(f) => f,
        None => return false,
    };
    // 8-byte PSN buffers (two UInt32s).
    let mut prev_psn = [0u8; 8];
    let mut target_psn = [0u8; 8];

    let ok_prev = unsafe { get_front(prev_psn.as_mut_ptr() as *mut c_void) } == 0;
    if !ok_prev {
        return false;
    }

    if !get_process_psn_for_window(target_wid, target_pid, &mut target_psn) {
        return false;
    }

    // Build the 248-byte event buffer.
    let mut buf = [0u8; 0xF8];
    buf[0x04] = 0xF8;
    buf[0x08] = 0x0D;
    // Stamp target window id in little-endian at bytes 0x3c–0x3f.
    buf[0x3C] = (target_wid & 0xFF) as u8;
    buf[0x3D] = ((target_wid >> 8) & 0xFF) as u8;
    buf[0x3E] = ((target_wid >> 16) & 0xFF) as u8;
    buf[0x3F] = ((target_wid >> 24) & 0xFF) as u8;

    // Step 3: defocus previous front.
    buf[0x8A] = 0x02;
    let defocus_ok = unsafe { post_fn(prev_psn.as_ptr() as *const c_void, buf.as_ptr()) == 0 };

    // Step 4: focus target.
    buf[0x8A] = 0x01;
    let focus_ok = unsafe { post_fn(target_psn.as_ptr() as *const c_void, buf.as_ptr()) == 0 };

    defocus_ok && focus_ok
}

// ── NSMenu shortcut activation ────────────────────────────────────────────────

/// Gets the PSN for the process that owns `window_id`.
/// Uses `CGSMainConnectionID` + `SLSGetWindowOwner` + `SLSGetConnectionPSN`.
/// Falls back to `GetProcessForPID(pid)` when the SkyLight path fails.
pub fn get_process_psn_for_window(window_id: u32, pid: libc::pid_t, out_psn: &mut [u8; 8]) -> bool {
    // Try modern path: CGSMainConnectionID → SLSGetWindowOwner → SLSGetConnectionPSN
    if let (Some(get_owner), Some(get_psn), Some(conn_id_fn)) = (
        get_window_owner_fn(),
        get_connection_psn_fn(),
        connection_id_fn(),
    ) {
        let main_cid = unsafe { conn_id_fn() };
        let mut owner_cid: u32 = 0;
        let ok = unsafe { get_owner(main_cid, window_id, &mut owner_cid) } == 0;
        if ok && owner_cid != 0 {
            let psn_ok = unsafe { get_psn(owner_cid, out_psn.as_mut_ptr() as *mut c_void) } == 0;
            if psn_ok {
                return true;
            }
        }
    }
    // Fallback: GetProcessForPID
    if let Some(get_pid_psn) = get_process_for_pid_fn() {
        return unsafe { get_pid_psn(pid, out_psn.as_mut_ptr() as *mut c_void) } == 0;
    }
    false
}

/// Return whether WindowServer currently considers the exact window's process
/// frontmost. Unlike `NSWorkspace.frontmostApplication`, this query does not
/// depend on the caller's AppKit run loop processing an activation update.
pub fn front_process_matches(target_pid: libc::pid_t, target_wid: u32) -> Option<bool> {
    let get_front = get_front_process_fn()?;
    let mut front_psn = [0u8; 8];
    if unsafe { get_front(front_psn.as_mut_ptr() as *mut c_void) } != 0 {
        return None;
    }
    let mut target_psn = [0u8; 8];
    if !get_process_psn_for_window(target_wid, target_pid, &mut target_psn) {
        return None;
    }
    Some(front_psn == target_psn)
}

/// Make `target_pid` and `target_wid` WindowServer-frontmost and leave them
/// there. Unlike [`with_foreground_assist`], this deliberately does not save or
/// restore the previous process. It is the persistent counterpart required by
/// focus-proxy surfaces whose input channel is armed only while genuinely
/// frontmost.
///
/// Returns `true` only when the target PSN resolved and WindowServer accepted
/// `SLPSSetFrontProcessWithOptions`.
pub fn set_front_process_persistently(target_pid: libc::pid_t, target_wid: u32) -> bool {
    let Some(set_front) = set_front_process_fn() else {
        return false;
    };
    let mut target_psn = [0u8; 8];
    if !get_process_psn_for_window(target_wid, target_pid, &mut target_psn) {
        return false;
    }

    // kCPSNoWindows = 0x400. Supplying the exact target window still makes
    // that window's process frontmost while avoiding a broad all-window raise.
    unsafe { set_front(target_psn.as_ptr() as *const c_void, target_wid, 0x400) == 0 }
}

fn make_key_window_record(window_id: u32, event_kind: u8) -> [u8; 0xF8] {
    let mut record = [0u8; 0xF8];
    record[0x04] = 0xF8;
    record[0x08] = event_kind;
    record[0x3A] = 0x10;
    record[0x3C..0x40].copy_from_slice(&window_id.to_le_bytes());
    record[0x20..0x30].fill(0xFF);
    record
}

/// Make one exact application window native-key and frontmost.
///
/// Accessibility's `AXFocusedWindow` can change without AppKit making the
/// corresponding `NSWindow` key. Native menu validation observes the latter,
/// so focus-sensitive commands remain disabled in that split state. This is
/// the bounded exact-window sequence used by established macOS window tools:
/// mark the front-process request as user generated, synthesize the paired
/// make-key records for the requested WindowServer id, then let the caller
/// raise the matching AX window. No other application window is addressed.
pub fn make_exact_window_key(target_pid: libc::pid_t, target_wid: u32) -> bool {
    let Some(set_front) = set_front_process_fn() else {
        return false;
    };
    let Some(post) = post_event_record_to_fn() else {
        return false;
    };
    let mut target_psn = [0u8; 8];
    if !get_process_psn_for_window(target_wid, target_pid, &mut target_psn) {
        return false;
    }

    // kCPSUserGenerated = 0x200. Unlike kCPSNoWindows, this permits AppKit to
    // establish the requested native key window before it validates NSMenu.
    if unsafe { set_front(target_psn.as_ptr() as *const c_void, target_wid, 0x200) } != 0 {
        return false;
    }
    for event_kind in [0x01, 0x02] {
        let record = make_key_window_record(target_wid, event_kind);
        if unsafe { post(target_psn.as_ptr() as *const c_void, record.as_ptr()) } != 0 {
            return false;
        }
    }
    true
}

/// Tool-agnostic foreground-assist: briefly front `window_id`, wait for the
/// activation to actually land, run `body` (which posts the synthetic input),
/// then restore the prior frontmost process.
///
/// This is the `delivery_mode:"foreground"` rung of the best-effort-background
/// ladder, shared by `type_text` and `click`. Reached only when the agent has
/// seen the background rungs fail (clicks) or the field is unverifiable +
/// focus-sensitive (Catalyst typing).
///
/// ## Why this does not delegate to [`with_menu_shortcut_activation`]
///
/// It used to. That helper posts `set_front` and calls `action` immediately,
/// which is correct for its own purpose: NSMenu key dispatch only needs the key
/// event *enqueued* in the target's run-loop queue, so first-responder identity
/// is irrelevant and the sub-millisecond front → act → restore is a feature.
///
/// Input delivery has the opposite requirement. `set_front` is asynchronous, so
/// running the body straight away means the body's `AXFocused` write races
/// AppKit's own activation. AppKit wins: when activation completes it installs
/// the window's remembered first responder and clobbers the write. The
/// keystrokes then land wherever the app chose — for a Catalyst app such as
/// WhatsApp, the message list rather than the composer, which silently scrolls
/// the transcript instead of typing.
///
/// So this waits for the activation to be observable before running `body`, the
/// same ordering [`with_foreground_hid_activation`] already relies on. The wait
/// is a bounded poll rather than a fixed sleep: an app that activates in 10 ms
/// pays 10 ms, and a slow Catalyst/RDP surface still gets its full budget.
///
/// Returns `Ok(true)` when the brief activation happened, `Ok(false)` when the
/// fronting SPIs are unavailable (the body still ran, just without a front).
pub fn with_foreground_assist(
    target_pid: libc::pid_t,
    target_wid: u32,
    body: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<bool> {
    let set_front = match set_front_process_fn() {
        Some(f) => f,
        None => {
            // SPIs unavailable — run body anyway without activation.
            body()?;
            return Ok(false);
        }
    };

    let mut prev_psn = [0u8; 8];
    let prev_ok = get_front_process_fn()
        .map(|f| unsafe { f(prev_psn.as_mut_ptr() as *mut c_void) } == 0)
        .unwrap_or(false);

    let mut target_psn = [0u8; 8];
    if !get_process_psn_for_window(target_wid, target_pid, &mut target_psn) {
        body()?;
        return Ok(false);
    }

    unsafe { set_front(target_psn.as_ptr() as *const c_void, target_wid, 0x400) };
    // `set_front` moves WindowServer's front process but does not make the
    // target's NSWindow key, and AppKit installs a first responder only for a
    // key window. Without this the app is "frontmost" to WindowServer while
    // remaining, from AppKit's point of view, unfocused — so the AXFocused
    // write in the body has no responder chain to attach to.
    make_exact_window_key(target_pid, target_wid);
    await_window_focused(target_pid, target_wid);

    let result = body();

    if prev_ok {
        unsafe { set_front(prev_psn.as_ptr() as *const c_void, 0, 0x400) };
    }

    result?;
    Ok(true)
}

/// Upper bound on how long [`with_foreground_assist`] waits for a requested
/// activation to become observable. Chosen to cover a Catalyst app's activation
/// plus key-window install; past this the caller proceeds anyway so a stubborn
/// target degrades to the old behavior instead of hanging.
const ACTIVATION_WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(400);

/// Poll interval for [`await_window_focused`]. Short enough that a fast native
/// app pays roughly one tick, long enough not to spin on the WindowServer.
const ACTIVATION_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

/// Block until `target_wid` is the application's focused AX window, or the
/// timeout expires. Returns whether that state was observed.
///
/// The predicate is deliberately `AXFocusedWindow` and not
/// `NSWorkspace.frontmostApplication`. The latter does not observe a
/// SkyLight-level front-process change at all: polling it every 15ms across a
/// full foreground `type_text` against WhatsApp showed zero transitions while
/// the target was demonstrably being fronted, so a frontmost-based wait always
/// burns its whole timeout and never actually gates on anything. `AXFocusedWindow`
/// is the same proof [`preserves_exact_existing_focus`] already trusts to decide
/// whether a window is focused.
///
/// A `false` return is not fatal: the caller proceeds with delivery regardless,
/// because a target that never reports focus is exactly the case the pre-existing
/// best-effort contract already covered.
fn await_window_focused(pid: libc::pid_t, window_id: u32) -> bool {
    let deadline = std::time::Instant::now() + ACTIVATION_WAIT_TIMEOUT;
    loop {
        if crate::ax::bindings::focused_window_id_of_pid(pid) == Some(window_id) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(ACTIVATION_POLL_INTERVAL);
    }
}

/// Activate an exact target window for a global HID keyboard action.
///
/// Unlike [`with_menu_shortcut_activation`], this helper must not run `action`
/// when the private foreground SPI is unavailable: a global HID event has no
/// pid addressing and would otherwise land in whichever application is
/// currently frontmost. The short settles keep the target frontmost until
/// WindowServer has routed both sides of the key chord, then restore the prior
/// process even when the action fails.
pub fn with_foreground_hid_activation(
    target_pid: libc::pid_t,
    target_wid: u32,
    action: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let set_front = set_front_process_fn()
        .ok_or_else(|| anyhow::anyhow!("foreground HID delivery is unavailable"))?;

    let mut prev_psn = [0u8; 8];
    let prev_ok = get_front_process_fn()
        .map(|f| unsafe { f(prev_psn.as_mut_ptr() as *mut c_void) } == 0)
        .unwrap_or(false);

    let mut target_psn = [0u8; 8];
    if !get_process_psn_for_window(target_wid, target_pid, &mut target_psn) {
        anyhow::bail!("could not resolve target window for foreground HID delivery");
    }

    let focused_window_id = crate::ax::bindings::focused_window_id_of_pid(target_pid);
    if preserves_exact_existing_focus(prev_ok, prev_psn, target_psn, focused_window_id, target_wid)
    {
        // Re-activating an already key exact window can clear Chromium's
        // renderer focus even though WindowServer keeps the app frontmost.
        // The AX window proof lets us deliver directly without weakening the
        // exact-window guard or disturbing the current key target.
        return action();
    }

    let activated = unsafe { set_front(target_psn.as_ptr() as *const c_void, target_wid, 0x400) };
    if activated != 0 {
        anyhow::bail!("WindowServer rejected foreground HID activation");
    }

    make_exact_window_key(target_pid, target_wid);
    if !await_window_focused(target_pid, target_wid) {
        if prev_ok {
            unsafe { set_front(prev_psn.as_ptr() as *const c_void, 0, 0x400) };
        }
        anyhow::bail!("exact target window did not become focused for foreground HID delivery");
    }

    let result = action();
    std::thread::sleep(std::time::Duration::from_millis(40));

    if prev_ok {
        unsafe { set_front(prev_psn.as_ptr() as *const c_void, 0, 0x400) };
    }

    result
}

fn preserves_exact_existing_focus(
    previous_process_known: bool,
    previous_psn: [u8; 8],
    target_psn: [u8; 8],
    focused_window_id: Option<u32>,
    target_window_id: u32,
) -> bool {
    previous_process_known
        && previous_psn == target_psn
        && focused_window_id == Some(target_window_id)
}

/// Activate `target_pid`'s window `target_wid` for NSMenu key dispatch, run `action`,
/// then immediately restore the prior frontmost process.
///
/// The entire activate → action → restore sequence is < 1 ms — a 5 ms UX monitor
/// never observes the intermediate frontmost state. NSMenu still fires because the
/// key event is already enqueued in the target's run-loop queue before we restore.
///
/// Returns `Ok(true)` when activation succeeded, `Ok(false)` when SPIs unavailable.
pub fn with_menu_shortcut_activation(
    target_pid: libc::pid_t,
    target_wid: u32,
    action: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<bool> {
    let set_front = match set_front_process_fn() {
        Some(f) => f,
        None => {
            // SPIs unavailable — run action anyway without activation.
            action()?;
            return Ok(false);
        }
    };

    // Capture prior frontmost PSN.
    let mut prev_psn = [0u8; 8];
    let prev_ok = get_front_process_fn()
        .map(|f| unsafe { f(prev_psn.as_mut_ptr() as *mut c_void) } == 0)
        .unwrap_or(false);

    // Resolve target PSN.
    let mut target_psn = [0u8; 8];
    let target_ok = get_process_psn_for_window(target_wid, target_pid, &mut target_psn);
    if !target_ok {
        action()?;
        return Ok(false);
    }

    // Make target WindowServer-frontmost (kCPSNoWindows = 0x400).
    unsafe { set_front(target_psn.as_ptr() as *const c_void, target_wid, 0x400) };

    // Run action then restore — even if action fails.
    let result = action();

    // Restore prior frontmost (windowID=0, options=0x400).
    if prev_ok {
        unsafe { set_front(prev_psn.as_ptr() as *const c_void, 0, 0x400) };
    }

    result?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::{make_key_window_record, preserves_exact_existing_focus};

    #[test]
    fn make_key_records_address_only_the_exact_window() {
        let press = make_key_window_record(0x7856_3412, 0x01);
        let release = make_key_window_record(0x7856_3412, 0x02);
        assert_eq!(press.len(), 0xF8);
        assert_eq!(press[0x04], 0xF8);
        assert_eq!(press[0x08], 0x01);
        assert_eq!(release[0x08], 0x02);
        assert_eq!(&press[0x3C..0x40], &[0x12, 0x34, 0x56, 0x78]);
        assert_eq!(press[0x3A], 0x10);
        assert!(press[0x20..0x30].iter().all(|byte| *byte == 0xFF));
    }

    #[test]
    fn exact_existing_focus_avoids_reactivation() {
        let psn = [1, 2, 3, 4, 5, 6, 7, 8];
        assert!(preserves_exact_existing_focus(true, psn, psn, Some(42), 42));
    }

    #[test]
    fn process_or_window_uncertainty_requires_guarded_activation() {
        let target = [1, 2, 3, 4, 5, 6, 7, 8];
        let other = [8, 7, 6, 5, 4, 3, 2, 1];
        assert!(!preserves_exact_existing_focus(
            false,
            target,
            target,
            Some(42),
            42
        ));
        assert!(!preserves_exact_existing_focus(
            true,
            other,
            target,
            Some(42),
            42
        ));
        assert!(!preserves_exact_existing_focus(
            true,
            target,
            target,
            Some(41),
            42
        ));
        assert!(!preserves_exact_existing_focus(
            true, target, target, None, 42
        ));
    }
}
