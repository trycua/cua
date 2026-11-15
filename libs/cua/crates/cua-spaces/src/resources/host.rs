//! This computer's processor and memory (sysinfo) and the volume that holds
//! Spaces (`statvfs`).

use cua_vmm::disk;
use std::path::{Path, PathBuf};
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

/// Processor and memory now.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HostSample {
    /// Cores.
    pub cpu_count: u32,
    /// Busy now, 0 to 100.
    pub cpu_percent: f64,
    /// 1-minute load average.
    pub load_average_1m: f64,
    /// Memory size, bytes.
    pub memory_total: u64,
    /// Memory in use, bytes, without reclaimable cache.
    pub memory_used: u64,
}

/// Reads this computer's processor and memory. Keep one: a busy percentage
/// is the change since the previous read, so a first read waits
/// [`sysinfo::MINIMUM_CPU_UPDATE_INTERVAL`] and later ones do not.
pub struct HostSampler {
    system: System,
    primed: bool,
}

impl Default for HostSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl HostSampler {
    /// A sampler that has not read yet.
    pub fn new() -> Self {
        Self {
            system: System::new_with_specifics(
                RefreshKind::nothing()
                    .with_cpu(CpuRefreshKind::nothing().with_cpu_usage())
                    .with_memory(MemoryRefreshKind::everything()),
            ),
            primed: false,
        }
    }

    /// Reads now. Blocks for a moment on the first call: run it on a
    /// blocking thread.
    pub fn sample(&mut self) -> HostSample {
        if !self.primed {
            std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
            self.primed = true;
        }
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        HostSample {
            cpu_count: self.system.cpus().len() as u32,
            cpu_percent: f64::from(self.system.global_cpu_usage()).clamp(0.0, 100.0),
            load_average_1m: System::load_average().one.max(0.0),
            memory_total: self.system.total_memory(),
            memory_used: self.system.used_memory().min(self.system.total_memory()),
        }
    }
}

/// Total and free bytes of the volume that holds Spaces.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Volume {
    /// Volume size.
    pub total: u64,
    /// Free bytes available to this user.
    pub free: u64,
}

/// The volumes Spaces are stored on: the cua home (QEMU disks, images,
/// containers) and Lume's VM store. Only paths that exist.
pub fn space_paths(home: &Path) -> Vec<PathBuf> {
    let mut paths = vec![home.to_path_buf()];
    if let Some(lume) = lume_home() {
        paths.push(lume);
    }
    paths
}

/// Lume's VM store (`~/.lume`), when it exists.
pub fn lume_home() -> Option<PathBuf> {
    let dir = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)?
        .join(".lume");
    dir.is_dir().then_some(dir)
}

/// The most constrained of the volumes at `paths` (the least free space):
/// what fills first is what matters. `None` when none can be read.
pub fn most_constrained(paths: &[PathBuf]) -> Option<Volume> {
    paths
        .iter()
        .filter_map(|p| disk::space(p).ok())
        .map(|s| Volume {
            total: s.total,
            free: s.available,
        })
        .min_by_key(|v| v.free)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_computer_has_cores_and_memory() {
        let mut s = HostSampler::new();
        let a = s.sample();
        assert!(a.cpu_count >= 1);
        assert!(a.memory_total > 0);
        assert!(a.memory_used <= a.memory_total);
        assert!((0.0..=100.0).contains(&a.cpu_percent));
        // A second read works without the first-read pause.
        assert_eq!(s.sample().cpu_count, a.cpu_count);
    }

    #[test]
    fn the_tightest_volume_wins() {
        let dir = tempfile::tempdir().unwrap();
        let v = most_constrained(&[dir.path().to_path_buf()]).unwrap();
        assert!(v.total > 0 && v.free <= v.total);
        assert_eq!(most_constrained(&[]), None);
        // A path that does not exist yet reads its nearest parent.
        assert!(most_constrained(&[dir.path().join("not/here")]).is_some());
    }
}
