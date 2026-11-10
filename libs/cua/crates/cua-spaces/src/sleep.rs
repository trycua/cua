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
/// Marks an agent home (also the agent home label).
const LABEL_HOME: &str = "cua.home";
/// Marks a Space kept out of the lists (the home warming for Home).
const LABEL_HIDDEN: &str = "cua.hidden";

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

    /// The setting a Space's labels carry. With no (readable) `cua.sleep`
    /// label a Space a helper made for a task, an agent home (`cua.home`) or
    /// the hidden home warming for Home (`cua.hidden`), is `auto`, so homes
    /// made before the setting existed pick it up without being recreated;
    /// any other Space is `never`.
    pub fn from_labels(labels: &BTreeMap<String, String>) -> Sleep {
        if let Some(s) = labels.get(LABEL_SLEEP).and_then(|w| Sleep::parse(w)) {
            return s;
        }
        let helper =
            labels.get(LABEL_HOME).is_some_and(|v| v == "1") || labels.contains_key(LABEL_HIDDEN);
        if helper { Sleep::Auto } else { Sleep::Never }
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

    /// The sleep setting of the Space `id`: what `set_sleep` stored for this
    /// very Space, else the label it was created with, else `never`.
    pub fn sleep_of(&self, id: &str) -> Sleep {
        let labels = self
            .list()
            .ok()
            .and_then(|l| l.into_iter().find(|s| s.id == id))
            .map(|s| s.labels)
            .unwrap_or_default();
        self.sleep_with_labels(id, &labels)
    }

    /// [`Self::sleep_of`] when the caller already has the Space's labels.
    pub fn sleep_with_labels(&self, id: &str, labels: &BTreeMap<String, String>) -> Sleep {
        match self.stored(id).and_then(|e| e.sleep) {
            Some(w) => Sleep::parse(&w).unwrap_or_default(),
            None => Sleep::from_labels(labels),
        }
    }

    /// Changes the sleep setting of a registered Space. `never` is stored
    /// too, so it wins over a label that said `auto`.
    pub fn set_sleep(&self, id: &str, sleep: Sleep) -> Result<()> {
        let id = self.resolve(id)?.to_string();
        self.update_stored(&id, |e| e.sleep = Some(sleep.word()))
    }

    /// Keeps a Space awake for `dur` (at most [`MAX_KEEP_AWAKE`] from now),
    /// whatever its sleep setting says. Says until when. The reason is shown
    /// to the person.
    pub fn keep_awake(&self, id: &str, dur: Duration, reason: &str) -> Result<KeepAwake> {
        let id = self.resolve(id)?.to_string();
        let capped = dur > MAX_KEEP_AWAKE;
        let until = now_unix() + dur.min(MAX_KEEP_AWAKE).as_secs();
        let reason: String = reason.chars().take(200).collect();
        self.update_stored(&id, |e| {
            e.keep_until_unix = Some(until);
            e.keep_reason = Some(reason.clone());
        })?;
        Ok(KeepAwake {
            until_unix: until,
            reason,
            capped,
        })
    }

    /// Ends a keep-awake early.
    pub fn clear_keep_awake(&self, id: &str) -> Result<()> {
        let id = self.resolve(id)?.to_string();
        self.update_stored(&id, |e| {
            e.keep_until_unix = None;
            e.keep_reason = None;
        })
    }

    /// The keep-awake of `id` while it holds.
    pub fn keep_awake_of(&self, id: &str) -> Option<KeepAwake> {
        let e = self.stored(id)?;
        let until = e.keep_until_unix.filter(|u| *u > now_unix())?;
        Some(KeepAwake {
            until_unix: until,
            reason: e.keep_reason.unwrap_or_default(),
            capped: false,
        })
    }

    fn stored(&self, id: &str) -> Option<Entry> {
        let at = self.sleep_created_at(id);
        let mut all = load(self.home_dir());
        all.spaces.remove(id).filter(|e| e.created == at)
    }

    fn sleep_created_at(&self, id: &str) -> u64 {
        self.registry()
            .meta(id)
            .map(|m| m.created_at_unix)
            .unwrap_or(0)
    }

    fn update_stored(&self, id: &str, f: impl FnOnce(&mut Entry)) -> Result<()> {
        let at = self.sleep_created_at(id);
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut all = load(self.home_dir());
        let mut e = all
            .spaces
            .remove(id)
            .filter(|e| e.created == at)
            .unwrap_or(Entry {
                created: at,
                ..Default::default()
            });
        f(&mut e);
        all.spaces.insert(id.to_owned(), e);
        save(self.home_dir(), &all)
            .map_err(|e| Error::invalid(format!("could not save the sleep setting: {e}")))
    }
}

/// The longest a keep-awake lasts.
pub const MAX_KEEP_AWAKE: Duration = Duration::from_secs(2 * 3600);

/// A keep-awake that holds.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct KeepAwake {
    /// When it ends (Unix seconds).
    pub until_unix: u64,
    /// Why, in the words of whoever asked.
    pub reason: String,
    /// The ask was longer than [`MAX_KEEP_AWAKE`] and was cut.
    pub capped: bool,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Entry {
    /// When the Space was created: a Space of the same name made later does
    /// not inherit the setting.
    created: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sleep: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    keep_until_unix: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    keep_reason: Option<String>,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct File {
    #[serde(default)]
    spaces: BTreeMap<String, Entry>,
}

static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn path(home: &std::path::Path) -> std::path::PathBuf {
    home.join("sleep.json")
}

fn load(home: &std::path::Path) -> File {
    std::fs::read(path(home))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save(home: &std::path::Path, f: &File) -> std::io::Result<()> {
    let p = path(home);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = p.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(f).unwrap_or_default())?;
    std::fs::rename(&tmp, &p)
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
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
    fn helper_made_spaces_sleep_unless_they_say_otherwise() {
        let mut l = BTreeMap::new();
        l.insert("cua.home".to_string(), "1".to_string());
        assert_eq!(Sleep::from_labels(&l), Sleep::Auto);
        l.insert(LABEL_SLEEP.into(), "never".into());
        assert_eq!(Sleep::from_labels(&l), Sleep::Never);
        let mut w = BTreeMap::new();
        w.insert("cua.hidden".to_string(), "home".to_string());
        assert_eq!(Sleep::from_labels(&w), Sleep::Auto);
        let mut u = BTreeMap::new();
        u.insert("team".to_string(), "research".to_string());
        assert_eq!(Sleep::from_labels(&u), Sleep::Never);
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
