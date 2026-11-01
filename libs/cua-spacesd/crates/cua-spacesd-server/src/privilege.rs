// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Whether, and how, this agent can run work as another guest user.
//!
//! `ProcessService` (`ProcessConfig.user`) and `FilesystemService`
//! (`run_as`) make the child take the target user's identity (supplementary
//! groups, gid, uid) between `fork` and `exec`, so no `sudo` is involved.
//! That needs one of:
//!
//! - **root**: the agent runs as uid 0;
//! - **capabilities**: the agent runs as an ordinary user (the Spaces images
//!   run it as `cua`) that holds `CAP_SETUID` and `CAP_SETGID` (and
//!   `CAP_KILL`, to signal the processes it started as another user) in its
//!   effective set. The images grant exactly those three, as ambient
//!   capabilities of the agent's start (`AmbientCapabilities=` in the
//!   systemd unit, `setpriv --ambient-caps` in the container supervisor).
//!   Nothing else is added and the agent never becomes root.
//!
//! [`confine`] runs first thing in the agent: it empties the inheritable and
//! ambient sets, so only the agent process itself holds the capabilities.
//! Everything it starts (a command as the default user, a desktop app, a
//! browser) is an ordinary unprivileged process, and a child switched to
//! another user loses them at `exec` as well (and explicitly, by
//! [`drop_in_child`], before it).
//!
//! Without either, the call fails with `PERMISSION_DENIED` and says why;
//! older images are served by the client's `sudo -n` fallback.

/// The `ServerCapabilities` feature describing how `ProcessConfig.user` and
/// `run_as` are served. Attributes: `mode` (`root`, `capabilities`, `sudo`
/// or `none`), `signal_others` (`true` when the agent can signal processes
/// it started as another user).
pub const PROCESS_RUN_AS_FEATURE: &str = "process.run_as";

/// How the agent can switch users.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchMode {
    /// The agent is root.
    Root,
    /// The agent holds `CAP_SETUID` and `CAP_SETGID` without being root.
    Capabilities,
    /// Neither.
    None,
}

impl SwitchMode {
    /// The value of the `mode` attribute of the `process.run_as` feature.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::Capabilities => "capabilities",
            Self::None => "none",
        }
    }
}

/// The `process.run_as` feature for this agent.
pub fn run_as_feature() -> cua_proto::env::v1::Feature {
    let mut feature = cua_proto::env::v1::Feature {
        name: PROCESS_RUN_AS_FEATURE.into(),
        supported: true,
        limitation: String::new(),
        attributes: Default::default(),
    };
    #[cfg(unix)]
    {
        let mode = match switch_mode() {
            SwitchMode::None => match crate::filesystem::runas::privilege() {
                // `sudo` is on the path: the client wraps the command in
                // `sudo -n` when the guest allows it.
                Ok(_) => "sudo",
                Err(why) => {
                    feature.supported = false;
                    feature.limitation = why;
                    "none"
                }
            },
            other => other.as_str(),
        };
        feature.attributes.insert("mode".into(), mode.into());
        feature
            .attributes
            .insert("signal_others".into(), can_signal_others().to_string());
    }
    #[cfg(not(unix))]
    {
        feature.supported = false;
        feature.limitation = "running as another user is not supported on this platform".into();
        feature.attributes.insert("mode".into(), "none".into());
    }
    feature
}

/// `CAP_KILL`.
#[cfg(target_os = "linux")]
const CAP_KILL: u32 = 5;
/// `CAP_SETGID`.
#[cfg(target_os = "linux")]
const CAP_SETGID: u32 = 6;
/// `CAP_SETUID`.
#[cfg(target_os = "linux")]
const CAP_SETUID: u32 = 7;

#[cfg(target_os = "linux")]
mod sys {
    /// `_LINUX_CAPABILITY_VERSION_3`.
    const VERSION_3: u32 = 0x2008_0522;
    const PR_CAP_AMBIENT: libc::c_int = 47;
    const PR_CAP_AMBIENT_CLEAR_ALL: libc::c_ulong = 4;

    #[repr(C)]
    struct Header {
        version: u32,
        pid: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct Data {
        pub effective: u32,
        pub permitted: u32,
        pub inheritable: u32,
    }

    /// The calling thread's capability sets (low and high words).
    pub fn capget() -> Option<[Data; 2]> {
        let mut header = Header {
            version: VERSION_3,
            pid: 0,
        };
        let mut data = [Data::default(); 2];
        // SAFETY: capget(2) with a valid header and two data words.
        let rc = unsafe { libc::syscall(libc::SYS_capget, &mut header, data.as_mut_ptr()) };
        (rc == 0).then_some(data)
    }

    /// Sets the calling thread's capability sets.
    ///
    /// Async-signal-safe: one raw syscall.
    pub fn capset(data: &[Data; 2]) -> bool {
        let header = Header {
            version: VERSION_3,
            pid: 0,
        };
        // SAFETY: capset(2) with a valid header and two data words.
        unsafe { libc::syscall(libc::SYS_capset, &header, data.as_ptr()) == 0 }
    }

    /// Empties the ambient set. Async-signal-safe.
    pub fn clear_ambient() {
        // SAFETY: prctl(2) with constant arguments. Fails (ignored) on
        // kernels without ambient capabilities, where there is none to clear.
        unsafe {
            libc::prctl(PR_CAP_AMBIENT, PR_CAP_AMBIENT_CLEAR_ALL, 0, 0, 0);
        }
    }
}

/// How this agent can switch users.
pub fn switch_mode() -> SwitchMode {
    #[cfg(unix)]
    {
        // SAFETY: geteuid has no preconditions.
        if unsafe { libc::geteuid() } == 0 {
            return SwitchMode::Root;
        }
    }
    #[cfg(target_os = "linux")]
    if has_effective(CAP_SETUID) && has_effective(CAP_SETGID) {
        return SwitchMode::Capabilities;
    }
    SwitchMode::None
}

/// Whether this agent can signal processes of other users (root, or
/// `CAP_KILL`).
pub fn can_signal_others() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: geteuid has no preconditions.
        if unsafe { libc::geteuid() } == 0 {
            return true;
        }
    }
    #[cfg(target_os = "linux")]
    if has_effective(CAP_KILL) {
        return true;
    }
    false
}

#[cfg(target_os = "linux")]
fn has_effective(cap: u32) -> bool {
    sys::capget().is_some_and(|d| d[(cap / 32) as usize].effective & (1 << (cap % 32)) != 0)
}

/// Keeps the capabilities in this process only: empties the inheritable and
/// ambient sets (the effective and permitted sets stay). Call it first thing
/// in the agent, before it starts any thread or child. Does nothing for
/// root, which holds its privilege by uid, and on other platforms.
pub fn confine() {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: geteuid has no preconditions.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        if let Some(mut data) = sys::capget() {
            if data.iter().any(|d| d.inheritable != 0) {
                for d in &mut data {
                    d.inheritable = 0;
                }
                sys::capset(&data);
            }
        }
        sys::clear_ambient();
    }
}

/// Drops every capability of the calling process (all sets, ambient
/// included). For the forked child, after it has taken the target identity
/// and before `exec`. Async-signal-safe.
pub fn drop_in_child() {
    #[cfg(target_os = "linux")]
    {
        sys::capset(&[sys::Data::default(); 2]);
        sys::clear_ambient();
    }
}

/// The effective capability sets of this process as hex, for diagnostics
/// and tests (`CapEff` of `/proc/self/status`).
#[cfg(target_os = "linux")]
pub fn effective_hex() -> Option<String> {
    let d = sys::capget()?;
    Some(format!("{:08x}{:08x}", d[1].effective, d[0].effective))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_names_are_the_wire_values() {
        assert_eq!(SwitchMode::Root.as_str(), "root");
        assert_eq!(SwitchMode::Capabilities.as_str(), "capabilities");
        assert_eq!(SwitchMode::None.as_str(), "none");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn mode_agrees_with_the_kernel_view() {
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let cap_eff = status
            .lines()
            .find_map(|l| l.strip_prefix("CapEff:"))
            .map(|v| u64::from_str_radix(v.trim(), 16).unwrap())
            .unwrap();
        assert_eq!(
            effective_hex().unwrap(),
            format!("{cap_eff:016x}"),
            "capget and /proc agree"
        );
        let has = |cap: u32| cap_eff & (1 << cap) != 0;
        let root = unsafe { libc::geteuid() } == 0;
        let expected = if root {
            SwitchMode::Root
        } else if has(CAP_SETUID) && has(CAP_SETGID) {
            SwitchMode::Capabilities
        } else {
            SwitchMode::None
        };
        assert_eq!(switch_mode(), expected);
    }
}
