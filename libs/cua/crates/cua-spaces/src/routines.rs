//! Routines: recurring tasks a Bot runs on a schedule.
//!
//! A routine is a saved prompt plus a clock, owned by one Bot (one long-lived
//! agent thread). When it comes due the store hands it to a
//! [`RoutineRunner`], which gives that Bot another turn in the shared Space.
//! The store keeps three things apart:
//!
//! - **persistence**: every mutation is written through a [`RoutineStorage`]
//!   as a JSON array, the same shape the TypeScript and Swift SDKs write;
//! - **the clock**: [`RoutineStore::tick`] takes the instant to evaluate, so
//!   the scheduler is testable without sleeping;
//! - **the firing**: delegated to the runner, so the store never sees a Space.
//!
//! A scheduler that was asleep fires a due routine **once** on waking, never
//! once per missed slot, and a refused firing still uses its slot.
//!
//! ```
//! use cua_spaces::routines::{MemoryStorage, RoutineStore, Schedule};
//! let mut store = RoutineStore::new(Box::new(MemoryStorage::default()));
//! let r = store.create("inbox", "Morning sweep", "Triage the inbox",
//!                      Schedule::DailyAt { hour: 8, minute: 0 }, true, chrono::Utc::now());
//! assert_eq!(r.schedule.label(), "Every day at 8:00 AM");
//! ```

use chrono::{
    DateTime, Datelike, Duration as ChronoDuration, Local, LocalResult, NaiveDate, TimeZone, Utc,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// The three recurrence shapes. `weekday` is 1 = Sunday … 7 = Saturday.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Schedule {
    EveryMinutes {
        minutes: i64,
    },
    DailyAt {
        hour: u32,
        minute: u32,
    },
    WeeklyOn {
        weekday: u32,
        hour: u32,
        minute: u32,
    },
}

const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// `8:05 AM`.
pub fn clock_label(hour: u32, minute: u32) -> String {
    let h = if hour.is_multiple_of(12) {
        12
    } else {
        hour % 12
    };
    format!("{h}:{minute:02} {}", if hour < 12 { "AM" } else { "PM" })
}

/// Local midnight-based slot: `date` at `hour:minute:00`, or the next
/// instant that exists when a DST gap swallows it.
fn local_at(date: NaiveDate, hour: u32, minute: u32) -> Option<DateTime<Utc>> {
    let naive = date.and_hms_opt(hour, minute, 0)?;
    match Local.from_local_datetime(&naive) {
        LocalResult::Single(t) => Some(t.with_timezone(&Utc)),
        LocalResult::Ambiguous(first, _) => Some(first.with_timezone(&Utc)),
        LocalResult::None => Local
            .from_local_datetime(&(naive + ChronoDuration::hours(1)))
            .earliest()
            .map(|t| t.with_timezone(&Utc)),
    }
}

impl Schedule {
    /// The first slot strictly after `reference`, in local time; `None` for
    /// an invalid schedule.
    pub fn next_fire(&self, reference: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let local = reference.with_timezone(&Local).date_naive();
        match *self {
            Schedule::EveryMinutes { minutes } => {
                (minutes > 0).then(|| reference + ChronoDuration::minutes(minutes))
            }
            Schedule::DailyAt { hour, minute } => {
                let today = local_at(local, hour, minute)?;
                if today > reference {
                    Some(today)
                } else {
                    local_at(local.succ_opt()?, hour, minute)
                }
            }
            Schedule::WeeklyOn {
                weekday,
                hour,
                minute,
            } => {
                let want = (weekday + 6) % 7; // 0 = Sunday
                let have = local.weekday().num_days_from_sunday();
                let ahead = (want + 7 - have) % 7;
                let day = local + ChronoDuration::days(ahead as i64);
                let c = local_at(day, hour, minute)?;
                if c > reference {
                    Some(c)
                } else {
                    local_at(day + ChronoDuration::days(7), hour, minute)
                }
            }
        }
    }

    /// The one-line description: `Every day at 8:00 AM`.
    pub fn label(&self) -> String {
        match *self {
            Schedule::EveryMinutes { minutes: 1 } => "Every minute".into(),
            Schedule::EveryMinutes { minutes: 60 } => "Every hour".into(),
            Schedule::EveryMinutes { minutes } if minutes > 0 && minutes % 60 == 0 => {
                format!("Every {} hours", minutes / 60)
            }
            Schedule::EveryMinutes { minutes } => format!("Every {minutes} minutes"),
            Schedule::DailyAt { hour, minute } => {
                format!("Every day at {}", clock_label(hour, minute))
            }
            Schedule::WeeklyOn {
                weekday,
                hour,
                minute,
            } => format!(
                "Every {} at {}",
                WEEKDAYS[((weekday + 6) % 7) as usize],
                clock_label(hour, minute)
            ),
        }
    }
}

/// ISO 8601 in UTC with whole seconds, the portable form every SDK reads.
pub mod iso {
    use chrono::{DateTime, SecondsFormat, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn format(d: &DateTime<Utc>) -> String {
        d.to_rfc3339_opts(SecondsFormat::Secs, true)
    }
    pub fn parse(s: &str) -> Result<DateTime<Utc>, chrono::ParseError> {
        DateTime::parse_from_rfc3339(s).map(|d| d.with_timezone(&Utc))
    }
    pub fn serialize<S: Serializer>(d: &DateTime<Utc>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format(d))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<DateTime<Utc>, D::Error> {
        let s = String::deserialize(d)?;
        parse(&s).map_err(serde::de::Error::custom)
    }
    pub mod option {
        use chrono::{DateTime, Utc};
        use serde::{Deserialize, Deserializer, Serializer};
        pub fn serialize<S: Serializer>(
            d: &Option<DateTime<Utc>>,
            s: S,
        ) -> Result<S::Ok, S::Error> {
            match d {
                Some(d) => super::serialize(d, s),
                None => s.serialize_none(),
            }
        }
        pub fn deserialize<'de, D: Deserializer<'de>>(
            d: D,
        ) -> Result<Option<DateTime<Utc>>, D::Error> {
            Option::<String>::deserialize(d)?
                .map(|s| super::parse(&s).map_err(serde::de::Error::custom))
                .transpose()
        }
    }
}

fn whole_seconds(d: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::from_timestamp(d.timestamp(), 0).unwrap_or(d)
}

fn default_true() -> bool {
    true
}

/// One routine. The JSON keys are the ones every SDK writes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Routine {
    pub id: String,
    /// The Bot that runs it. A routine never spans Bots; a group chat does.
    #[serde(rename = "botID")]
    pub bot_id: String,
    pub title: String,
    /// The text handed to the Bot when it fires.
    pub prompt: String,
    pub schedule: Schedule,
    #[serde(rename = "isEnabled", default = "default_true")]
    pub is_enabled: bool,
    #[serde(rename = "createdAt", with = "iso")]
    pub created_at: DateTime<Utc>,
    /// When the scheduler last started a firing.
    #[serde(
        rename = "lastFiredAt",
        with = "iso::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub last_fired_at: Option<DateTime<Utc>>,
    /// The agent run the last firing produced.
    #[serde(rename = "lastRunID", default, skip_serializing_if = "Option::is_none")]
    pub last_run_id: Option<String>,
    /// The scheduler's words about the last firing.
    #[serde(
        rename = "lastOutcome",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub last_outcome: Option<String>,
}

impl Routine {
    /// When this routine next fires after `reference`; `None` when disabled.
    pub fn next_fire(&self, reference: DateTime<Utc>) -> Option<DateTime<Utc>> {
        if !self.is_enabled {
            return None;
        }
        let from = self.last_fired_at.map_or(reference, |l| l.max(reference));
        self.schedule.next_fire(from)
    }

    /// Whether the scheduler should fire it at `now`. Measured from the last
    /// firing, not the tick, so a sleeping scheduler fires once on waking.
    pub fn is_due(&self, now: DateTime<Utc>) -> bool {
        self.is_enabled
            && self
                .schedule
                .next_fire(self.last_fired_at.unwrap_or(self.created_at))
                .is_some_and(|next| next <= now)
    }

    /// The text a runner hands the Bot: `[routine] <title>: <prompt>`.
    pub fn turn_text(&self) -> String {
        format!("{ROUTINE_PREFIX} {}: {}", self.title, self.prompt)
    }
}

/// Marks a routine-originated turn in a transcript.
pub const ROUTINE_PREFIX: &str = "[routine]";

/// What happened when a routine fired. A refusal is not a failure.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RoutineFiring {
    Started {
        #[serde(rename = "runId")]
        run_id: String,
    },
    Refused {
        reason: String,
    },
    Failed {
        reason: String,
    },
}

impl RoutineFiring {
    /// `started run <id>`, `refused: <why>`, `failed: <why>`.
    pub fn summary(&self) -> String {
        match self {
            RoutineFiring::Started { run_id } => format!("started run {run_id}"),
            RoutineFiring::Refused { reason } => format!("refused: {reason}"),
            RoutineFiring::Failed { reason } => format!("failed: {reason}"),
        }
    }
    pub fn run_id(&self) -> Option<&str> {
        match self {
            RoutineFiring::Started { run_id } => Some(run_id),
            _ => None,
        }
    }
}

/// Gives a Bot its routine turn. Never errors: a refusal is a result.
#[async_trait::async_trait]
pub trait RoutineRunner: Send + Sync {
    async fn fire(&self, routine: &Routine) -> RoutineFiring;
}

/// Where the routine list lives.
pub trait RoutineStorage: Send + Sync {
    /// `None` when nothing was saved yet.
    fn load(&self) -> std::io::Result<Option<String>>;
    fn save(&self, json: &str) -> std::io::Result<()>;
}

/// A JSON file (written atomically).
pub struct FileStorage(pub PathBuf);

impl RoutineStorage for FileStorage {
    fn load(&self) -> std::io::Result<Option<String>> {
        match std::fs::read_to_string(&self.0) {
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }
    fn save(&self, json: &str) -> std::io::Result<()> {
        if let Some(dir) = self.0.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.0.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &self.0)
    }
}

/// In memory.
#[derive(Default)]
pub struct MemoryStorage(pub std::sync::Mutex<Option<String>>);

impl RoutineStorage for MemoryStorage {
    fn load(&self) -> std::io::Result<Option<String>> {
        Ok(self.0.lock().map(|g| g.clone()).unwrap_or_default())
    }
    fn save(&self, json: &str) -> std::io::Result<()> {
        if let Ok(mut g) = self.0.lock() {
            *g = Some(json.to_string());
        }
        Ok(())
    }
}

/// One line of the scheduler's log, newest first.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FiringRecord {
    #[serde(rename = "routineID")]
    pub routine_id: String,
    pub title: String,
    #[serde(with = "iso")]
    pub at: DateTime<Utc>,
    pub firing: RoutineFiring,
}

/// A fresh id (UUID v4 form, upper case, like the Swift SDK's).
pub fn new_id() -> String {
    let mut v: [u8; 16] = rand::random();
    v[6] = (v[6] & 0x0f) | 0x40;
    v[8] = (v[8] & 0x3f) | 0x80;
    let hex: String = v.iter().map(|x| format!("{x:02X}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Routines: storage, editing, and the clock that fires them.
pub struct RoutineStore {
    pub routines: Vec<Routine>,
    /// What the scheduler did, newest first (at most 50).
    pub log: Vec<FiringRecord>,
    storage: Box<dyn RoutineStorage>,
    runner: Option<Arc<dyn RoutineRunner>>,
}

impl RoutineStore {
    pub fn new(storage: Box<dyn RoutineStorage>) -> Self {
        let mut s = Self {
            routines: vec![],
            log: vec![],
            storage,
            runner: None,
        };
        s.load();
        s
    }

    pub fn attach(&mut self, runner: Arc<dyn RoutineRunner>) {
        self.runner = Some(runner);
    }

    pub fn runner(&self) -> Option<Arc<dyn RoutineRunner>> {
        self.runner.clone()
    }

    /// Reads the list back. A corrupt file starts empty, with a logged complaint.
    pub fn load(&mut self) {
        self.routines = match self.storage.load() {
            Ok(Some(s)) if !s.trim().is_empty() => match serde_json::from_str(&s) {
                Ok(v) => v,
                Err(e) => {
                    self.note(format!(
                        "routines file could not be read ({e}); starting empty"
                    ));
                    vec![]
                }
            },
            Ok(_) => vec![],
            Err(e) => {
                self.note(format!(
                    "routines file could not be read ({e}); starting empty"
                ));
                vec![]
            }
        };
    }

    pub fn save(&mut self) -> bool {
        let json = serde_json::to_string_pretty(&self.routines).unwrap_or_else(|_| "[]".into());
        match self.storage.save(&json) {
            Ok(()) => true,
            Err(e) => {
                self.note(format!("could not save routines: {e}"));
                false
            }
        }
    }

    pub fn routines_for(&self, bot_id: &str) -> Vec<Routine> {
        let mut v: Vec<_> = self
            .routines
            .iter()
            .filter(|r| r.bot_id == bot_id)
            .cloned()
            .collect();
        v.sort_by_key(|r| r.created_at);
        v
    }

    pub fn routine(&self, id: &str) -> Option<&Routine> {
        self.routines.iter().find(|r| r.id == id)
    }

    pub fn create(
        &mut self,
        bot_id: &str,
        title: &str,
        prompt: &str,
        schedule: Schedule,
        enabled: bool,
        now: DateTime<Utc>,
    ) -> Routine {
        let r = Routine {
            id: new_id(),
            bot_id: bot_id.into(),
            title: title.into(),
            prompt: prompt.into(),
            schedule,
            is_enabled: enabled,
            created_at: whole_seconds(now),
            last_fired_at: None,
            last_run_id: None,
            last_outcome: None,
        };
        self.routines.push(r.clone());
        self.save();
        r
    }

    pub fn update(&mut self, routine: Routine) {
        if let Some(r) = self.routines.iter_mut().find(|r| r.id == routine.id) {
            *r = routine;
            self.save();
        }
    }

    pub fn delete(&mut self, id: &str) {
        self.routines.retain(|r| r.id != id);
        self.save();
    }

    /// Enable or disable without deleting; the firing history is kept.
    pub fn set_enabled(&mut self, id: &str, enabled: bool) {
        if let Some(r) = self.routines.iter_mut().find(|r| r.id == id) {
            r.is_enabled = enabled;
            self.save();
        }
    }

    pub fn due(&self, now: DateTime<Utc>) -> Vec<Routine> {
        self.routines
            .iter()
            .filter(|r| r.is_due(now))
            .cloned()
            .collect()
    }

    /// Records a firing: the slot is used even on a refusal (otherwise a busy
    /// Bot is hammered every tick).
    pub fn record(
        &mut self,
        routine: &Routine,
        firing: RoutineFiring,
        now: DateTime<Utc>,
    ) -> FiringRecord {
        let now = whole_seconds(now);
        if let Some(r) = self.routines.iter_mut().find(|r| r.id == routine.id) {
            r.last_fired_at = Some(now);
            if let Some(run) = firing.run_id() {
                r.last_run_id = Some(run.to_string());
            }
            r.last_outcome = Some(firing.summary());
            self.save();
        }
        let rec = FiringRecord {
            routine_id: routine.id.clone(),
            title: routine.title.clone(),
            at: now,
            firing,
        };
        self.log.insert(0, rec.clone());
        self.log.truncate(50);
        rec
    }

    /// Fires one routine now, whatever the clock says ("Run now").
    pub async fn fire(&mut self, routine: &Routine, now: DateTime<Utc>) -> FiringRecord {
        let firing = fire_with(self.runner.clone(), routine).await;
        self.record(routine, firing, now)
    }

    /// Evaluates the clock once and fires whatever is due.
    pub async fn tick(&mut self, now: DateTime<Utc>) -> Vec<FiringRecord> {
        let mut out = vec![];
        for r in self.due(now) {
            out.push(self.fire(&r, now).await);
        }
        out
    }

    fn note(&mut self, text: String) {
        self.log.insert(
            0,
            FiringRecord {
                routine_id: String::new(),
                title: "Routines".into(),
                at: whole_seconds(Utc::now()),
                firing: RoutineFiring::Failed { reason: text },
            },
        );
    }
}

async fn fire_with(runner: Option<Arc<dyn RoutineRunner>>, routine: &Routine) -> RoutineFiring {
    match runner {
        Some(r) => r.fire(routine).await,
        None => RoutineFiring::Failed {
            reason: "no runner attached: not connected to a Space".into(),
        },
    }
}

/// One tick against a shared store, without holding its lock while a Bot is
/// being started (so readers are never blocked by a firing).
pub async fn tick_shared(
    store: &tokio::sync::Mutex<RoutineStore>,
    now: DateTime<Utc>,
) -> Vec<FiringRecord> {
    let (due, runner) = {
        let s = store.lock().await;
        (s.due(now), s.runner())
    };
    let mut out = vec![];
    for r in due {
        let firing = fire_with(runner.clone(), &r).await;
        out.push(store.lock().await.record(&r, firing, now));
    }
    out
}

/// The background loop: one for every routine, ticking every `every`.
/// Abort the handle to stop it.
pub fn spawn_scheduler(
    store: Arc<tokio::sync::Mutex<RoutineStore>>,
    every: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tick_shared(&store, Utc::now()).await;
            tokio::time::sleep(every).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;
    use std::sync::Mutex;

    fn t0() -> DateTime<Utc> {
        // Friday 25 September 2026, 9:30 local.
        local_at(NaiveDate::from_ymd_opt(2026, 9, 25).unwrap(), 9, 30).unwrap()
    }

    #[derive(Default)]
    struct Runner {
        fired: Mutex<Vec<String>>,
        refuse: bool,
    }
    #[async_trait::async_trait]
    impl RoutineRunner for Runner {
        async fn fire(&self, r: &Routine) -> RoutineFiring {
            let mut f = self.fired.lock().unwrap();
            f.push(r.id.clone());
            if self.refuse {
                RoutineFiring::Refused {
                    reason: "mid-turn".into(),
                }
            } else {
                RoutineFiring::Started {
                    run_id: format!("run-{}", f.len() - 1),
                }
            }
        }
    }

    #[test]
    fn slots_are_strictly_after_the_reference_in_local_time() {
        let t = t0();
        assert_eq!(
            Schedule::EveryMinutes { minutes: 5 }.next_fire(t),
            Some(t + ChronoDuration::minutes(5))
        );
        assert_eq!(Schedule::EveryMinutes { minutes: 0 }.next_fire(t), None);
        let l = |d: DateTime<Utc>| {
            let x = d.with_timezone(&Local);
            (
                x.day(),
                x.hour(),
                x.minute(),
                x.weekday().num_days_from_sunday(),
            )
        };
        assert_eq!(
            l(Schedule::DailyAt { hour: 8, minute: 0 }
                .next_fire(t)
                .unwrap()),
            (26, 8, 0, 6)
        );
        assert_eq!(
            l(Schedule::DailyAt {
                hour: 10,
                minute: 15
            }
            .next_fire(t)
            .unwrap()),
            (25, 10, 15, 5)
        );
        assert_eq!(
            l(Schedule::DailyAt {
                hour: 9,
                minute: 30
            }
            .next_fire(t)
            .unwrap())
            .0,
            26
        );
        let monday = Schedule::WeeklyOn {
            weekday: 2,
            hour: 9,
            minute: 0,
        };
        assert_eq!(l(monday.next_fire(t).unwrap()), (28, 9, 0, 1));
        let friday = Schedule::WeeklyOn {
            weekday: 6,
            hour: 17,
            minute: 0,
        };
        assert_eq!(l(friday.next_fire(t).unwrap()), (25, 17, 0, 5));
    }

    #[test]
    fn labels_match_the_other_sdks() {
        assert_eq!(
            Schedule::EveryMinutes { minutes: 1 }.label(),
            "Every minute"
        );
        assert_eq!(
            Schedule::EveryMinutes { minutes: 15 }.label(),
            "Every 15 minutes"
        );
        assert_eq!(Schedule::EveryMinutes { minutes: 60 }.label(), "Every hour");
        assert_eq!(
            Schedule::EveryMinutes { minutes: 120 }.label(),
            "Every 2 hours"
        );
        assert_eq!(
            Schedule::DailyAt { hour: 0, minute: 5 }.label(),
            "Every day at 12:05 AM"
        );
        assert_eq!(
            Schedule::WeeklyOn {
                weekday: 2,
                hour: 13,
                minute: 0
            }
            .label(),
            "Every Monday at 1:00 PM"
        );
    }

    #[tokio::test]
    async fn fires_once_after_sleeping_and_persists() {
        let storage = Arc::new(MemoryStorage::default());
        struct Shared(Arc<MemoryStorage>);
        impl RoutineStorage for Shared {
            fn load(&self) -> std::io::Result<Option<String>> {
                self.0.load()
            }
            fn save(&self, j: &str) -> std::io::Result<()> {
                self.0.save(j)
            }
        }
        let runner = Arc::new(Runner::default());
        let mut s = RoutineStore::new(Box::new(Shared(storage.clone())));
        s.attach(runner.clone());
        let r = s.create(
            "inbox",
            "Sweep",
            "Triage",
            Schedule::EveryMinutes { minutes: 1 },
            true,
            t0(),
        );
        assert!(s.tick(t0() + ChronoDuration::seconds(30)).await.is_empty());
        let wake = t0() + ChronoDuration::hours(12);
        let fired = s.tick(wake).await;
        assert_eq!(fired.len(), 1, "a backlog fires once");
        assert!(s.tick(wake).await.is_empty());
        let reloaded = RoutineStore::new(Box::new(Shared(storage.clone())));
        let saved = reloaded.routine(&r.id).unwrap();
        assert_eq!(saved.last_fired_at, Some(wake));
        assert_eq!(saved.last_run_id.as_deref(), Some("run-0"));
        assert_eq!(saved.last_outcome.as_deref(), Some("started run run-0"));
        assert!(reloaded.due(wake).is_empty());
        // The portable shape.
        let raw = storage.load().unwrap().unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let mut keys: Vec<_> = v[0].as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "botID",
                "createdAt",
                "id",
                "isEnabled",
                "lastFiredAt",
                "lastOutcome",
                "lastRunID",
                "prompt",
                "schedule",
                "title"
            ]
        );
        assert_eq!(
            v[0]["schedule"],
            serde_json::json!({"kind": "everyMinutes", "minutes": 1})
        );
        assert!(!v[0]["createdAt"].as_str().unwrap().contains('.'));
    }

    #[tokio::test]
    async fn refusal_uses_the_slot_and_no_runner_fails() {
        let mut s = RoutineStore::new(Box::new(MemoryStorage::default()));
        s.attach(Arc::new(Runner {
            refuse: true,
            ..Default::default()
        }));
        let r = s.create(
            "b",
            "T",
            "p",
            Schedule::EveryMinutes { minutes: 1 },
            true,
            t0(),
        );
        let at = t0() + ChronoDuration::seconds(61);
        let rec = s.tick(at).await;
        assert_eq!(rec[0].firing.summary(), "refused: mid-turn");
        assert!(s.due(at).is_empty());
        assert_eq!(
            s.routine(&r.id).unwrap().last_outcome.as_deref(),
            Some("refused: mid-turn")
        );
        let mut bare = RoutineStore::new(Box::new(MemoryStorage::default()));
        let r2 = bare.create(
            "b",
            "T",
            "p",
            Schedule::EveryMinutes { minutes: 1 },
            true,
            t0(),
        );
        assert!(matches!(
            bare.fire(&r2, t0()).await.firing,
            RoutineFiring::Failed { .. }
        ));
        assert_eq!(r2.turn_text(), "[routine] T: p");
    }

    #[test]
    fn reads_what_the_swift_sdk_writes_and_survives_corruption() {
        let swift = r#"[{"botID":"b","createdAt":"2026-09-25T09:30:00Z","id":"X","isEnabled":false,"lastFiredAt":"2026-09-25T10:30:00Z","prompt":"p","schedule":{"hour":8,"kind":"dailyAt","minute":0},"title":"T"}]"#;
        let s = RoutineStore::new(Box::new(MemoryStorage(Mutex::new(Some(swift.into())))));
        assert!(!s.routines[0].is_enabled);
        assert_eq!(
            s.routines[0].schedule,
            Schedule::DailyAt { hour: 8, minute: 0 }
        );
        let bad = RoutineStore::new(Box::new(MemoryStorage(Mutex::new(Some("{nope".into())))));
        assert!(bad.routines.is_empty());
        assert!(bad.log[0].firing.summary().contains("could not be read"));
    }

    #[tokio::test]
    async fn the_scheduler_loop_fires_a_due_routine() {
        let runner = Arc::new(Runner::default());
        let mut s = RoutineStore::new(Box::new(MemoryStorage::default()));
        s.attach(runner.clone());
        s.create(
            "b",
            "T",
            "p",
            Schedule::EveryMinutes { minutes: 1 },
            true,
            Utc::now() - ChronoDuration::seconds(61),
        );
        let store = Arc::new(tokio::sync::Mutex::new(s));
        let h = spawn_scheduler(store.clone(), Duration::from_millis(20));
        for _ in 0..100 {
            if !runner.fired.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::time::sleep(Duration::from_millis(80)).await;
        h.abort();
        assert_eq!(runner.fired.lock().unwrap().len(), 1);
        assert!(store.lock().await.routines[0].last_fired_at.is_some());
    }
}
