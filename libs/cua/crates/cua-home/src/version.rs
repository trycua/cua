//! Which of two `cua` builds is newer, the one rule every "never downgrade"
//! decision uses: an app linking its bundled CLI, `cua daemon start`
//! choosing a helper, and a daemon's keep-or-replace verdict.
//!
//! Versions are semver (`major.minor.patch[-pre][+build]`, an optional
//! leading `v`). A pre-release sorts before its release. A *dev build*
//! (`0.0.0`, or a pre-release or build part naming `dev`) counts as older
//! than any release of the same or a later version, unless it is the very
//! same build (identical version text).

use std::cmp::Ordering;

/// A parsed `cua` version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CuaVersion {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// Dot-separated pre-release identifiers (`rc.1`), empty for a release.
    pub pre: Vec<String>,
    /// A dev build: older than every release.
    pub dev: bool,
}

impl CuaVersion {
    /// `0.7.0`, `v0.7.0-rc.1+abc`, ... `None` when it is not semver.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().trim_start_matches('v');
        let (rest, build) = match text.split_once('+') {
            Some((r, b)) => (r, b),
            None => (text, ""),
        };
        let (core, pre) = match rest.split_once('-') {
            Some((c, p)) => (c, p),
            None => (rest, ""),
        };
        let mut nums = core.split('.').map(|p| {
            (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
                .then(|| p.parse::<u64>().ok())
                .flatten()
        });
        let (major, minor, patch) = (nums.next()??, nums.next()??, nums.next()??);
        if nums.next().is_some() {
            return None;
        }
        let pre: Vec<String> = pre
            .split('.')
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect();
        let names_dev = |s: &str| s.split(['.', '-']).any(|p| p.eq_ignore_ascii_case("dev"));
        let dev = (major, minor, patch) == (0, 0, 0)
            || pre.iter().any(|p| names_dev(p))
            || names_dev(build);
        Some(Self {
            major,
            minor,
            patch,
            pre,
            dev,
        })
    }

    /// The version in a `cua --version` line (`cua 0.7.0 (abc123)`), or in a
    /// bare version.
    pub fn from_version_line(line: &str) -> Option<Self> {
        let mut words = line.split_whitespace();
        let first = words.next()?;
        if first == "cua" {
            Self::parse(words.next()?)
        } else {
            Self::parse(first)
        }
    }

    fn core(&self) -> (u64, u64, u64) {
        (self.major, self.minor, self.patch)
    }

    /// Whether `self` is strictly newer than `other`. A dev build is older
    /// than any release of its own version or a later one, and two dev
    /// builds compare by number only.
    pub fn newer_than(&self, other: &Self) -> bool {
        match (self.dev, other.dev) {
            (true, false) => self.core() > other.core(),
            (false, true) => self.core() >= other.core(),
            _ => self.cmp_semver(other) == Ordering::Greater,
        }
    }

    fn cmp_semver(&self, other: &Self) -> Ordering {
        self.core().cmp(&other.core()).then_with(|| {
            match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => cmp_pre(&self.pre, &other.pre),
            }
        })
    }
}

fn cmp_pre(a: &[String], b: &[String]) -> Ordering {
    for (x, y) in a.iter().zip(b) {
        let o = match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(x), Ok(y)) => x.cmp(&y),
            (Ok(_), Err(_)) => Ordering::Less,
            (Err(_), Ok(_)) => Ordering::Greater,
            (Err(_), Err(_)) => x.cmp(y),
        };
        if o != Ordering::Equal {
            return o;
        }
    }
    a.len().cmp(&b.len())
}

/// Whether a bundled `cua` (`bundled`, its `--version` line or bare
/// version) should replace the one that is there (`installed`, `None` when
/// none runs): only when none runs, or the bundled one is newer. The same
/// version, a newer installed one, and an installed one whose version
/// cannot be read while the bundled one's can too are all kept; a
/// different build of the same version is kept. A dev build installed is
/// replaced by a release; a release is never replaced by a dev build.
pub fn bundled_replaces(installed: Option<&str>, bundled: &str) -> bool {
    let Some(installed) = installed.map(str::trim).filter(|s| !s.is_empty()) else {
        return true;
    };
    if installed == bundled.trim() {
        return false;
    }
    match (
        CuaVersion::from_version_line(installed),
        CuaVersion::from_version_line(bundled),
    ) {
        (Some(have), Some(ours)) => ours.newer_than(&have),
        // The installed one is not a readable cua: replace it. A bundled
        // one with no readable version never replaces a working one.
        (None, Some(_)) => true,
        (_, None) => false,
    }
}

/// Whether `theirs` (a running daemon's or CLI's version text) is strictly
/// newer than `ours`; false when either cannot be read.
pub fn is_newer(theirs: &str, ours: &str) -> bool {
    match (
        CuaVersion::from_version_line(theirs),
        CuaVersion::from_version_line(ours),
    ) {
        (Some(t), Some(o)) => t.newer_than(&o),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_semver_and_cua_version_lines() {
        let v = CuaVersion::from_version_line("cua 0.7.0 (abc123)").unwrap();
        assert_eq!(v.core(), (0, 7, 0));
        assert!(!v.dev && v.pre.is_empty());
        assert_eq!(CuaVersion::parse("v1.2.3-rc.1+x").unwrap().pre, ["rc", "1"]);
        assert!(CuaVersion::parse("0.7").is_none());
        assert!(CuaVersion::parse("1.2.3.4").is_none());
        assert!(CuaVersion::parse("Traceback").is_none());
        assert!(CuaVersion::parse("0.0.0").unwrap().dev);
        assert!(CuaVersion::parse("0.8.0-dev.3").unwrap().dev);
        assert!(CuaVersion::parse("0.8.0+dev").unwrap().dev);
        assert!(!CuaVersion::parse("0.8.0-rc.1").unwrap().dev);
    }

    #[test]
    fn newer_follows_semver() {
        assert!(is_newer("0.7.1", "0.7.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.7.0", "0.7.0"));
        assert!(!is_newer("0.6.0", "0.7.0"));
        assert!(is_newer("0.7.0", "0.7.0-rc.1"));
        assert!(!is_newer("0.7.0-rc.1", "0.7.0"));
        assert!(is_newer("0.7.0-rc.2", "0.7.0-rc.1"));
        assert!(is_newer("0.7.0-rc.10", "0.7.0-rc.9"));
        assert!(!is_newer("garbage", "0.7.0"));
        assert!(!is_newer("0.7.0", "garbage"));
    }

    #[test]
    fn a_dev_build_is_older_than_a_release() {
        assert!(!is_newer("0.7.0-dev", "0.7.0"));
        assert!(is_newer("0.7.0", "0.7.0-dev"));
        assert!(is_newer("0.7.0", "0.7.0+dev"));
        assert!(!is_newer("0.6.9-dev", "0.7.0"));
        assert!(is_newer("0.8.0-dev", "0.7.0"));
        assert!(!is_newer("0.0.0", "0.6.0"));
        assert!(is_newer("0.6.0", "0.0.0"));
    }

    #[test]
    fn the_bundled_cua_replaces_only_what_it_is_newer_than() {
        // Nothing there, or something that is not a cua: install.
        assert!(bundled_replaces(None, "cua 0.7.0"));
        assert!(bundled_replaces(Some(""), "cua 0.7.0"));
        assert!(bundled_replaces(
            Some("Traceback (most recent call last):"),
            "cua 0.7.0"
        ));
        // Older installed: upgrade.
        assert!(bundled_replaces(Some("cua 0.6.0"), "cua 0.7.0"));
        assert!(bundled_replaces(Some("cua 0.7.0-rc.1"), "cua 0.7.0"));
        // Newer installed (the UL2 case: CLI 0.7.99, app bundling 0.6.0): keep.
        assert!(!bundled_replaces(Some("cua 0.7.99"), "cua 0.6.0"));
        assert!(!bundled_replaces(Some("cua 0.7.0"), "cua 0.6.0"));
        // Same version: keep (a different build of it too).
        assert!(!bundled_replaces(Some("cua 0.7.0"), "cua 0.7.0"));
        assert!(!bundled_replaces(
            Some("cua 0.7.0 (aaa)"),
            "cua 0.7.0 (bbb)"
        ));
        // Dev builds.
        assert!(bundled_replaces(Some("cua 0.7.0-dev"), "cua 0.7.0"));
        assert!(!bundled_replaces(Some("cua 0.7.0"), "cua 0.7.0-dev"));
        assert!(!bundled_replaces(
            Some("cua 0.7.0-dev (abc)"),
            "cua 0.7.0-dev (abc)"
        ));
        // A bundled one with no readable version never replaces a working cua.
        assert!(!bundled_replaces(Some("cua 0.7.0"), "cua nightly"));
    }
}
