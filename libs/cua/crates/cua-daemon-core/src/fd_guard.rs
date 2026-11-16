//! The open-file limit, and the daemon's guard on it.
//!
//! A daemon that runs for days must not drift into "Too many open files":
//! once it does, the Keyvault, persistent agents and every Space call fail
//! together. The guard measures this process's descriptors every
//! [`ENV_FD_GUARD_SECS`] (30 s by default). Past [`PRESSURE_PERCENT`] of the
//! soft limit it logs a warning with what the descriptors are, raises the
//! soft limit when the hard limit allows, and asks the daemon's components
//! to give back what they can (the mesh's loopback forwards and their
//! connections; Spaces over the mesh reconnect on their next call).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Seconds between checks (`0`: no guard).
pub const ENV_FD_GUARD_SECS: &str = "CUA_FD_GUARD_SECS";
/// The share of the soft limit that counts as pressure.
pub const PRESSURE_PERCENT: u64 = 80;
const DEFAULT_INTERVAL: Duration = Duration::from_secs(30);
/// While pressure persists after healing, warn at most this often.
const REWARN: Duration = Duration::from_secs(600);

/// The open-file limit a daemon or SDK process asks for: it holds a socket
/// or pipe per Space, client, stream and engine connection, and macOS
/// starts processes (launchd, a terminal) at a soft limit of 256.
#[cfg(unix)]
const WANTED_OPEN_FILES: u64 = 65_536;

/// Raises this process's soft open-file limit toward 65,536, within the
/// hard limit (and, on macOS, `kern.maxfilesperproc`). Returns the soft
/// limit before and after, or `None` when nothing changed.
#[cfg(unix)]
pub fn raise_open_file_limit() -> Option<(u64, u64)> {
    let mut lim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit fills the struct it is given.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) } != 0 {
        return None;
    }
    let before: u64 = lim.rlim_cur;
    #[allow(unused_mut)]
    let mut ceiling = lim.rlim_max.min(WANTED_OPEN_FILES);
    #[cfg(target_os = "macos")]
    {
        // setrlimit refuses more than this on macOS, whatever the hard limit.
        let mut per_proc: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>();
        // SAFETY: sysctlbyname writes at most `len` bytes into `per_proc`.
        let rc = unsafe {
            libc::sysctlbyname(
                c"kern.maxfilesperproc".as_ptr(),
                (&mut per_proc as *mut libc::c_int).cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc == 0 && per_proc > 0 {
            ceiling = ceiling.min(per_proc as u64);
        }
    }
    if ceiling <= before {
        return None;
    }
    lim.rlim_cur = ceiling;
    // SAFETY: setrlimit reads the struct it is given.
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &lim) } != 0 {
        return None;
    }
    Some((before, ceiling))
}

/// Not applicable off Unix.
#[cfg(not(unix))]
pub fn raise_open_file_limit() -> Option<(u64, u64)> {
    None
}

/// This process's soft open-file limit.
#[cfg(unix)]
pub fn soft_limit() -> Option<u64> {
    let mut lim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit fills the struct it is given.
    (unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) } == 0).then_some(lim.rlim_cur)
}

/// Not measured off Unix.
#[cfg(not(unix))]
pub fn soft_limit() -> Option<u64> {
    None
}

/// This process's open descriptors, by kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FdCounts {
    /// All of them.
    pub total: u64,
    /// Sockets (TCP, UDP, Unix).
    pub sockets: u64,
    /// Regular files.
    pub files: u64,
    /// Pipes and FIFOs.
    pub pipes: u64,
}

/// Counts this process's open descriptors (`None` where it cannot).
#[cfg(unix)]
pub fn open_fds() -> Option<FdCounts> {
    #[cfg(target_os = "linux")]
    const DIR: &str = "/proc/self/fd";
    #[cfg(not(target_os = "linux"))]
    const DIR: &str = "/dev/fd";
    let fds: Vec<libc::c_int> = std::fs::read_dir(DIR)
        .ok()?
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
        .collect();
    let mut c = FdCounts::default();
    for fd in fds {
        // SAFETY: fstat writes into the zeroed struct it is given; a
        // descriptor closed meanwhile (the directory's own) just fails.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(fd, &mut st) } != 0 {
            continue;
        }
        c.total += 1;
        match st.st_mode & libc::S_IFMT {
            libc::S_IFSOCK => c.sockets += 1,
            libc::S_IFREG => c.files += 1,
            libc::S_IFIFO => c.pipes += 1,
            _ => {}
        }
    }
    Some(c)
}

/// Not measured off Unix.
#[cfg(not(unix))]
pub fn open_fds() -> Option<FdCounts> {
    None
}

/// Whether `open` descriptors are past [`PRESSURE_PERCENT`] of `limit`.
pub fn under_pressure(open: u64, limit: u64) -> bool {
    limit > 0 && u128::from(open) * 100 >= u128::from(limit) * u128::from(PRESSURE_PERCENT)
}

/// Gives descriptors back on request; returns how many things it dropped.
pub type Shed = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = usize> + Send>> + Send + Sync>;

/// What one check found and did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Below the threshold (or not measurable here).
    Fine,
    /// Was under pressure; `after` is the count and limit once healed.
    Healed {
        /// Descriptors and soft limit when the check found pressure.
        before: (FdCounts, u64),
        /// The soft limit raised (before, after), when it could be.
        raised: Option<(u64, u64)>,
        /// What the components dropped.
        shed: usize,
        /// Descriptors and soft limit after healing.
        after: (FdCounts, u64),
        /// Still past the threshold after healing.
        still_high: bool,
    },
}

/// One check: measures, and under pressure raises the limit and sheds.
/// `measure` and `raise` are [`open_fds`]/[`soft_limit`] and
/// [`raise_open_file_limit`] outside tests.
pub async fn check(
    measure: &(dyn Fn() -> Option<(FdCounts, u64)> + Sync),
    raise: &(dyn Fn() -> Option<(u64, u64)> + Sync),
    shed: &Shed,
) -> Outcome {
    let Some(before) = measure() else {
        return Outcome::Fine;
    };
    if !under_pressure(before.0.total, before.1) {
        return Outcome::Fine;
    }
    let raised = raise();
    // Shed only when raising did not leave room: it cuts mesh connections.
    let room = measure().is_some_and(|(c, l)| !under_pressure(c.total, l));
    let shed = if room { 0 } else { shed().await };
    let after = measure().unwrap_or(before);
    Outcome::Healed {
        before,
        raised,
        shed,
        still_high: under_pressure(after.0.total, after.1),
        after,
    }
}

fn measure_self() -> Option<(FdCounts, u64)> {
    Some((open_fds()?, soft_limit()?))
}

/// The check interval ([`ENV_FD_GUARD_SECS`]), `None` when turned off.
pub fn interval() -> Option<Duration> {
    match std::env::var(ENV_FD_GUARD_SECS)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
    {
        Some(0) => None,
        Some(s) => Some(Duration::from_secs(s)),
        None => Some(DEFAULT_INTERVAL),
    }
}

/// Runs the guard until `shutdown` changes.
pub async fn run(every: Duration, shed: Shed, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let mut last_warn: Option<Instant> = None;
    loop {
        tokio::select! {
            _ = shutdown.changed() => return,
            _ = tokio::time::sleep(every) => {}
        }
        let outcome = tokio::select! {
            _ = shutdown.changed() => return,
            outcome = check(&measure_self, &raise_open_file_limit, &shed) => outcome,
        };
        let Outcome::Healed {
            before,
            raised,
            shed,
            after,
            still_high,
        } = outcome
        else {
            continue;
        };
        if still_high && last_warn.is_some_and(|t| t.elapsed() < REWARN) {
            continue;
        }
        last_warn = Some(Instant::now());
        tracing::warn!(
            open = before.0.total,
            limit = before.1,
            sockets = before.0.sockets,
            files = before.0.files,
            pipes = before.0.pipes,
            raised_limit = ?raised,
            shed,
            open_after = after.0.total,
            limit_after = after.1,
            "the daemon is past {PRESSURE_PERCENT}% of its open-file limit{}",
            if still_high {
                "; healing (a higher limit where allowed, dropped mesh forwards) did not bring it down, which points to a descriptor leak (see `lsof -p` of this process)"
            } else {
                "; recovered"
            }
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    fn counts(total: u64) -> FdCounts {
        FdCounts {
            total,
            sockets: total,
            ..Default::default()
        }
    }

    fn shed_counter(
        hits: Arc<AtomicUsize>,
        gives_back: Arc<AtomicU64>,
        open: Arc<AtomicU64>,
    ) -> Shed {
        Arc::new(move || {
            let hits = hits.clone();
            let gives_back = gives_back.clone();
            let open = open.clone();
            Box::pin(async move {
                hits.fetch_add(1, Ordering::SeqCst);
                open.fetch_sub(gives_back.load(Ordering::SeqCst), Ordering::SeqCst);
                3
            })
        })
    }

    #[test]
    fn pressure_starts_at_eighty_percent() {
        assert!(!under_pressure(0, 0));
        assert!(!under_pressure(2047, 2560));
        assert!(under_pressure(2048, 2560));
        assert!(under_pressure(2566, 2560));
        assert!(!under_pressure(u64::MAX / 2, u64::MAX));
    }

    #[tokio::test]
    async fn below_the_threshold_nothing_happens() {
        let hits = Arc::new(AtomicUsize::new(0));
        let open = Arc::new(AtomicU64::new(100));
        let shed = shed_counter(hits.clone(), Arc::new(AtomicU64::new(0)), open.clone());
        let o = check(
            &|| Some((counts(open.load(Ordering::SeqCst)), 2560)),
            &|| panic!("no raise below the threshold"),
            &shed,
        )
        .await;
        assert_eq!(o, Outcome::Fine);
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn raising_the_limit_heals_without_cutting_connections() {
        let hits = Arc::new(AtomicUsize::new(0));
        let open = Arc::new(AtomicU64::new(2300));
        let limit = Arc::new(AtomicU64::new(2560));
        let shed = shed_counter(hits.clone(), Arc::new(AtomicU64::new(0)), open.clone());
        let l2 = limit.clone();
        let o = check(
            &|| {
                Some((
                    counts(open.load(Ordering::SeqCst)),
                    limit.load(Ordering::SeqCst),
                ))
            },
            &|| {
                l2.store(65_536, Ordering::SeqCst);
                Some((2560, 65_536))
            },
            &shed,
        )
        .await;
        let Outcome::Healed {
            raised,
            shed,
            still_high,
            ..
        } = o
        else {
            panic!("{o:?}");
        };
        assert_eq!(raised, Some((2560, 65_536)));
        assert_eq!(shed, 0);
        assert!(!still_high);
        assert_eq!(hits.load(Ordering::SeqCst), 0, "no need to shed");
    }

    #[tokio::test]
    async fn at_the_hard_limit_it_sheds_and_says_whether_that_was_enough() {
        let hits = Arc::new(AtomicUsize::new(0));
        let open = Arc::new(AtomicU64::new(2500));
        let gives_back = Arc::new(AtomicU64::new(2000));
        let shed = shed_counter(hits.clone(), gives_back.clone(), open.clone());
        let measure = || Some((counts(open.load(Ordering::SeqCst)), 2560));
        let o = check(&measure, &|| None, &shed).await;
        let Outcome::Healed {
            shed: n,
            still_high,
            after,
            ..
        } = o
        else {
            panic!("{o:?}");
        };
        assert_eq!((n, still_high, after.0.total), (3, false, 500));
        // A leak shedding cannot fix is reported as such.
        open.store(2500, Ordering::SeqCst);
        gives_back.store(10, Ordering::SeqCst);
        let o = check(&measure, &|| None, &shed).await;
        assert!(
            matches!(
                o,
                Outcome::Healed {
                    still_high: true,
                    ..
                }
            ),
            "{o:?}"
        );
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[cfg(unix)]
    #[test]
    fn counts_this_processes_descriptors_by_kind() {
        let before = open_fds().expect("measurable");
        let dir = tempfile::tempdir().unwrap();
        let files: Vec<_> = (0..8)
            .map(|i| std::fs::File::create(dir.path().join(i.to_string())).unwrap())
            .collect();
        let sock = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let after = open_fds().unwrap();
        // Other tests run in parallel: at least ours are counted.
        assert!(
            after.files >= before.files + 8 || after.total >= before.total + 9,
            "{before:?} -> {after:?}"
        );
        assert!(after.sockets >= 1);
        assert!(soft_limit().unwrap() > 0);
        drop((files, sock));
    }

    #[test]
    fn the_interval_can_be_turned_off() {
        // Read-only on the environment: the default when unset.
        if std::env::var(ENV_FD_GUARD_SECS).is_err() {
            assert_eq!(interval(), Some(DEFAULT_INTERVAL));
        }
    }
}
