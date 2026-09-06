//! Incarnation-aware ownership encoded atomically in XIAddMaster's name.
//! No separate property-write window can leave an unmarked new master after SIGKILL.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sha2::{Digest, Sha256};
use std::{fs, io};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Owner {
    domain: [u8; 16],
    pid: u32,
    start_ticks: u64,
}

fn start_ticks(stat: &str) -> io::Result<u64> {
    // comm can contain spaces and closing parentheses. Fields following its
    // final ')' begin with state (field 3); starttime is field 22.
    stat.rsplit_once(')')
        .and_then(|(_, tail)| tail.split_whitespace().nth(19))
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid proc stat"))
}

fn process_start(pid: u32) -> io::Result<u64> {
    start_ticks(&fs::read_to_string(format!("/proc/{pid}/stat"))?)
}

impl Owner {
    pub(super) fn current() -> io::Result<Self> {
        let pid = std::process::id();
        let own_stat = fs::read_to_string("/proc/self/stat")?;
        if own_stat
            .split_whitespace()
            .next()
            .and_then(|p| p.parse::<u32>().ok())
            != Some(pid)
        {
            return Err(io::Error::other("procfs exposes another PID namespace"));
        }
        let start_ticks = start_ticks(&own_stat)?;
        // A procfs mounted from another PID namespace cannot be used to
        // identify peers through namespace-local PIDs.
        if process_start(pid)? != start_ticks {
            return Err(io::Error::other(
                "procfs does not identify this PID namespace",
            ));
        }
        let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
        let namespace = fs::read_link("/proc/self/ns/pid")?;
        let mut hash = Sha256::new();
        hash.update(boot.trim().as_bytes());
        hash.update([0]);
        hash.update(namespace.as_os_str().as_encoded_bytes());
        hash.update([0]);
        hash.update(unsafe { libc::geteuid() }.to_be_bytes());
        let domain = hash.finalize()[..16].try_into().unwrap();
        Ok(Self {
            domain,
            pid,
            start_ticks,
        })
    }

    pub(super) fn master_name(&self, nonce: u64) -> String {
        let mut process = [0; 12];
        process[..4].copy_from_slice(&self.pid.to_be_bytes());
        process[4..].copy_from_slice(&self.start_ticks.to_be_bytes());
        format!(
            "CUA v1.{}.{}.{}",
            URL_SAFE_NO_PAD.encode(self.domain),
            URL_SAFE_NO_PAD.encode(process),
            URL_SAFE_NO_PAD.encode(nonce.to_be_bytes())
        )
    }

    pub(super) fn from_pointer_name(name: &str) -> Option<Self> {
        let mut parts = name
            .strip_prefix("CUA v1.")?
            .strip_suffix(" pointer")?
            .split('.');
        let domain: [u8; 16] = URL_SAFE_NO_PAD
            .decode(parts.next()?)
            .ok()?
            .try_into()
            .ok()?;
        let process: [u8; 12] = URL_SAFE_NO_PAD
            .decode(parts.next()?)
            .ok()?
            .try_into()
            .ok()?;
        let _: [u8; 8] = URL_SAFE_NO_PAD
            .decode(parts.next()?)
            .ok()?
            .try_into()
            .ok()?;
        if parts.next().is_some() {
            return None;
        }
        let pid = u32::from_be_bytes(process[..4].try_into().ok()?);
        if pid == 0 || pid > i32::MAX as u32 {
            return None;
        }
        Some(Self {
            domain,
            pid,
            start_ticks: u64::from_be_bytes(process[4..].try_into().ok()?),
        })
    }

    pub(super) fn stale_in(&self, current: &Self) -> bool {
        self.stale_with(current, process_start, |pid| {
            // hidepid and permission failures can look like absent procfs
            // entries. Require the kernel to independently report ESRCH.
            (unsafe { libc::kill(pid as i32, 0) }) == -1
                && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        })
    }

    fn stale_with(
        &self,
        current: &Self,
        read: impl FnOnce(u32) -> io::Result<u64>,
        absent: impl FnOnce(u32) -> bool,
    ) -> bool {
        if self.domain != current.domain {
            return false;
        }
        match read(self.pid) {
            Ok(start) => start != self.start_ticks,
            Err(error) if error.kind() == io::ErrorKind::NotFound => absent(self.pid),
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn owner() -> Owner {
        Owner {
            domain: [7; 16],
            pid: 123,
            start_ticks: 456,
        }
    }

    #[test]
    fn ownership_roundtrip_is_bounded_even_at_integer_limits() {
        let owner = Owner {
            pid: i32::MAX as u32,
            start_ticks: u64::MAX,
            ..owner()
        };
        let name = owner.master_name(u64::MAX);
        assert!(format!("{name} uinput pointer").len() <= 78);
        assert_eq!(
            Owner::from_pointer_name(&format!("{name} pointer")),
            Some(owner.clone())
        );
        assert_ne!(owner.master_name(1), owner.master_name(2));
        for suffix in [" keyboard", " XTEST pointer", " pointer extra"] {
            assert!(Owner::from_pointer_name(&format!("{name}{suffix}")).is_none());
        }
        for name in [
            "CUA legacy mp-123-1 pointer",
            "CUA v1.bad.bad.bad pointer",
            "ordinary pointer",
        ] {
            assert!(Owner::from_pointer_name(name).is_none());
        }
    }

    #[test]
    fn live_peer_and_unverifiable_metadata_are_preserved() {
        let owner = owner();
        assert!(!owner.stale_with(&owner, |_| Ok(456), |_| panic!("must not probe")));
        for kind in [
            io::ErrorKind::PermissionDenied,
            io::ErrorKind::InvalidData,
            io::ErrorKind::Interrupted,
        ] {
            assert!(!owner.stale_with(&owner, |_| Err(kind.into()), |_| panic!("must not probe")));
        }
        assert!(!owner.stale_with(&owner, |_| Err(io::ErrorKind::NotFound.into()), |_| false));
        let foreign = Owner {
            domain: [8; 16],
            ..owner.clone()
        };
        assert!(!foreign.stale_with(
            &owner,
            |_| panic!("foreign domain"),
            |_| panic!("foreign domain")
        ));
    }

    #[test]
    fn dead_owner_and_reused_pid_are_stale() {
        let owner = owner();
        assert!(owner.stale_with(&owner, |_| Err(io::ErrorKind::NotFound.into()), |_| true));
        assert!(owner.stale_with(&owner, |_| Ok(999), |_| panic!("must not probe")));
    }

    #[test]
    fn proc_stat_comm_delimiters_do_not_shift_starttime() {
        let tail = (3..=21)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(
            start_ticks(&format!("123 (name with ) parentheses) {tail} 98765 23")).unwrap(),
            98765
        );
        assert!(start_ticks("bad").is_err());
    }

    #[test]
    fn actual_current_process_is_not_stale() {
        let owner = Owner::current().unwrap();
        assert!(!owner.stale_in(&owner));
    }
}
