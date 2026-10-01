//! Detect target processes whose UI toolkit derives pointer-event locations
//! from the hardware pointer rather than from the delivered event.
//!
//! Tk's macOS backend reads the global pointer position when it translates an
//! incoming mouse event into Tk coordinates. A PID-routed background CGEvent
//! never moves the hardware pointer, so such an app receives the click wherever
//! the real pointer happens to be (for example the visual-only canvas fixture's
//! `ignored click outside cards at canvas (-86, -114)`), while the driver can
//! only report the post as unverifiable. Background delivery therefore cannot
//! satisfy these targets; the explicit foreground rung moves the pointer first.
//!
//! Detection is intentionally cheap and conservative: walk the target's
//! file-backed memory regions with `proc_pidinfo` (same-user, no task port) and
//! look for a mapped Tk library or Python's `_tkinter` extension. A toolkit
//! loaded only from the dyld shared cache is not visible this way; such targets
//! fall back to the honest "not driver-verified" background result.

/// A UI toolkit known to derive click locations from the hardware pointer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PointerReadingToolkit {
    Tk,
}

impl PointerReadingToolkit {
    pub fn name(self) -> &'static str {
        match self {
            Self::Tk => "tk",
        }
    }
}

/// Classify one mapped image path. Pure so the matcher is unit-testable.
///
/// Matches `Tk.framework` bundles, Tk shared libraries (`libtk8.6.dylib`,
/// Tk 9's `libtcl9tk9.0.dylib`), and CPython's `_tkinter` extension module,
/// which either links Tk dynamically or embeds it statically.
pub fn classify_image_path(path: &str) -> Option<PointerReadingToolkit> {
    if path.contains("/Tk.framework/") {
        return Some(PointerReadingToolkit::Tk);
    }
    let file = path.rsplit('/').next().unwrap_or(path);
    let lower = file.to_ascii_lowercase();
    if lower.starts_with("_tkinter") && lower.ends_with(".so") {
        return Some(PointerReadingToolkit::Tk);
    }
    if !lower.ends_with(".dylib") {
        return None;
    }
    let rest = lower.strip_prefix("lib")?;
    // Tk 9 prefixes the Tcl major version: `libtcl9tk9.0.dylib`.
    let rest = match rest.strip_prefix("tcl") {
        Some(after_tcl) => {
            let digits = after_tcl.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return None;
            }
            &after_tcl[digits..]
        }
        None => rest,
    };
    let after_tk = rest.strip_prefix("tk")?;
    after_tk
        .bytes()
        .next()
        .filter(u8::is_ascii_digit)
        .map(|_| PointerReadingToolkit::Tk)
}

/// Return the pointer-reading toolkit mapped into `pid`, if any.
#[cfg(target_os = "macos")]
pub fn detect(pid: i32) -> Option<PointerReadingToolkit> {
    // Prefer the vnode-only flavor, which skips anonymous regions; fall back
    // to the full walk on kernels that reject it.
    match walk_regions(pid, PROC_PIDREGIONPATHINFO2) {
        Some(found) => found,
        None => walk_regions(pid, PROC_PIDREGIONPATHINFO).flatten(),
    }
}

#[cfg(not(target_os = "macos"))]
pub fn detect(_pid: i32) -> Option<PointerReadingToolkit> {
    None
}

#[cfg(target_os = "macos")]
const PROC_PIDREGIONPATHINFO: libc::c_int = 8;
#[cfg(target_os = "macos")]
const PROC_PIDREGIONPATHINFO2: libc::c_int = 22;
/// Bound the walk so a pathological address space cannot stall a click.
#[cfg(target_os = "macos")]
const MAX_REGIONS: usize = 65_536;

/// `struct proc_regioninfo` from `<sys/proc_info.h>`.
#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
struct ProcRegionInfo {
    pri_protection: u32,
    pri_max_protection: u32,
    pri_inheritance: u32,
    pri_flags: u32,
    pri_offset: u64,
    pri_behavior: u32,
    pri_user_wired_count: u32,
    pri_user_tag: u32,
    pri_pages_resident: u32,
    pri_pages_shared_now_private: u32,
    pri_pages_swapped_out: u32,
    pri_pages_dirtied: u32,
    pri_ref_count: u32,
    pri_shadow_depth: u32,
    pri_share_mode: u32,
    pri_private_pages_resident: u32,
    pri_shared_pages_resident: u32,
    pri_obj_id: u32,
    pri_depth: u32,
    pri_address: u64,
    pri_size: u64,
}

/// `struct proc_regionwithpathinfo`: region info followed by
/// `struct vnode_info_path` (152-byte `vnode_info` + `MAXPATHLEN` path).
#[cfg(target_os = "macos")]
#[repr(C)]
struct ProcRegionWithPathInfo {
    prp_prinfo: ProcRegionInfo,
    prp_vip_vi: [u8; 152],
    prp_vip_path: [libc::c_char; 1024],
}

/// Walk `pid`'s regions with `flavor`. `None` means the flavor was rejected
/// before any region was read; `Some(result)` is a completed walk.
#[cfg(target_os = "macos")]
fn walk_regions(pid: i32, flavor: libc::c_int) -> Option<Option<PointerReadingToolkit>> {
    let size = std::mem::size_of::<ProcRegionWithPathInfo>();
    let mut address: u64 = 0;
    let mut read_any = false;
    for _ in 0..MAX_REGIONS {
        // SAFETY: an all-zero bit pattern is valid for this plain C struct.
        let mut info: ProcRegionWithPathInfo = unsafe { std::mem::zeroed() };
        // SAFETY: the buffer is exactly the kernel structure size and lives for
        // the duration of the call.
        let written = unsafe {
            libc::proc_pidinfo(
                pid,
                flavor,
                address,
                (&mut info as *mut ProcRegionWithPathInfo).cast(),
                size as libc::c_int,
            )
        };
        if written <= 0 || (written as usize) < size {
            break;
        }
        read_any = true;
        // SAFETY: the kernel NUL-terminates `vip_path` within MAXPATHLEN.
        let path = unsafe { std::ffi::CStr::from_ptr(info.prp_vip_path.as_ptr()) };
        if let Some(toolkit) = classify_image_path(&path.to_string_lossy()) {
            return Some(Some(toolkit));
        }
        let region = info.prp_prinfo;
        let next = region.pri_address.saturating_add(region.pri_size);
        if next <= address {
            break;
        }
        address = next;
    }
    if read_any {
        Some(None)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_tk_images_and_rejects_neighbours() {
        for path in [
            "/Library/Frameworks/Tk.framework/Versions/8.6/Tk",
            "/opt/homebrew/Cellar/tcl-tk/9.0.2/lib/libtcl9tk9.0.dylib",
            "/opt/homebrew/opt/tcl-tk@8/lib/libtk8.6.dylib",
            "/usr/local/lib/python3.12/lib-dynload/_tkinter.cpython-312-darwin.so",
            "/Users/me/.local/share/uv/python/cpython-3.13/lib/python3.13/lib-dynload/_tkinter.cpython-313-darwin.so",
        ] {
            assert_eq!(
                classify_image_path(path),
                Some(PointerReadingToolkit::Tk),
                "{path}"
            );
        }
        for path in [
            "",
            "/usr/lib/libSystem.B.dylib",
            "/opt/homebrew/lib/libtcl9.0.dylib",
            "/opt/homebrew/lib/libtcl8.6.dylib",
            "/opt/homebrew/lib/libtkrzw.dylib",
            "/System/Library/Frameworks/AppKit.framework/Versions/C/AppKit",
            "/usr/lib/python3/_tkinter_helper.py",
            "/Applications/Foo.app/Contents/MacOS/tk8.6",
        ] {
            assert_eq!(classify_image_path(path), None, "{path}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn kernel_region_structure_matches_proc_info_layout() {
        assert_eq!(std::mem::size_of::<ProcRegionInfo>(), 96);
        assert_eq!(std::mem::size_of::<ProcRegionWithPathInfo>(), 1272);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn detect_is_negative_for_a_non_tk_process() {
        // The test binary maps no Tk image. A completed negative walk proves the
        // region flavor is readable for a same-user process.
        assert_eq!(detect(std::process::id() as i32), None);
        assert!(walk_regions(std::process::id() as i32, PROC_PIDREGIONPATHINFO).is_some());
    }

    /// Loads `_tkinter` without creating any window, so this never touches the
    /// desktop. Skips when the host Python lacks Tk support.
    #[cfg(target_os = "macos")]
    #[test]
    fn detect_finds_tk_in_a_python_process_that_imported_tkinter() {
        use std::io::BufRead;
        let spawned = std::process::Command::new("python3")
            .args([
                "-c",
                "import _tkinter, sys, time; print('ready', flush=True); time.sleep(30)",
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn();
        let Ok(mut child) = spawned else {
            eprintln!("skipping: python3 is unavailable");
            return;
        };
        let mut line = String::new();
        let ready = child
            .stdout
            .take()
            .map(|stdout| std::io::BufReader::new(stdout).read_line(&mut line))
            .is_some_and(|read| read.is_ok())
            && line.trim() == "ready";
        let detected = ready.then(|| detect(child.id() as i32));
        let _ = child.kill();
        let _ = child.wait();
        match detected {
            None => eprintln!("skipping: python3 has no importable _tkinter"),
            Some(found) => assert_eq!(found, Some(PointerReadingToolkit::Tk)),
        }
    }
}
