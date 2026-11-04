//! The host's system-wide open-file table, a resource every VM and job
//! draws on.
//!
//! macOS caps the table at `kern.maxfiles` and every process on the Mac
//! shares it: an Apple Virtualization VM that shares host directories
//! (virtiofs) holds one host file descriptor per file its guest has
//! cached, so a few file-heavy VMs can fill it. Then everything on the
//! Mac that opens a file fails with `ENFILE` ("Too many open files in
//! system"): `git`, `ssh`, the guest's `uv`. Linux has the same table
//! (`/proc/sys/fs/file-nr`).
//!
//! This module measures it ([`read`]), decides whether there is room for
//! more work ([`check`], pure) and names who holds the most descriptors
//! ([`top_holders`]) for the refusal's message ([`refusal`]). Lives here, in
//! the crate every host-side layer already depends on, so the VM runtime
//! (`cua-vmm`) and the job runner (`cua-cluster`) use one measurement.

/// How full the file table is: descriptors open and the most it holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileTable {
    pub open: u64,
    pub max: u64,
}

impl FileTable {
    /// Descriptors still available.
    pub fn free(&self) -> u64 {
        self.max.saturating_sub(self.open)
    }

    /// The free share of the table, 0.0 to 1.0.
    pub fn free_fraction(&self) -> f64 {
        if self.max == 0 {
            return 1.0;
        }
        self.free() as f64 / self.max as f64
    }
}

/// Environment variable: the least free share of the file table, in
/// percent, below which new VMs and jobs are refused (default
/// [`DEFAULT_MIN_FREE_PCT`]; `0` turns the check off).
pub const MIN_FREE_ENV: &str = "CUA_FILE_TABLE_MIN_FREE_PCT";

/// The default least free share of the table, in percent.
pub const DEFAULT_MIN_FREE_PCT: f64 = 10.0;

/// The least free fraction (0.0 to 1.0) from the environment value.
pub fn min_free_from(value: Option<&str>) -> f64 {
    let pct = value
        .and_then(|v| v.trim().trim_end_matches('%').parse::<f64>().ok())
        .filter(|p| p.is_finite() && (0.0..=100.0).contains(p))
        .unwrap_or(DEFAULT_MIN_FREE_PCT);
    pct / 100.0
}

/// The least free fraction of the file table this process enforces.
pub fn min_free() -> f64 {
    min_free_from(std::env::var(MIN_FREE_ENV).ok().as_deref())
}

/// Whether `table` leaves at least `min_free` (a fraction) of itself free.
/// `Err` carries the shortfall.
pub fn check(table: FileTable, min_free: f64) -> Result<(), FileTable> {
    if min_free <= 0.0 || table.free_fraction() >= min_free {
        Ok(())
    } else {
        Err(table)
    }
}

/// The system file table now, when the OS says.
pub fn read() -> Option<FileTable> {
    imp::read()
}

/// File descriptors process `pid` holds open, when the OS says and this
/// user may inspect it.
pub fn process_fds(pid: u32) -> Option<u64> {
    imp::process_fds(pid)
}

/// The `n` processes holding the most file descriptors as `(name, pid,
/// count)`, most first. Only processes this user may inspect (on macOS,
/// its own); empty where the OS cannot say. Cheap enough for a refusal's
/// message, not for a hot path.
pub fn top_holders(n: usize) -> Vec<(String, u32, u64)> {
    let mut all = imp::holders();
    all.sort_by_key(|h| std::cmp::Reverse(h.2));
    all.truncate(n);
    all
}

/// The one-sentence refusal plus its fix, naming the biggest holders.
pub fn refusal(table: FileTable, what: &str, holders: &[(String, u32, u64)]) -> String {
    let who = if holders.is_empty() {
        String::new()
    } else {
        let list: Vec<String> = holders
            .iter()
            .map(|(name, pid, n)| format!("{name} (pid {pid}) holds {n}"))
            .collect();
        format!(" Most are held by {}.", list.join(", "))
    };
    format!(
        "This machine is out of open-file capacity for {what}: {} of {} file descriptors are \
         free.{who} Stop Spaces or VMs you are not using (`cua daemon stop` stops an idle Linux \
         runtime), or set {MIN_FREE_ENV} lower.",
        table.free(),
        table.max,
    )
}

#[cfg(target_os = "macos")]
mod imp {
    use super::FileTable;

    fn sysctl_i32(name: &str) -> Option<u64> {
        let c = std::ffi::CString::new(name).ok()?;
        let mut v: i32 = 0;
        let mut len = std::mem::size_of::<i32>();
        // SAFETY: a NUL-terminated name, an out int and its length.
        let r = unsafe {
            libc::sysctlbyname(
                c.as_ptr(),
                (&mut v as *mut i32).cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        (r == 0 && v >= 0).then_some(v as u64)
    }

    pub fn read() -> Option<FileTable> {
        Some(FileTable {
            open: sysctl_i32("kern.num_files")?,
            max: sysctl_i32("kern.maxfiles")?,
        })
    }

    pub fn process_fds(pid: u32) -> Option<u64> {
        const ENTRY: usize = std::mem::size_of::<libc::proc_fdinfo>();
        let pid = pid as libc::pid_t;
        // The size a null buffer reports is an over-estimate that only
        // grows: read the list itself and count what it holds.
        // SAFETY: a null buffer asks for the descriptor list's size.
        let want =
            unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
        if want <= 0 {
            return None;
        }
        let mut buf = vec![0u8; want as usize + 64 * ENTRY];
        // SAFETY: the buffer and its byte length.
        let got = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDLISTFDS,
                0,
                buf.as_mut_ptr().cast(),
                buf.len() as libc::c_int,
            )
        };
        (got >= 0).then(|| got as u64 / ENTRY as u64)
    }

    pub fn holders() -> Vec<(String, u32, u64)> {
        // SAFETY: a null buffer asks for the size; the buffer is sized by it.
        let bytes = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        if bytes <= 0 {
            return Vec::new();
        }
        let mut pids = vec![0 as libc::pid_t; bytes as usize / 4 + 64];
        // SAFETY: the buffer and its byte length.
        let got = unsafe {
            libc::proc_listallpids(
                pids.as_mut_ptr().cast(),
                (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int,
            )
        };
        if got <= 0 {
            return Vec::new();
        }
        pids.truncate(got as usize);
        let mut out = Vec::new();
        for pid in pids.into_iter().filter(|p| *p > 0) {
            let Some(count) = process_fds(pid as u32) else {
                continue;
            };
            let mut name = [0u8; 64];
            // SAFETY: a 64-byte buffer and its length.
            let n = unsafe { libc::proc_name(pid, name.as_mut_ptr().cast(), 64) };
            let name = String::from_utf8_lossy(&name[..n.max(0) as usize]).into_owned();
            out.push((name, pid as u32, count));
        }
        out
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::FileTable;

    /// `/proc/sys/fs/file-nr`: "allocated unused max".
    pub fn parse_file_nr(text: &str) -> Option<FileTable> {
        let mut f = text.split_whitespace().map(|w| w.parse::<u64>().ok());
        let allocated = f.next()??;
        let unused = f.next()??;
        let max = f.next()??;
        Some(FileTable {
            open: allocated.saturating_sub(unused),
            max,
        })
    }

    pub fn read() -> Option<FileTable> {
        parse_file_nr(&std::fs::read_to_string("/proc/sys/fs/file-nr").ok()?)
    }

    pub fn process_fds(pid: u32) -> Option<u64> {
        Some(std::fs::read_dir(format!("/proc/{pid}/fd")).ok()?.count() as u64)
    }

    pub fn holders() -> Vec<(String, u32, u64)> {
        let Ok(dir) = std::fs::read_dir("/proc") else {
            return Vec::new();
        };
        dir.flatten()
            .filter_map(|e| {
                let pid: u32 = e.file_name().to_str()?.parse().ok()?;
                let n = std::fs::read_dir(e.path().join("fd")).ok()?.count() as u64;
                let name = std::fs::read_to_string(e.path().join("comm"))
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                Some((name, pid, n))
            })
            .collect()
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod imp {
    use super::FileTable;

    pub fn read() -> Option<FileTable> {
        None
    }

    pub fn process_fds(_: u32) -> Option<u64> {
        None
    }

    pub fn holders() -> Vec<(String, u32, u64)> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(open: u64, max: u64) -> FileTable {
        FileTable { open, max }
    }

    #[test]
    fn free_share_is_the_unused_part_of_the_table() {
        assert_eq!(t(250, 1000).free(), 750);
        assert!((t(250, 1000).free_fraction() - 0.75).abs() < 1e-9);
        // An unknown or absurd max never refuses, an over-full table has 0.
        assert_eq!(t(5, 0).free_fraction(), 1.0);
        assert_eq!(t(1100, 1000).free(), 0);
    }

    #[test]
    fn work_is_admitted_while_the_free_share_holds() {
        assert!(check(t(900, 1000), 0.10).is_ok());
        assert_eq!(check(t(901, 1000), 0.10), Err(t(901, 1000)));
        assert_eq!(check(t(1000, 1000), 0.10), Err(t(1000, 1000)));
        // 0 turns the check off, even for a full table.
        assert!(check(t(1000, 1000), 0.0).is_ok());
    }

    #[test]
    fn the_threshold_comes_from_the_environment_with_a_sane_default() {
        assert_eq!(min_free_from(None), 0.10);
        assert_eq!(min_free_from(Some("25")), 0.25);
        assert_eq!(min_free_from(Some(" 5% ")), 0.05);
        assert_eq!(min_free_from(Some("0")), 0.0);
        assert_eq!(min_free_from(Some("nonsense")), 0.10);
        assert_eq!(min_free_from(Some("-3")), 0.10);
        assert_eq!(min_free_from(Some("400")), 0.10);
    }

    #[test]
    fn the_refusal_is_one_sentence_plus_a_fix_naming_the_holders() {
        let m = refusal(
            t(240_000, 262_144),
            "a Linux VM",
            &[(
                "com.apple.Virtualization.VirtualMachine".into(),
                4242,
                151_000,
            )],
        );
        assert!(m.contains("22144 of 262144"), "{m}");
        assert!(m.contains("(pid 4242) holds 151000"), "{m}");
        assert!(m.contains("cua daemon stop"), "{m}");
        let bare = refusal(t(10, 100), "a job", &[]);
        assert!(!bare.contains("Most are held"), "{bare}");
    }

    #[test]
    fn the_live_table_reads_on_macos_and_linux() {
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            let table = read().expect("the OS reports its file table");
            assert!(table.max > 0 && table.open > 0 && table.open <= table.max);
            // This process holds some descriptors, and the list is sorted.
            let top = top_holders(3);
            assert!(top.windows(2).all(|w| w[0].2 >= w[1].2));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn file_nr_is_allocated_less_unused() {
        assert_eq!(
            imp::parse_file_nr("1248\t0\t9223372036854775807\n"),
            Some(t(1248, 9223372036854775807))
        );
        assert_eq!(imp::parse_file_nr("1000 40 5000"), Some(t(960, 5000)));
        assert_eq!(imp::parse_file_nr("garbage"), None);
    }
}
