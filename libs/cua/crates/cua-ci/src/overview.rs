//! The read model of the CI Runners page (`cua ci overview`): setup steps,
//! usage over the last week, the jobs table (wait, run, cost, computer,
//! cloud, a pointer to the log), the autopilot's reasons and the budget.
//!
//! It is plain data built from files this computer already keeps (the pool
//! event log, the capacity ledgers, the target reports), so reading it
//! needs no network and no cloud credential. A field that only the service
//! knows (every repository the GitHub app can reach) is `None` here and is
//! filled by whoever has the service open (see `RepositoriesView`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::events::{Event, EventKind};
use crate::targets::AutopilotView;

/// One step of the setup checklist.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupStep {
    /// `connect_github`, `repositories`, `runner_label`, `first_job_passed`.
    pub id: String,
    /// Words for the checklist row.
    pub title: String,
    /// `done`, `todo` or `unknown` (only the service can say).
    pub state: String,
    /// One line of detail (`3 repositories can run jobs`).
    pub detail: String,
}

/// A repository and what it ran this week.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoUse {
    /// `owner/name`.
    pub name: String,
    /// `ready` once it ran a job.
    pub status: String,
    /// Jobs in the last 7 days.
    pub jobs_7d: u32,
    /// Unix ms of the last job (0: none).
    pub last_job_ms: u64,
}

/// A repository the GitHub connection can see.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoChoice {
    /// `owner/name`.
    pub name: String,
    /// Cua serves its jobs (it is on the list of repositories that can run
    /// jobs).
    pub allowed: bool,
    /// A private repository.
    #[serde(default)]
    pub private: bool,
}

/// The repositories the page lists.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoriesView {
    /// Repositories that ran jobs lately. Complete only when `all` is `None`.
    pub used: Vec<RepoUse>,
    /// Every repository the connection can see, with whether jobs of it are
    /// served, from the service (`cua ci overview --live`); `None` when this
    /// read had no service.
    pub all: Option<Vec<RepoChoice>>,
}

/// The GitHub connection, in the words the page needs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubView {
    /// A GitHub connection exists on this computer.
    pub connected: bool,
    /// `hosted` (the Cua CI GitHub app), `own` (a pool of your own GitHub
    /// app or token) or `none`.
    pub mode: String,
    /// The GitHub organization or account the connection serves.
    pub login: String,
    /// Where repositories are chosen on GitHub (empty when not connected).
    pub choose_url: String,
    /// A reading of the service failed (`--live`); the page says so quietly.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// The overview line.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    /// Jobs started in the last 24 hours.
    pub starts_24h: u32,
    /// Jobs finished in the last 7 days.
    pub jobs_7d: u32,
    /// Share of those that passed, 0 to 1 (`None` before any finished).
    pub pass_rate_7d: Option<f64>,
    /// Micro-dollars the finished jobs cost over 7 days (slot time at the
    /// computer's rate).
    pub spend_7d_micros: u64,
}

/// One row of the jobs table.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct JobRow {
    /// `<run_id>/<job_id>`.
    pub job_key: String,
    /// The job's name in the workflow (empty until it runs).
    pub name: String,
    /// `owner/name` when known.
    pub repo: String,
    /// `queued`, `running`, `passed` or `failed`.
    pub state: String,
    /// Unix ms the job was queued (0: unknown).
    pub queued_ms: u64,
    /// Ms from queued to the first step (the Wait column).
    pub wait_ms: Option<u64>,
    /// Ms the job ran (the Run column).
    pub run_ms: Option<u64>,
    /// Micro-dollars of slot time (the Cost column; `None` until known).
    pub cost_micros: Option<u64>,
    /// Micro-dollars with the computer's idle share, once it ended.
    pub all_in_micros: Option<u64>,
    /// How the computer started: `warm_pool`, `fork`, `cold_create`.
    pub start_mode: String,
    /// The computer's name (empty when the job ran on a local Space).
    pub machine: String,
    /// The computer type (`c7i.xlarge`).
    pub machine_type: String,
    /// The cloud (`aws`, `hetzner`, ...; empty for a local computer).
    pub cloud: String,
    /// Bought as spot.
    pub spot: bool,
    /// Which pool's event log holds the job's log lines and where
    /// (`cua ci logs --pool <pool>`): the slot of the job.
    pub log_pool: String,
    /// The slot whose log is the job's.
    pub log_slot: String,
}

/// The budget lines.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetView {
    /// Micro-dollars committed this month across the clouds.
    pub month_committed_micros: u64,
    /// The monthly budget across the clouds.
    pub monthly_micros: u64,
    /// The hard cap an hour, micro-dollars.
    pub hourly_cap_micros: u64,
    /// `ok`, `warn` or `exhausted` (the worst of the targets).
    pub state: String,
}

/// What one target decided (the autopilot) and its goal.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TargetPilot {
    /// The target (`aws`).
    pub target: String,
    /// `cheapest`, `balanced` or `fastest`.
    pub goal: String,
    /// `--warm N` fixed the reserve.
    pub warm: Option<u32>,
    /// The plans and their plain-words reasons.
    pub autopilot: Option<AutopilotView>,
}

/// Everything the CI Runners page shows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Overview {
    /// Unix ms the overview was built.
    pub generated_ms: u64,
    /// The checklist.
    pub setup: Vec<SetupStep>,
    /// The GitHub connection.
    pub github: GithubView,
    /// The repositories.
    pub repositories: RepositoriesView,
    /// The overview line.
    pub usage: Usage,
    /// The jobs table, newest first.
    pub jobs: Vec<JobRow>,
    /// The autopilot of each target.
    pub targets: Vec<TargetPilot>,
    /// The budget.
    pub budget: BudgetView,
}

const DAY_MS: u64 = 86_400_000;

/// The jobs in an event log, grouped by `job_key`, newest first. Events
/// without a key (a pool that does not record one) are not jobs here.
pub fn jobs_from_events(pool: &str, events: &[Event], now_ms: u64, since_ms: u64) -> Vec<JobRow> {
    let key = |e: &Event| {
        e.extra
            .get("job_key")
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    let mut rows: BTreeMap<String, JobRow> = BTreeMap::new();
    for e in events.iter().filter(|e| e.ts_ms >= since_ms) {
        let Some(k) = key(e) else { continue };
        let r = rows.entry(k.clone()).or_insert_with(|| JobRow {
            job_key: k.clone(),
            state: "queued".into(),
            log_pool: pool.to_string(),
            ..JobRow::default()
        });
        if let Some(repo) = e.extra.get("repo").and_then(|v| v.as_str()) {
            r.repo = repo.to_string();
        }
        if let Some(q) = e.extra.get("queued_at_ms").and_then(|v| v.as_u64()) {
            r.queued_ms = q;
        }
        if let Some(s) = &e.slot {
            r.log_slot = s.clone();
        }
        match e.kind {
            EventKind::JobQueued => {
                if r.queued_ms == 0 {
                    r.queued_ms = e.ts_ms;
                }
            }
            EventKind::VmClaimed => {
                if r.queued_ms == 0 {
                    r.queued_ms = e.ts_ms;
                }
                if let Some(m) = e.extra.get("start_mode").and_then(|v| v.as_str()) {
                    r.start_mode = m.to_string();
                }
            }
            EventKind::JobStarted => {
                if r.wait_ms.is_none() && r.queued_ms > 0 {
                    r.wait_ms = Some(e.ts_ms.saturating_sub(r.queued_ms));
                }
                r.state = "running".into();
            }
            EventKind::JobAssigned if r.wait_ms.is_none() && r.queued_ms > 0 => {
                r.wait_ms = Some(e.ts_ms.saturating_sub(r.queued_ms));
                r.state = "running".into();
            }
            EventKind::JobFinished => {
                r.run_ms = e.ms;
                if let Some(j) = &e.job {
                    r.name = j.clone();
                }
                let ok = e
                    .result
                    .as_deref()
                    .is_none_or(|x| matches!(x, "Succeeded" | "success"))
                    && e.exit_code.unwrap_or(0) == 0;
                r.state = if ok { "passed" } else { "failed" }.into();
            }
            _ => {}
        }
    }
    let _ = now_ms;
    let mut v: Vec<JobRow> = rows.into_values().collect();
    v.sort_by_key(|r| std::cmp::Reverse(r.queued_ms));
    v
}

/// Fills a job's computer, cloud and cost from the capacity ledgers.
pub fn attach_cost(
    rows: &mut [JobRow],
    machine: impl Fn(&str) -> Option<(String, String, String, bool, u64, u64)>,
) {
    for r in rows {
        if let Some((host, ty, cloud, spot, compute, all_in)) = machine(&r.job_key) {
            r.machine = host;
            r.machine_type = ty;
            r.cloud = cloud;
            r.spot = spot;
            if r.state == "passed" || r.state == "failed" {
                r.cost_micros = Some(compute);
                r.all_in_micros = Some(all_in);
            }
        }
    }
}

/// The overview line and the repositories that ran jobs, from the rows.
pub fn usage_of(rows: &[JobRow], events: &[Event], now_ms: u64) -> (Usage, Vec<RepoUse>) {
    let starts_24h = events
        .iter()
        .filter(|e| e.kind == EventKind::JobAssigned && e.ts_ms + DAY_MS >= now_ms)
        .count() as u32;
    let week: Vec<&JobRow> = rows
        .iter()
        .filter(|r| {
            matches!(r.state.as_str(), "passed" | "failed") && r.queued_ms + 7 * DAY_MS >= now_ms
        })
        .collect();
    let passed = week.iter().filter(|r| r.state == "passed").count();
    let mut repos: BTreeMap<String, RepoUse> = BTreeMap::new();
    for r in rows
        .iter()
        .filter(|r| !r.repo.is_empty() && r.queued_ms + 7 * DAY_MS >= now_ms)
    {
        let u = repos.entry(r.repo.clone()).or_insert_with(|| RepoUse {
            name: r.repo.clone(),
            status: "ready".into(),
            ..RepoUse::default()
        });
        u.jobs_7d += 1;
        u.last_job_ms = u.last_job_ms.max(r.queued_ms);
    }
    (
        Usage {
            starts_24h,
            jobs_7d: week.len() as u32,
            pass_rate_7d: (!week.is_empty()).then(|| passed as f64 / week.len() as f64),
            spend_7d_micros: week.iter().filter_map(|r| r.cost_micros).sum(),
        },
        repos.into_values().collect(),
    )
}

/// One line of a job's Cua CI log.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogLine {
    /// Unix ms.
    pub at_ms: u64,
    /// A plain sentence.
    pub text: String,
}

fn secs(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        format!("{:.1} s", ms as f64 / 1000.0)
    } else {
        format!("{} min {} s", ms / 60_000, (ms % 60_000) / 1000)
    }
}

/// A job's own log from the events of its pool: the computer starting, the
/// runner, the job and the teardown, one plain sentence each. Events of the
/// job (`job_key`) and of its computer slot (`slot`, when known).
pub fn log_lines(events: &[Event], job_key: &str, slot: &str) -> Vec<LogLine> {
    let mut out = Vec::new();
    for e in events {
        let own = e
            .extra
            .get("job_key")
            .and_then(|v| v.as_str())
            .is_some_and(|k| k == job_key);
        let on_slot = !slot.is_empty() && e.slot.as_deref() == Some(slot);
        if !own && !on_slot {
            continue;
        }
        // A slot's events of another job are not this job's.
        if on_slot
            && !own
            && e.extra
                .get("job_key")
                .and_then(|v| v.as_str())
                .is_some_and(|k| k != job_key)
        {
            continue;
        }
        let ms = e.ms.map(secs);
        let detail = e.detail.clone().filter(|d| !d.is_empty());
        let text = match e.kind {
            EventKind::JobQueued => "Queued, waiting for a computer".to_string(),
            EventKind::VmClaimed => match e.extra.get("start_mode").and_then(|v| v.as_str()) {
                Some("warm_pool") => "Took a Space that was already running".to_string(),
                Some("fork") => "Started a Space from a saved copy".to_string(),
                _ => "Started a new Space".to_string(),
            },
            EventKind::SlotCreated => "Starting a Space".to_string(),
            EventKind::SpaceReady | EventKind::VmReady => match ms {
                Some(m) => format!("The Space is ready after {m}"),
                None => "The Space is ready".to_string(),
            },
            EventKind::RunnerInstalled => "The runner is installed".to_string(),
            EventKind::RunnerRegistered => "The runner registered with GitHub".to_string(),
            EventKind::JobAssigned => "GitHub gave the job to the runner".to_string(),
            EventKind::JobStarted => "The job started".to_string(),
            EventKind::JobFinished => {
                let ok = e
                    .result
                    .as_deref()
                    .is_none_or(|x| matches!(x, "Succeeded" | "success"))
                    && e.exit_code.unwrap_or(0) == 0;
                let word = if ok { "passed" } else { "failed" };
                match ms {
                    Some(m) => format!("The job {word} after {m}"),
                    None => format!("The job {word}"),
                }
            }
            EventKind::SlotDeleted | EventKind::TeardownDone => {
                "The Space was deleted and nothing of the job remains".to_string()
            }
            EventKind::CreateFailed | EventKind::Error | EventKind::InfraFailure => match detail {
                Some(d) => format!("Problem: {d}"),
                None => "A problem came up".to_string(),
            },
            EventKind::CapacityWait => "Waiting for room on the computer".to_string(),
            EventKind::CheckoutDone => match ms {
                Some(m) => format!("The repository was fetched in {m}"),
                None => "The repository was fetched".to_string(),
            },
            _ => continue,
        };
        out.push(LogLine {
            at_ms: e.ts_ms,
            text,
        });
    }
    out
}

/// The checklist from what is known. `permitted` is how many repositories
/// can run jobs, when the service said (`None` offline).
pub fn setup_steps(
    connected: bool,
    permitted: Option<usize>,
    repos_used: usize,
    passed: bool,
) -> Vec<SetupStep> {
    let step = |id: &str, title: &str, done: Option<bool>, detail: String| SetupStep {
        id: id.into(),
        title: title.into(),
        state: match done {
            Some(true) => "done",
            Some(false) => "todo",
            None => "unknown",
        }
        .into(),
        detail,
    };
    let plural = |n: usize, one: &str, many: &str| {
        if n == 1 {
            one.to_string()
        } else {
            many.to_string()
        }
    };
    vec![
        step(
            "connect_github",
            "Connect GitHub",
            Some(connected),
            if connected {
                "Connected".into()
            } else {
                "Sign in to GitHub and choose where Cua may run jobs".into()
            },
        ),
        // Which repositories may run jobs is the service's to say.
        match permitted {
            Some(n) if connected => step(
                "repositories",
                "Give access to repositories",
                Some(n > 0),
                if n == 0 {
                    "No repository can run jobs yet".into()
                } else {
                    format!("{n} repositor{} can run jobs", plural(n, "y", "ies"))
                },
            ),
            _ => step(
                "repositories",
                "Give access to repositories",
                if connected { None } else { Some(false) },
                if connected {
                    "Reading the repositories that can run jobs".into()
                } else {
                    "Choose them when you connect GitHub".into()
                },
            ),
        },
        step(
            "runner_label",
            "Use the runner label in a workflow",
            Some(repos_used > 0),
            format!(
                "{repos_used} repositor{} used it this week",
                plural(repos_used, "y", "ies")
            ),
        ),
        step(
            "first_job_passed",
            "A job passed",
            Some(passed),
            if passed {
                "A job passed".into()
            } else {
                "No job has passed yet".into()
            },
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(ts: u64, kind: EventKind, key: &str) -> Event {
        Event::new(ts, kind).slot("s1").with("job_key", key)
    }

    #[test]
    fn a_jobs_row_has_wait_run_state_and_start_mode() {
        let now = 10_000_000;
        let mut fin = ev(now - 1_000, EventKind::JobFinished, "1/7").ms(180_000);
        fin.job = Some("test".into());
        fin.result = Some("Succeeded".into());
        let events = vec![
            ev(now - 200_000, EventKind::JobQueued, "1/7")
                .with("queued_at_ms", now - 200_000)
                .with("repo", "acme/app"),
            ev(now - 199_000, EventKind::VmClaimed, "1/7").with("start_mode", "warm_pool"),
            ev(now - 195_000, EventKind::JobStarted, "1/7"),
            fin,
            ev(now - 50_000, EventKind::JobQueued, "1/8").with("queued_at_ms", now - 50_000),
        ];
        let rows = jobs_from_events("aws", &events, now, 0);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].job_key, "1/8", "newest first");
        assert_eq!(rows[0].state, "queued");
        let r = &rows[1];
        assert_eq!((r.wait_ms, r.run_ms), (Some(5_000), Some(180_000)));
        assert_eq!((r.state.as_str(), r.name.as_str()), ("passed", "test"));
        assert_eq!(
            (r.repo.as_str(), r.start_mode.as_str(), r.log_slot.as_str()),
            ("acme/app", "warm_pool", "s1")
        );
    }

    #[test]
    fn a_jobs_log_is_plain_sentences_of_its_events_and_its_machine() {
        let now = 10_000_000;
        let mut fin = ev(now - 1_000, EventKind::JobFinished, "1/7").ms(180_000);
        fin.result = Some("Failed".into());
        let events = vec![
            ev(now - 200_000, EventKind::JobQueued, "1/7"),
            Event::new(now - 199_000, EventKind::SpaceReady)
                .slot("s1")
                .ms(4_200),
            ev(now - 195_000, EventKind::JobStarted, "1/7"),
            // Another job of the same computer is not this one's.
            ev(now - 90_000, EventKind::JobStarted, "1/8"),
            // Another computer's events are not this one's.
            Event::new(now - 80_000, EventKind::SpaceReady).slot("s2"),
            fin,
        ];
        let lines = log_lines(&events, "1/7", "s1");
        let text: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(
            text,
            [
                "Queued, waiting for a computer",
                "The Space is ready after 4.2 s",
                "The job started",
                "The job failed after 3 min 0 s",
            ]
        );
    }

    #[test]
    fn a_failed_job_cost_and_machine_come_from_the_ledger() {
        let now = 10_000_000;
        let mut fin = ev(now, EventKind::JobFinished, "2/1").ms(60_000);
        fin.result = Some("Failed".into());
        let mut rows = jobs_from_events(
            "aws",
            &[ev(now - 70_000, EventKind::JobQueued, "2/1"), fin],
            now,
            0,
        );
        attach_cost(&mut rows, |k| {
            (k == "2/1").then(|| {
                (
                    "cua-ci-1".into(),
                    "c7i.xlarge".into(),
                    "aws".into(),
                    true,
                    1200,
                    1500,
                )
            })
        });
        let r = &rows[0];
        assert_eq!(r.state, "failed");
        assert_eq!((r.cost_micros, r.all_in_micros), (Some(1200), Some(1500)));
        assert_eq!(
            (r.machine.as_str(), r.cloud.as_str(), r.spot),
            ("cua-ci-1", "aws", true)
        );
    }

    #[test]
    fn usage_counts_the_week_and_the_repositories() {
        let now = 20 * DAY_MS;
        let mk = |k: &str, repo: &str, state: &str, q: u64, cost: u64| JobRow {
            job_key: k.into(),
            repo: repo.into(),
            state: state.into(),
            queued_ms: q,
            cost_micros: Some(cost),
            ..JobRow::default()
        };
        let rows = vec![
            mk("1/1", "acme/app", "passed", now - 1000, 100),
            mk("1/2", "acme/app", "failed", now - 2000, 50),
            mk("1/3", "acme/api", "passed", now - 3000, 25),
            mk("0/1", "acme/old", "passed", now - 9 * DAY_MS, 999),
        ];
        let assigned = vec![
            Event::new(now - 1000, EventKind::JobAssigned),
            Event::new(now - 2 * DAY_MS, EventKind::JobAssigned),
        ];
        let (u, repos) = usage_of(&rows, &assigned, now);
        assert_eq!((u.starts_24h, u.jobs_7d, u.spend_7d_micros), (1, 3, 175));
        assert!((u.pass_rate_7d.unwrap() - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(repos.len(), 2);
        assert_eq!(
            repos.iter().find(|r| r.name == "acme/app").unwrap().jobs_7d,
            2
        );
        let (none, _) = usage_of(&[], &[], now);
        assert_eq!(none.pass_rate_7d, None);
    }

    #[test]
    fn the_checklist_says_what_is_done_and_what_only_the_service_knows() {
        let s = setup_steps(false, None, 0, false);
        assert_eq!(s[0].state, "todo");
        assert_eq!(s[1].state, "todo");
        let s = setup_steps(true, None, 2, true);
        assert_eq!(
            s.iter().map(|x| x.state.as_str()).collect::<Vec<_>>(),
            ["done", "unknown", "done", "done"]
        );
        assert!(s[2].detail.starts_with("2 repositories"), "{}", s[2].detail);
        assert!(
            setup_steps(true, None, 1, false)[2]
                .detail
                .starts_with("1 repository used")
        );
        // The service said which repositories can run jobs.
        let s = setup_steps(true, Some(3), 0, false);
        assert_eq!(
            (s[1].state.as_str(), s[1].detail.as_str()),
            ("done", "3 repositories can run jobs")
        );
        let s = setup_steps(true, Some(1), 0, false);
        assert_eq!(s[1].detail, "1 repository can run jobs");
        let s = setup_steps(true, Some(0), 0, false);
        assert_eq!(
            (s[1].state.as_str(), s[1].detail.as_str()),
            ("todo", "No repository can run jobs yet")
        );
    }
}
