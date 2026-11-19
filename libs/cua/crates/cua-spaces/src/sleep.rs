//! Sleep: what a Space does when nothing needs it.
//!
//! The setting is chosen when a Space is created and travels with it as the
//! label [`LABEL_SLEEP`]:
//!
//! - `never` (shown as "Keep running"): the Space is never put to sleep. A
//!   Space with no stored setting is `never`, so nothing that exists changes.
//! - `auto`: the host puts the Space to sleep once nothing needs it and wakes
//!   it when something does (a message, a task).
//! - a duration (`90s`, `10m`, `2h`): like `auto`, but only after the Space
//!   was quiet for that long.
//!
//! Helpers that make a Space to run a task for the person (an agent home)
//! create it with `auto`; Spaces people create themselves are `never`.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::{Error, Result, SpaceId, Spaces};

/// The label that carries a Space's sleep setting.
pub const LABEL_SLEEP: &str = "cua.sleep";

/// How long an agent run waits for its next message before it exits, in a
/// Space that sleeps on its own. Elsewhere it is 30 minutes
/// (`cua_agents::runs::DEFAULT_IDLE_EXIT_SECS`).
pub const AUTO_IDLE_EXIT_SECS: u32 = 60;

/// How long a Space that sleeps on its own must have nothing running (no
/// turn, no process of its own) before the host stops it.
pub const AUTO_QUIET: Duration = Duration::from_secs(30);

/// The shortest idle duration a person can ask for.
pub const MIN_IDLE: Duration = Duration::from_secs(10);

/// The longest idle duration a person can ask for.
pub const MAX_IDLE: Duration = Duration::from_secs(7 * 24 * 3600);

/// A Space's sleep setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sleep {
    /// Never put to sleep ("Keep running").
    #[default]
    Never,
    /// Put to sleep when nothing needs it.
    Auto,
    /// Put to sleep after this long without anything needing it.
    After(Duration),
}

impl Sleep {
    /// Parses `never`, `auto` or a duration such as `90s`, `10m`, `2h`
    /// (seconds when it has no unit). A duration is kept within
    /// [`MIN_IDLE`]..=[`MAX_IDLE`].
    pub fn parse(word: &str) -> Option<Sleep> {
        let w = word.trim().to_ascii_lowercase();
        match w.as_str() {
            "never" | "off" | "keep-running" | "keep_running" => return Some(Sleep::Never),
            "auto" | "on" => return Some(Sleep::Auto),
            _ => {}
        }
        let (digits, unit) = w.split_at(w.find(|c: char| !c.is_ascii_digit())?.min(w.len()));
        let n: u64 = digits.parse().ok()?;
        let secs = match unit {
            "" | "s" => n,
            "m" => n.checked_mul(60)?,
            "h" => n.checked_mul(3600)?,
            "d" => n.checked_mul(86_400)?,
            _ => return None,
        };
        let d = Duration::from_secs(secs);
        (MIN_IDLE..=MAX_IDLE)
            .contains(&d)
            .then_some(Sleep::After(d))
    }

    /// The word stored in the label and shown by the CLI.
    pub fn word(self) -> String {
        match self {
            Sleep::Never => "never".into(),
            Sleep::Auto => "auto".into(),
            Sleep::After(d) => {
                let s = d.as_secs();
                if s % 3600 == 0 {
                    format!("{}h", s / 3600)
                } else if s % 60 == 0 {
                    format!("{}m", s / 60)
                } else {
                    format!("{s}s")
                }
            }
        }
    }

    /// The setting a Space's labels carry; `never` when there is none or it
    /// does not read.
    pub fn from_labels(labels: &BTreeMap<String, String>) -> Sleep {
        labels
            .get(LABEL_SLEEP)
            .and_then(|w| Sleep::parse(w))
            .unwrap_or(Sleep::Never)
    }

    /// Whether the host may put the Space to sleep at all.
    pub fn sleeps(self) -> bool {
        !matches!(self, Sleep::Never)
    }

    /// How long the Space must be quiet before the host stops it.
    pub fn quiet_for(self) -> Duration {
        match self {
            Sleep::Never => Duration::MAX,
            Sleep::Auto => AUTO_QUIET,
            Sleep::After(d) => d.max(AUTO_QUIET),
        }
    }
}

impl Spaces {
    /// Puts a local Space to sleep: stops it with its disk kept and its
    /// memory given back, whatever its engine would do on a plain
    /// [`Spaces::stop`] (a container is suspended there, and a suspended
    /// container keeps its memory). Its child Spaces stop first. The caller
    /// has checked that nothing in it is running.
    pub async fn sleep(&self, space: &str) -> Result<()> {
        let id = self.resolve(space)?;
        let SpaceId::Local { name } = &id else {
            return Err(Error::invalid(format!(
                "{space} is not on this computer, so it cannot be put to sleep from here"
            )));
        };
        self.power_off_children(space, false).await;
        self.drop_connection(&id).await;
        self.sandboxes()
            .power_off_as(name, cua_sandbox_core::PowerControl::Stop)
            .await?;
        Ok(())
    }

    /// The sleep setting of the Space `id`, from its labels.
    pub fn sleep_of(&self, id: &str) -> Sleep {
        self.list()
            .ok()
            .and_then(|l| l.into_iter().find(|s| s.id == id))
            .map(|s| Sleep::from_labels(&s.labels))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_parse_and_print_back() {
        assert_eq!(Sleep::parse("never"), Some(Sleep::Never));
        assert_eq!(Sleep::parse(" Auto "), Some(Sleep::Auto));
        assert_eq!(
            Sleep::parse("90s"),
            Some(Sleep::After(Duration::from_secs(90)))
        );
        assert_eq!(
            Sleep::parse("10m"),
            Some(Sleep::After(Duration::from_secs(600)))
        );
        assert_eq!(
            Sleep::parse("2h"),
            Some(Sleep::After(Duration::from_secs(7200)))
        );
        for w in ["never", "auto", "90s", "10m", "2h"] {
            assert_eq!(
                Sleep::parse(&Sleep::parse(w).unwrap().word()),
                Sleep::parse(w)
            );
        }
    }

    #[test]
    fn nonsense_and_out_of_range_durations_do_not_parse() {
        for w in ["", "soon", "5", "5x", "-3m", "1s", "99d", "m"] {
            assert_eq!(Sleep::parse(w), None, "{w:?}");
        }
    }

    #[test]
    fn a_space_with_no_setting_never_sleeps() {
        let mut l = BTreeMap::new();
        assert_eq!(Sleep::from_labels(&l), Sleep::Never);
        l.insert(LABEL_SLEEP.into(), "garbage".into());
        assert_eq!(Sleep::from_labels(&l), Sleep::Never);
        l.insert(LABEL_SLEEP.into(), "auto".into());
        assert_eq!(Sleep::from_labels(&l), Sleep::Auto);
        assert!(!Sleep::Never.sleeps());
        assert!(Sleep::Auto.sleeps());
    }

    #[test]
    fn a_duration_is_never_shorter_than_the_quiet_window() {
        assert_eq!(Sleep::Auto.quiet_for(), AUTO_QUIET);
        assert_eq!(
            Sleep::After(Duration::from_secs(10)).quiet_for(),
            AUTO_QUIET
        );
        assert_eq!(
            Sleep::After(Duration::from_secs(600)).quiet_for(),
            Duration::from_secs(600)
        );
    }
}
