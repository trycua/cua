//! `cua ci overview`: everything the CI Runners page shows, as JSON, read
//! from the files this computer keeps (event logs, capacity ledgers, target
//! reports). It needs no network and no cloud credential.

use std::io::Write;

use cua_ci::CiPaths;
use cua_ci::config::{PoolConfig, Scope};
use cua_ci::events::EventLog;
use cua_ci::hosted::HostedState;
use cua_ci::overview::{
    self, BudgetView, GithubView, Overview, RepoChoice, RepositoriesView, TargetPilot,
};
use cua_ci::targets::TargetStore;
use cua_sdk::CuaError;

use crate::ci_hosted::HOSTED_POOL;
use crate::util::line;

const WEEK_MS: u64 = 7 * 86_400_000;

/// How many jobs the table carries.
const MAX_JOBS: usize = 200;

/// Builds the overview at `now_ms`.
pub fn build(paths: &CiPaths, now_ms: u64) -> Overview {
    let since = now_ms.saturating_sub(WEEK_MS);
    let mut pools = paths.pools();
    if !pools.iter().any(|p| p == HOSTED_POOL) {
        pools.push(HOSTED_POOL.to_string());
    }
    let mut rows = Vec::new();
    let mut all_events = Vec::new();
    for p in &pools {
        let events = EventLog::new(paths.events(p)).read();
        rows.extend(overview::jobs_from_events(p, &events, now_ms, since));
        all_events.extend(events);
    }
    let store = TargetStore::new(paths);
    let targets = store.list();
    rows.sort_by_key(|r| std::cmp::Reverse(r.queued_ms));
    rows.truncate(MAX_JOBS);
    let (usage, used) = overview::usage_of(&rows, &all_events, now_ms);
    let github = github_view(paths);
    let connected = github.connected;
    let passed = rows.iter().any(|r| r.state == "passed");
    let mut budget = BudgetView {
        state: "ok".into(),
        ..BudgetView::default()
    };
    let mut pilots = Vec::new();
    for t in &targets {
        let report = store.read_report(&t.name);
        if let Some(r) = &report {
            budget.month_committed_micros += r.status.month_committed_micros;
            budget.monthly_micros += r.status.monthly_micros;
            let rank = |s: &str| match s {
                "exhausted" => 2,
                "warn" => 1,
                _ => 0,
            };
            if rank(&r.status.budget_state) > rank(&budget.state) {
                budget.state = r.status.budget_state.clone();
            }
        }
        budget.hourly_cap_micros += (t.max_per_hour_usd * 1_000_000.0) as u64;
        pilots.push(TargetPilot {
            target: t.name.clone(),
            goal: t.goal_or_default().to_string(),
            warm: t.fixed_warm(),
            autopilot: report.and_then(|r| r.status.autopilot),
        });
    }
    let mut setup = overview::setup_steps(connected, None, used.len(), passed);
    if github.mode == "own" {
        // With your own credential the repositories are the runner group's,
        // chosen at GitHub: nothing for the page to read or ask.
        setup[1].state = "done".into();
        setup[1].detail = "Chosen at GitHub, in the runner group".into();
    }
    Overview {
        generated_ms: now_ms,
        setup,
        github,
        repositories: RepositoriesView { used, all: None },
        usage,
        jobs: rows,
        targets: pilots,
        budget,
    }
}

/// The GitHub connection this computer has: the hosted Cua CI app, a pool of
/// your own app or token, or none.
pub fn github_view(paths: &CiPaths) -> GithubView {
    if let Ok(Some(h)) = HostedState::load(paths) {
        let login = h.github_login;
        let choose_url = if login.is_empty() {
            "https://github.com/settings/installations".to_string()
        } else {
            format!("https://github.com/organizations/{login}/settings/installations")
        };
        return GithubView {
            connected: true,
            mode: "hosted".into(),
            login,
            choose_url,
            note: String::new(),
        };
    }
    for name in paths.pools() {
        if name == HOSTED_POOL {
            continue;
        }
        let Ok(cfg) = PoolConfig::load(&paths.config(&name)) else {
            continue;
        };
        let (login, choose_url) = match &cfg.scope {
            Scope::Org(o) => (
                o.clone(),
                format!("https://github.com/organizations/{o}/settings/actions/runner-groups"),
            ),
            Scope::Repo { owner, name } => (
                format!("{owner}/{name}"),
                format!("https://github.com/{owner}/{name}/settings/actions/runners"),
            ),
        };
        return GithubView {
            connected: true,
            mode: "own".into(),
            login,
            choose_url,
            note: String::new(),
        };
    }
    GithubView {
        mode: "none".into(),
        ..GithubView::default()
    }
}

/// [`build`], then the one thing only the service knows: every repository
/// the hosted app can see and which of them may run jobs. A service that
/// cannot be reached leaves the offline reading with a quiet note.
pub async fn build_live(paths: &CiPaths, now_ms: u64) -> Overview {
    let mut o = build(paths, now_ms);
    if o.github.mode != "hosted" {
        return o;
    }
    let Ok(Some(state)) = HostedState::load(paths) else {
        return o;
    };
    let repos = match crate::ci_hosted::client(&state.account) {
        Ok(api) => api.repos().await.map(|(r, _)| r).map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    match repos {
        Ok(r) => {
            let all: Vec<RepoChoice> = r
                .repos
                .iter()
                .filter(|x| x.visible)
                .map(|x| RepoChoice {
                    name: x.full_name.clone(),
                    allowed: x.allowed,
                    private: x.private,
                })
                .collect();
            let permitted = all.iter().filter(|x| x.allowed).count();
            let passed = o.jobs.iter().any(|j| j.state == "passed");
            o.setup =
                overview::setup_steps(true, Some(permitted), o.repositories.used.len(), passed);
            o.repositories.all = Some(all);
        }
        Err(why) => o.github.note = format!("Could not read the repositories from Cua: {why}"),
    }
    o
}

/// `cua ci overview`: from this computer's records; with `live`, the
/// repositories from the service too.
pub async fn run(paths: &CiPaths, live: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let now = cua_ci::unix_ms();
    let o = if live {
        build_live(paths, now).await
    } else {
        build(paths, now)
    };
    line(
        out,
        serde_json::to_string_pretty(&o).map_err(|e| CuaError::Internal(e.to_string()))?,
    );
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_ci::events::{Event, EventKind};

    #[test]
    fn an_empty_home_gives_a_checklist_to_start_and_no_jobs() {
        let d = tempfile::tempdir().unwrap();
        let paths = CiPaths::new(d.path().join("ci"));
        let o = build(&paths, 1_000_000_000);
        assert!(o.jobs.is_empty() && o.targets.is_empty());
        assert_eq!(o.setup[0].state, "todo");
        assert_eq!(o.usage.jobs_7d, 0);
        assert_eq!(o.budget.state, "ok");
        let j = serde_json::to_value(&o).unwrap();
        for k in [
            "setup",
            "repositories",
            "usage",
            "jobs",
            "targets",
            "budget",
        ] {
            assert!(j.get(k).is_some(), "{k}");
        }
    }

    #[test]
    fn jobs_in_the_event_log_show_with_wait_run_and_state() {
        let d = tempfile::tempdir().unwrap();
        let paths = CiPaths::new(d.path().join("ci"));
        let now = 1_000_000_000u64;
        let log = EventLog::new(paths.events(HOSTED_POOL));
        log.record(
            &Event::new(now - 120_000, EventKind::JobQueued)
                .with("job_key", "9/1")
                .with("queued_at_ms", now - 120_000)
                .with("repo", "acme/app"),
        );
        log.record(&Event::new(now - 110_000, EventKind::JobStarted).with("job_key", "9/1"));
        let mut fin = Event::new(now - 10_000, EventKind::JobFinished)
            .with("job_key", "9/1")
            .ms(100_000);
        fin.result = Some("Succeeded".into());
        fin.job = Some("build".into());
        log.record(&fin);
        let o = build(&paths, now);
        assert_eq!(o.jobs.len(), 1);
        let j = &o.jobs[0];
        assert_eq!(
            (j.state.as_str(), j.wait_ms, j.run_ms),
            ("passed", Some(10_000), Some(100_000))
        );
        assert_eq!(o.usage.jobs_7d, 1);
        assert_eq!(o.repositories.used[0].name, "acme/app");
        assert_eq!(
            o.setup
                .iter()
                .find(|s| s.id == "first_job_passed")
                .unwrap()
                .state,
            "done"
        );
    }
}
