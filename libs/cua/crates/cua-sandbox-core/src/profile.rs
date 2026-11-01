//! Size profiles: named defaults for what a sandbox is for.
//!
//! A profile fills in what the caller left unset; anything given explicitly
//! (CPUs, memory, an environment variable) wins.
//!
//! * `default`: 2 vCPUs, 4 GiB, a desktop where the image has one.
//! * `rollout`: an RL rollout or agent episode that mostly waits on a
//!   trainer or a model: 1 vCPU (VMs overcommit vCPUs, `CUA_VM_VCPU_OVERCOMMIT`),
//!   1 GiB, and no desktop ([`CreateOptions::headless`], the one headless
//!   mechanism: cua-spacesd serves no desktop services, and a QEMU VM gets
//!   no display or input hardware).
//!   Exec, files, ports and processes work as always; screenshots and input
//!   do not.

use crate::CreateOptions;

/// A size profile ([`CreateOptions::apply_profile`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Profile {
    #[default]
    Default,
    Rollout,
}

/// The profile names, for errors and help.
pub const PROFILES: &[&str] = &["default", "rollout"];

impl Profile {
    /// `default` (or empty) or `rollout`, any case; else a message naming
    /// the valid ones.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "default" => Ok(Self::Default),
            "rollout" => Ok(Self::Rollout),
            other => Err(format!(
                "unknown profile '{other}' (use one of: {})",
                PROFILES.join(", ")
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Rollout => "rollout",
        }
    }

    /// vCPUs when the caller gives none.
    pub fn cpus(self) -> Option<u32> {
        match self {
            Self::Default => None,
            Self::Rollout => Some(1),
        }
    }

    /// Memory (MiB) when the caller gives none.
    pub fn memory_mb(self) -> Option<u64> {
        match self {
            Self::Default => None,
            Self::Rollout => Some(1024),
        }
    }

    /// Whether the profile asks for a Space without a desktop
    /// ([`CreateOptions::headless`]).
    pub fn headless(self) -> bool {
        matches!(self, Self::Rollout)
    }
}

impl CreateOptions {
    /// Fills in what `profile` sets and the caller did not: CPUs unless
    /// `cpus_given`, memory unless `memory_given`, and no desktop
    /// ([`CreateOptions::headless`]) unless the caller asked for the desktop
    /// with `CUA_ENV_NO_DESKTOP=false`.
    pub fn apply_profile(&mut self, profile: Profile, cpus_given: bool, memory_given: bool) {
        if !cpus_given && let Some(c) = profile.cpus() {
            self.cpus = c;
        }
        if !memory_given && let Some(m) = profile.memory_mb() {
            self.memory_mb = m;
        }
        let desktop_asked = self.env.get("CUA_ENV_NO_DESKTOP").is_some_and(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        });
        if profile.headless() && !desktop_asked {
            self.headless = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProviderKind;

    #[test]
    fn rollout_fills_what_is_unset_and_keeps_what_is_given() {
        let mut o = CreateOptions::new(ProviderKind::Local, "img");
        o.apply_profile(Profile::Rollout, false, false);
        assert_eq!((o.cpus, o.memory_mb), (1, 1024));
        assert!(
            o.headless,
            "rollout goes through the one headless mechanism"
        );
        assert!(o.env.is_empty(), "no second, env-only headless path");

        let mut o = CreateOptions::new(ProviderKind::Local, "img");
        o.cpus = 2;
        o.memory_mb = 4096;
        o.env.insert("CUA_ENV_NO_DESKTOP".into(), "false".into());
        o.apply_profile(Profile::Rollout, true, true);
        assert_eq!((o.cpus, o.memory_mb), (2, 4096));
        assert!(!o.headless);

        let mut o = CreateOptions::new(ProviderKind::Local, "img");
        o.apply_profile(Profile::Default, false, false);
        assert_eq!((o.cpus, o.memory_mb), (2, 4096));
        assert!(!o.headless);
    }

    #[test]
    fn parse_names_the_valid_profiles() {
        assert_eq!(Profile::parse(" Rollout ").unwrap(), Profile::Rollout);
        assert_eq!(Profile::parse("").unwrap(), Profile::Default);
        let e = Profile::parse("tiny").unwrap_err();
        assert!(
            e.contains("unknown profile 'tiny'") && e.contains("default, rollout"),
            "{e}"
        );
    }
}
