// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! GPU acceleration for macOS guests (experimental).
//!
//! A macOS guest's GPU is Apple's paravirtualized device
//! (`VZMacGraphicsDeviceConfiguration`); by default it reports a
//! conservative Metal feature level. The user preference
//! `com.apple.gpusw.ParavirtualizedGraphics ForceUnrestrictedDeviceFeatureLevel`
//! makes devices created afterwards report the unrestricted level, so Metal
//! apps in the guest can pick newer GPU paths (see
//! [`crate::gpu::LUME_GPU_DOCS`]). It is read when a VM's device is
//! created, so it is applied before every start of a VM created with GPU
//! acceleration, and it applies to every VM this user starts while set.
//!
//! What cua did is recorded: the VM's marker (`<root>/<vm>/gpu`) and, when
//! cua turned the preference on (it was off), `<root>/gpu-preference.json`;
//! when the last VM with the marker is deleted the preference goes back off.
//! A preference the user set themselves is never removed.

use std::path::{Path, PathBuf};

/// The preference domain.
pub const DOMAIN: &str = "com.apple.gpusw.ParavirtualizedGraphics";
/// The preference key.
pub const KEY: &str = "ForceUnrestrictedDeviceFeatureLevel";
/// The VM's marker file.
pub const MARKER: &str = "gpu";
const RECORD: &str = "gpu-preference.json";

/// Reads and writes the host preference.
pub trait GpuPreference: Send + Sync + std::fmt::Debug {
    /// Whether it is on.
    fn get(&self) -> std::io::Result<bool>;
    /// Turns it on (`true`) or removes it (`false`).
    fn set(&self, on: bool) -> std::io::Result<()>;
}

/// The real preference, through `defaults`.
#[derive(Debug, Default)]
pub struct Defaults;

impl GpuPreference for Defaults {
    fn get(&self) -> std::io::Result<bool> {
        let out = std::process::Command::new("/usr/bin/defaults")
            .args(["read", DOMAIN, KEY])
            .output()?;
        // `defaults read` fails when the key is absent.
        Ok(out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "1")
    }

    fn set(&self, on: bool) -> std::io::Result<()> {
        let mut cmd = std::process::Command::new("/usr/bin/defaults");
        if on {
            cmd.args(["write", DOMAIN, KEY, "-bool", "true"]);
        } else {
            cmd.args(["delete", DOMAIN, KEY]);
        }
        let out = cmd.output()?;
        if out.status.success() || !on {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "defaults write {DOMAIN} {KEY}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )))
        }
    }
}

/// The GPU state of Lume VMs under `root` (cua's Lume state directory).
#[derive(Debug)]
pub struct Gpu<'a> {
    root: &'a Path,
    pref: &'a dyn GpuPreference,
}

impl<'a> Gpu<'a> {
    /// Over `root`, with `pref`.
    pub fn new(root: &'a Path, pref: &'a dyn GpuPreference) -> Self {
        Self { root, pref }
    }

    fn marker(&self, vm: &str) -> PathBuf {
        self.root.join(vm).join(MARKER)
    }

    /// Whether `vm` was created with GPU acceleration.
    pub fn enabled(&self, vm: &str) -> bool {
        self.marker(vm).exists()
    }

    /// Records that `vm` runs with GPU acceleration.
    pub fn enable(&self, vm: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(self.root.join(vm))?;
        std::fs::write(self.marker(vm), b"paravirtual\n")
    }

    /// Before `vm` starts: turns the preference on when `vm` has GPU
    /// acceleration and it is off (recording that cua did). A no-op for
    /// other VMs.
    pub fn before_start(&self, vm: &str) -> std::io::Result<()> {
        if !self.enabled(vm) || self.pref.get()? {
            return Ok(());
        }
        self.pref.set(true)?;
        std::fs::create_dir_all(self.root)?;
        std::fs::write(self.root.join(RECORD), br#"{"set_by_cua":true}"#)?;
        tracing::info!(vm, "turned on {DOMAIN} {KEY} for GPU acceleration");
        Ok(())
    }

    /// After `vm` was deleted (its marker is gone with its directory):
    /// removes the preference when cua set it and no other VM here still
    /// has GPU acceleration.
    pub fn after_delete(&self, vm: &str) -> std::io::Result<()> {
        let _ = std::fs::remove_file(self.marker(vm));
        let record = self.root.join(RECORD);
        if !record.exists() {
            return Ok(());
        }
        let others = std::fs::read_dir(self.root)?
            .flatten()
            .any(|e| e.path().join(MARKER).exists());
        if others {
            return Ok(());
        }
        self.pref.set(false)?;
        std::fs::remove_file(record)?;
        tracing::info!("removed {DOMAIN} {KEY}: no VM with GPU acceleration is left");
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A preference in memory (tests never touch the host's).
    #[derive(Debug, Default)]
    pub(crate) struct FakePref(pub Mutex<(bool, u32)>);

    impl GpuPreference for FakePref {
        fn get(&self) -> std::io::Result<bool> {
            Ok(self.0.lock().unwrap().0)
        }
        fn set(&self, on: bool) -> std::io::Result<()> {
            let mut s = self.0.lock().unwrap();
            s.0 = on;
            s.1 += 1;
            Ok(())
        }
    }

    #[test]
    fn cua_turns_the_preference_on_for_gpu_vms_and_off_after_the_last() {
        let dir = tempfile::tempdir().unwrap();
        let pref = FakePref::default();
        let gpu = Gpu::new(dir.path(), &pref);
        gpu.before_start("plain").unwrap();
        assert_eq!(*pref.0.lock().unwrap(), (false, 0), "no GPU, no change");
        gpu.enable("a").unwrap();
        gpu.enable("b").unwrap();
        gpu.before_start("a").unwrap();
        gpu.before_start("b").unwrap();
        assert_eq!(*pref.0.lock().unwrap(), (true, 1), "set once");
        std::fs::remove_dir_all(dir.path().join("a")).unwrap();
        gpu.after_delete("a").unwrap();
        assert!(pref.0.lock().unwrap().0, "b still needs it");
        std::fs::remove_dir_all(dir.path().join("b")).unwrap();
        gpu.after_delete("b").unwrap();
        assert_eq!(
            *pref.0.lock().unwrap(),
            (false, 2),
            "the last one turns it off"
        );
    }

    #[test]
    fn a_preference_the_user_set_stays() {
        let dir = tempfile::tempdir().unwrap();
        let pref = FakePref(Mutex::new((true, 0)));
        let gpu = Gpu::new(dir.path(), &pref);
        gpu.enable("a").unwrap();
        gpu.before_start("a").unwrap();
        std::fs::remove_dir_all(dir.path().join("a")).unwrap();
        gpu.after_delete("a").unwrap();
        assert_eq!(*pref.0.lock().unwrap(), (true, 0), "never written");
    }
}
