//! Runtime reproduction + regression guard for the UIA element-cache
//! use-after-free fixed in d95b89a1 (retain-under-lock / `RetainedElement`).
//!
//! THE RACE (what these tests model):
//!   Two concurrent sessions drive the same (pid, hwnd).
//!   - Session A: `get_window_state` → `ElementCache::update` → `core.insert`
//!     replaces the snapshot → old `CachedSnapshot::drop` → COM `Release` on
//!     every cached `IUIAutomationElement`.
//!   - Session B: `click` / `type_text` / `set_value` looked the element up out
//!     of the cache and is mid-action, dereferencing the same COM pointer.
//!   With the PRE-FIX accessor (`get_element_ptr`, a bare `usize` copy with no
//!   AddRef) B's pointer can be `Release`d to zero by A between the lookup and
//!   the dereference → use-after-free → daemon crash. This is the Windows
//!   analogue of the macOS #1796 `AXUIElementCopyActionNames` fault.
//!
//! HOW WE REPRODUCE IT DETERMINISTICALLY WITHOUT A GUI:
//!   The cache only ever calls IUnknown vtable slots on the cached pointers
//!   (`AddRef` via `clone()`, `Release` via `drop`). So we feed it a real,
//!   independently-refcounted COM-ABI object of our own (`FakeObj`) with a
//!   hand-rolled IUnknown vtable. The cache's real `with_snapshot` mutex, real
//!   `CachedSnapshot::drop`, and the real `get_element_retained` AddRef-under-
//!   lock all run unchanged against it.
//!
//!   `FakeObj` is instrumented two ways:
//!     - logical: an AddRef/Release seen while the refcount is already <= 0
//!       bumps a shared `uaf_hits` counter (a use-after-free that a sanitizer
//!       would flag) — memory stays mapped so the assertion is deterministic
//!       and the test process never actually faults.
//!     - poison: on Release-to-zero we overwrite the object's vtable pointer
//!       with garbage, so the *next* AddRef dereferences it and the process
//!       takes a real STATUS_ACCESS_VIOLATION — the dramatic "old code crashes"
//!       demonstration. Gated behind `#[ignore]` so it never aborts the suite.
//!
//! `force_interleave` pins the dangerous ordering (B looks up → A replaces +
//! releases → B dereferences) with channels instead of relying on luck, so
//! BEFORE deterministically trips and AFTER deterministically survives.

use super::{CacheKey, CachedSnapshot, ElementCache, SnapshotKind};
use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use windows::core::{Interface, GUID, HRESULT};
use windows::Win32::UI::Accessibility::IUIAutomationElement;

#[repr(C)]
struct FakeVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
}

#[repr(C)]
struct FakeObj {
    /// MUST be first: IUnknown ABI. The cache reads `*ptr` as the vtable.
    vtbl: *const FakeVtbl,
    refcount: AtomicIsize,
    /// Shared counter: bumped whenever AddRef/Release touches us while our
    /// refcount is already <= 0 — i.e. a use-after-free.
    uaf_hits: *const AtomicUsize,
    /// When true, Release-to-zero poisons `vtbl` so the next vtable call faults.
    poison_on_zero: bool,
}

unsafe extern "system" fn fake_query_interface(
    _this: *mut c_void,
    _riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if !ppv.is_null() {
        *ppv = std::ptr::null_mut();
    }
    HRESULT(0x8000_4002u32 as i32) // E_NOINTERFACE
}

unsafe extern "system" fn fake_add_ref(this: *mut c_void) -> u32 {
    let obj = this as *mut FakeObj;
    let prev = (*obj).refcount.load(Ordering::SeqCst);
    if prev <= 0 {
        (*(*obj).uaf_hits).fetch_add(1, Ordering::SeqCst);
    }
    (*obj).refcount.fetch_add(1, Ordering::SeqCst);
    (prev + 1).max(1) as u32
}

unsafe extern "system" fn fake_release(this: *mut c_void) -> u32 {
    let obj = this as *mut FakeObj;
    let prev = (*obj).refcount.fetch_sub(1, Ordering::SeqCst);
    if prev <= 0 {
        (*(*obj).uaf_hits).fetch_add(1, Ordering::SeqCst);
    }
    let now = prev - 1;
    if now == 0 && (*obj).poison_on_zero {
        // Overwrite the vtable pointer in place. The allocation stays mapped,
        // but the next AddRef will read this garbage and call through it.
        let vtbl_slot = std::ptr::addr_of_mut!((*obj).vtbl) as *mut usize;
        vtbl_slot.write(0xD15E_A5ED_DEAD_BEEFusize);
    }
    now.max(0) as u32
}

static VTBL: FakeVtbl = FakeVtbl {
    query_interface: fake_query_interface,
    add_ref: fake_add_ref,
    release: fake_release,
};

/// Allocate a fake COM object with refcount 1 (the single ref the cache "owns",
/// matching the walker's clone()+forget() hand-off). Leaked on purpose: the
/// logical tests must keep the memory mapped to observe post-free touches; the
/// poison test crashes before cleanup would matter.
fn make_fake(uaf_hits: &'static AtomicUsize, poison_on_zero: bool) -> usize {
    let obj = Box::new(FakeObj {
        vtbl: &VTBL as *const FakeVtbl,
        refcount: AtomicIsize::new(1),
        uaf_hits: uaf_hits as *const AtomicUsize,
        poison_on_zero,
    });
    Box::into_raw(obj) as usize
}

fn snapshot_with(ptrs: Vec<usize>) -> CachedSnapshot {
    let n = ptrs.len();
    CachedSnapshot {
        kind: SnapshotKind::Uia,
        elements: ptrs,
        centers: vec![(0, 0); n],
        rects: vec![None; n],
        msaa_roles: vec![None; n],
    }
}

/// Pre-fix accessor, restored verbatim: a bare `usize` copy with NO AddRef
/// under the lock. This is exactly the `get_element_ptr` body deleted by
/// d95b89a1 — the vulnerable path.
fn old_get_element_ptr(cache: &ElementCache, pid: u32, hwnd: u64, idx: usize) -> Option<usize> {
    cache
        .core
        .with_snapshot(&CacheKey { pid, hwnd }, |s| s.elements.get(idx).copied())
        .flatten()
}

/// The mid-action dereference a tool performs: reconstruct the interface from
/// the raw pointer and touch its vtable (AddRef then Release). On a live object
/// this is harmless; on a freed/poisoned one it is the use-after-free.
unsafe fn touch_vtable(ptr: usize) {
    let elem: IUIAutomationElement = IUIAutomationElement::from_raw(ptr as *mut c_void);
    let dup = elem.clone(); // AddRef — reads the vtable
    std::mem::forget(elem); // don't Release the borrowed cache ref
    drop(dup); // Release — reads the vtable again
}

const PID: u32 = 4242;
const HWND: u64 = 0x1234;

/// Forced interleave: B looks up the element, THEN A replaces+releases the
/// snapshot, THEN B dereferences. `use_retained` selects the fixed path
/// (`get_element_retained`, AddRef under lock) vs the pre-fix bare path.
/// Returns the number of use-after-free touches observed.
fn run_forced_interleave(use_retained: bool, poison_on_zero: bool) -> usize {
    let uaf_hits: &'static AtomicUsize = Box::leak(Box::new(AtomicUsize::new(0)));
    let cache = Arc::new(ElementCache::new());

    let ptr = make_fake(uaf_hits, poison_on_zero);
    cache.core.insert(
        CacheKey {
            pid: PID,
            hwnd: HWND,
        },
        snapshot_with(vec![ptr]),
    );

    let (b_looked_up_tx, b_looked_up_rx) = mpsc::channel::<()>();
    let (a_replaced_tx, a_replaced_rx) = mpsc::channel::<()>();

    let cache_b = cache.clone();
    let worker_b = thread::spawn(move || {
        if use_retained {
            // FIXED path: AddRef under the lock; guard pins the object alive.
            let guard = cache_b
                .get_element_retained(PID, HWND, 0)
                .expect("element present");
            b_looked_up_tx.send(()).unwrap();
            a_replaced_rx.recv().unwrap(); // A has now replaced + released
            unsafe { touch_vtable(guard.as_ptr()) }; // safe: guard holds +1
            drop(guard);
        } else {
            // PRE-FIX path: bare pointer, no AddRef.
            let raw = old_get_element_ptr(&cache_b, PID, HWND, 0).expect("element present");
            b_looked_up_tx.send(()).unwrap();
            a_replaced_rx.recv().unwrap(); // A has now replaced + released → freed
            unsafe { touch_vtable(raw) }; // USE-AFTER-FREE
        }
    });

    let cache_a = cache.clone();
    let worker_a = thread::spawn(move || {
        b_looked_up_rx.recv().unwrap();
        // get_window_state on the same (pid, hwnd): replace the snapshot. The
        // old snapshot's Drop fires COM Release on `ptr`.
        cache_a.core.insert(
            CacheKey {
                pid: PID,
                hwnd: HWND,
            },
            snapshot_with(vec![]),
        );
        a_replaced_tx.send(()).unwrap();
    });

    worker_a.join().unwrap();
    worker_b.join().unwrap();
    uaf_hits.load(Ordering::SeqCst)
}

// ---- Regression guards (run in the normal suite) ----------------------------

/// AFTER: the fixed `get_element_retained` path survives the exact dangerous
/// interleave with zero use-after-free touches.
#[test]
fn fixed_path_survives_concurrent_replace() {
    let uaf = run_forced_interleave(/* use_retained */ true, /* poison */ false);
    assert_eq!(uaf, 0, "retained path must not touch a released element");
}

/// BEFORE: the pre-fix bare-pointer path deterministically commits a
/// use-after-free under the same interleave. Asserting `> 0` documents the bug
/// the fix closes (this test PASSES — it proves the old code was unsafe).
#[test]
fn prefix_path_commits_use_after_free() {
    let uaf = run_forced_interleave(/* use_retained */ false, /* poison */ false);
    assert!(
        uaf > 0,
        "pre-fix path must hit the released element (the UAF)"
    );
}

/// AFTER, under real (unforced) concurrency: hammer get_element_retained +
/// snapshot-replace from many threads in a tight loop; assert zero UAF.
#[test]
fn fixed_path_stress_no_uaf() {
    let uaf_hits: &'static AtomicUsize = Box::leak(Box::new(AtomicUsize::new(0)));
    let cache = Arc::new(ElementCache::new());

    // Seed a snapshot of several elements.
    let seed: Vec<usize> = (0..8).map(|_| make_fake(uaf_hits, false)).collect();
    cache.core.insert(
        CacheKey {
            pid: PID,
            hwnd: HWND,
        },
        snapshot_with(seed),
    );

    const ITERS: usize = 4000;
    let mut handles = Vec::new();

    // Replacer threads: continuously run `update` (snapshot replace → Release).
    for _ in 0..3 {
        let cache_r = cache.clone();
        handles.push(thread::spawn(move || {
            for _ in 0..ITERS {
                let fresh: Vec<usize> = (0..8).map(|_| make_fake(uaf_hits, false)).collect();
                cache_r.core.insert(
                    CacheKey {
                        pid: PID,
                        hwnd: HWND,
                    },
                    snapshot_with(fresh),
                );
            }
        }));
    }

    // Actor threads: lookup-retain-deref-release, the click/type/set_value path.
    for _ in 0..3 {
        let cache_c = cache.clone();
        handles.push(thread::spawn(move || {
            for i in 0..ITERS {
                if let Some(guard) = cache_c.get_element_retained(PID, HWND, i % 8) {
                    unsafe { touch_vtable(guard.as_ptr()) };
                }
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(
        uaf_hits.load(Ordering::SeqCst),
        0,
        "no element may be touched after Release under concurrent replace"
    );
}

// ---- Hard-crash demonstrations (run manually, never in the suite) -----------

/// BEFORE (real fault): the pre-fix path takes a genuine
/// STATUS_ACCESS_VIOLATION. Poison-on-zero makes the otherwise heap-reuse-
/// dependent UAF deterministic. Run with:
///   cargo test -p platform-windows -- --ignored --exact \
///     uia::cache::cache_uaf_repro::prefix_path_real_access_violation
/// EXPECTED: the test process crashes (exception 0xC0000005); it does NOT
/// print "test result: ok".
#[test]
#[ignore]
fn prefix_path_real_access_violation() {
    let uaf = run_forced_interleave(/* use_retained */ false, /* poison */ true);
    // Unreachable in practice — the dereference of the poisoned vtable faults.
    println!("SURVIVED unexpectedly (uaf_hits={uaf})");
}

/// AFTER (control): the same poison-on-zero setup, but the retained guard keeps
/// the refcount above zero during the dereference, so the vtable is never
/// poisoned while in use. EXPECTED: clean exit, prints the survival line.
#[test]
#[ignore]
fn fixed_path_no_access_violation() {
    let uaf = run_forced_interleave(/* use_retained */ true, /* poison */ true);
    assert_eq!(uaf, 0);
    println!("SURVIVED as expected (uaf_hits={uaf})");
}
