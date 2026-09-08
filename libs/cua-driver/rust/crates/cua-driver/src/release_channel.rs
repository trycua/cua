//! Persistent stable/nightly update-channel preference.

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

pub const FILE_NAME: &str = "release-channel";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseChannel {
    Stable,
    Nightly,
}

impl ReleaseChannel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Nightly => "nightly",
        }
    }

    pub fn from_version(version: &str) -> Option<Self> {
        let parsed = semver::Version::parse(version).ok()?;
        if parsed.build.is_empty() && parsed.pre.is_empty() && parsed.to_string() == version {
            Some(Self::Stable)
        } else if parsed.build.is_empty()
            && parsed.to_string() == version
            && is_canonical_nightly_prerelease(parsed.pre.as_str())
        {
            Some(Self::Nightly)
        } else {
            None
        }
    }
}

fn is_canonical_nightly_prerelease(value: &str) -> bool {
    let Some(suffix) = value.strip_prefix("nightly.") else {
        return false;
    };
    let Some((date, run)) = suffix.split_once('.') else {
        return false;
    };
    date.len() == 8
        && date.bytes().all(|byte| byte.is_ascii_digit())
        && !run.is_empty()
        && run.bytes().all(|byte| byte.is_ascii_digit())
        && !run.starts_with('0')
}

impl Default for ReleaseChannel {
    fn default() -> Self {
        Self::Stable
    }
}

impl fmt::Display for ReleaseChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ReleaseChannel {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "stable" => Ok(Self::Stable),
            "nightly" => Ok(Self::Nightly),
            other => Err(format!(
                "invalid release channel {other:?}; expected `stable` or `nightly`"
            )),
        }
    }
}

pub fn selected() -> Result<ReleaseChannel, String> {
    let path = state_path()?;
    selected_at(&path)
}

fn selected_at(path: &std::path::Path) -> Result<ReleaseChannel, String> {
    match std::fs::read_to_string(path) {
        Ok(raw) => raw.parse(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ReleaseChannel::Stable),
        Err(error) => Err(format!(
            "cannot read release channel at {}: {error}",
            path.display()
        )),
    }
}

pub fn set(channel: ReleaseChannel) -> Result<(), String> {
    let path = state_path()?;
    set_at(&path, channel)
}

fn set_at(path: &std::path::Path, channel: ReleaseChannel) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "release-channel path has no parent".to_owned())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;

    let temporary = parent.join(format!(".{FILE_NAME}.tmp-{}", std::process::id()));
    std::fs::write(&temporary, format!("{}\n", channel.as_str()))
        .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    if let Err(error) = std::fs::rename(&temporary, &path) {
        // std::fs::rename cannot replace an existing destination on Windows.
        // Keep partial contents out of the canonical path on all platforms.
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|remove| format!("cannot replace {}: {remove}", path.display()))?;
            std::fs::rename(&temporary, &path)
                .map_err(|move_error| format!("cannot replace {}: {move_error}", path.display()))?;
        } else {
            return Err(format!("cannot persist {}: {error}", path.display()));
        }
    }
    Ok(())
}

pub fn state_path() -> Result<PathBuf, String> {
    if let Some(home) = std::env::var_os("CUA_DRIVER_RS_HOME") {
        return Ok(PathBuf::from(home).join(FILE_NAME));
    }
    let root = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or_else(|| "neither HOME nor USERPROFILE is set".to_owned())?;
    Ok(PathBuf::from(root)
        .join(crate::bundle::user_home_subdirectory())
        .join(FILE_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_channel_vocabulary() {
        assert_eq!("stable".parse(), Ok(ReleaseChannel::Stable));
        assert_eq!("nightly\n".parse(), Ok(ReleaseChannel::Nightly));
        assert!("beta".parse::<ReleaseChannel>().is_err());
        assert!("".parse::<ReleaseChannel>().is_err());
    }

    #[test]
    fn current_channel_uses_strict_version_shape() {
        assert_eq!(
            ReleaseChannel::from_version("0.19.3"),
            Some(ReleaseChannel::Stable)
        );
        assert_eq!(
            ReleaseChannel::from_version("0.19.4-nightly.20260812.42"),
            Some(ReleaseChannel::Nightly)
        );
        assert_eq!(ReleaseChannel::from_version("0.19.4-dev"), None);
        assert_eq!(ReleaseChannel::from_version("0.19.4-nightly.foo"), None);
        assert_eq!(ReleaseChannel::from_version("0.19.4+build"), None);
        assert_eq!(
            ReleaseChannel::from_version("0.19.4-nightly.20260812.0"),
            None
        );
    }

    #[test]
    fn missing_state_defaults_stable_and_valid_state_round_trips() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join(FILE_NAME);
        assert_eq!(selected_at(&path), Ok(ReleaseChannel::Stable));
        set_at(&path, ReleaseChannel::Nightly).expect("persist nightly");
        assert_eq!(selected_at(&path), Ok(ReleaseChannel::Nightly));
        set_at(&path, ReleaseChannel::Stable).expect("persist stable");
        assert_eq!(selected_at(&path), Ok(ReleaseChannel::Stable));
    }
}
