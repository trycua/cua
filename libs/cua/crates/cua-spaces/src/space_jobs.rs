//! Background jobs in a Space: a long command (a build, a test suite, a wait
//! on CI) that outlives the call that started it, the connection and a restart
//! of the Space's daemon.
//!
//! Every operation is one shell run through the Space's spacesd, so a job
//! behaves the same on every runtime (container, gVisor, microVM, VM, cloud)
//! and in images that shipped before jobs existed. Nothing here is specific to
//! a runtime.
//!
//! A job is a directory in the Space, `$HOME/.cua/jobs/<id>`:
//!
//! | file | |
//! | --- | --- |
//! | `cmd.sh`, `cmdline`, `name` | what runs, as written, and its label |
//! | `run.sh` | the runner: starts `cmd.sh` as a background child in its own process group, enforces `timeout`, records the end |
//! | `runner.pid`, `child.pid`, `started` | liveness and timing (unix seconds) |
//! | `log` | stdout and stderr together |
//! | `exit` | `<code> <ended>`, written atomically when the command ends |
//! | `cancelled`, `timedout` | markers that say why it ended |
//!
//! The runner is started detached (`setsid` where it exists), so it is not a
//! child of the shell that started it or of cua-spacesd. The state is derived
//! from the files: `running`, `exited`, `cancelled`, `timed_out`, or `lost`
//! (the runner is gone and left no `exit`, for example after a reboot).
//!
//! The runner and the helper scripts are POSIX `sh`: Linux and macOS guests.
//! A Windows guest is refused with `unsupported`.

use std::collections::BTreeMap;
use std::time::Duration;

use cua_spaces_contract::inputs as i;
use serde::Serialize;
use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::space::Space;

/// Most bytes one `logs` call returns.
pub const MAX_LOG_BYTES: u64 = 256 * 1024;
/// Bytes of log a `logs` call returns by default (and `status` shows).
pub const DEFAULT_LOG_BYTES: u64 = 16_000;
/// Longest `follow_ms` (the thread's bound).
pub const MAX_FOLLOW_MS: u64 = 25_000;
/// Jobs `list` shows.
pub const MAX_LISTED: usize = 50;
/// Longest label kept.
const MAX_NAME: usize = 80;
/// Jobs that ended this many days ago are removed when a job starts.
pub const KEEP_DAYS: u32 = 7;
/// Separates the header from the log bytes in a script's output.
const LOG_MARK: &str = "\n---CUA-LOG---\n";
/// Separates jobs in a listing.
const ROW_MARK: &str = "---CUA-JOB---";

/// The states of a job.
pub const STATES: &[&str] = &["running", "exited", "cancelled", "timed_out", "lost"];

/// What a job is doing, from its files.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct JobStatus {
    /// The job id.
    pub job: String,
    /// The label (empty when none).
    pub name: String,
    /// One of [`STATES`].
    pub state: String,
    /// The exit code, once it ended.
    pub exit_code: Option<i32>,
    /// The signal that ended it (`15` for TERM), when it was one.
    pub signal: Option<i32>,
    /// Unix seconds it started.
    pub started_at: Option<u64>,
    /// Unix seconds it ended.
    pub ended_at: Option<u64>,
    /// Seconds from start to end (or to now while it runs).
    pub elapsed_s: u64,
    /// Bytes in the log.
    pub size: u64,
    /// The command line, cut to one line.
    pub command: String,
}

impl JobStatus {
    /// Whether the job has ended (any state but `running`).
    pub fn finished(&self) -> bool {
        self.state != "running"
    }

    /// `1h02m03s`, `4m10s` or `9s`.
    pub fn elapsed_text(&self) -> String {
        elapsed_text(self.elapsed_s)
    }
}

/// `1h02m03s`, `4m10s` or `9s`.
pub fn elapsed_text(s: u64) -> String {
    match (s / 3600, s / 60 % 60, s % 60) {
        (0, 0, s) => format!("{s}s"),
        (0, m, s) => format!("{m}m{s:02}s"),
        (h, m, s) => format!("{h}h{m:02}m{s:02}s"),
    }
}

/// A chunk of a job's log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobLogs {
    /// The job at the time of the read.
    pub status: JobStatus,
    /// Where `text` starts.
    pub offset: u64,
    /// Where to read on from.
    pub next_offset: u64,
    /// The bytes read (lossy UTF-8).
    pub text: String,
    /// More log exists after `next_offset`.
    pub more: bool,
}

// --------------------------------------------------------------- scripts

/// Shared shell functions: where jobs live and how a job's state reads from
/// its files.
const PRELUDE: &str = r#"
J="${HOME:-/tmp}/.cua/jobs"
now() { date +%s; }
alive() {
  kill -0 "$1" 2>/dev/null || return 1
  if [ -r "/proc/$1/stat" ]; then
    z=$(sed 's/.*) //' "/proc/$1/stat" 2>/dev/null | cut -c1)
    case "$z" in Z|X) return 1 ;; esac
  fi
  return 0
}
show() {
  D="$J/$1"
  if [ ! -d "$D" ]; then printf 'job=%s\nstate=notfound\n' "$1"; return; fi
  state=lost; code=; ended=
  # The runner writes `exit` and then ends, so look at `exit` again after
  # finding the runner gone: it may have finished between the two looks.
  p=$(cat "$D/runner.pid" 2>/dev/null)
  if [ ! -f "$D/exit" ] && [ -n "$p" ] && alive "$p"; then
    state=running
  elif [ -f "$D/exit" ]; then
    read code ended < "$D/exit"
    if [ -f "$D/cancelled" ]; then state=cancelled
    elif [ -f "$D/timedout" ]; then state=timed_out
    else state=exited; fi
  fi
  started=$(cat "$D/started" 2>/dev/null)
  size=$(wc -c < "$D/log" 2>/dev/null | tr -d ' ')
  printf 'job=%s\nstate=%s\ncode=%s\nstarted=%s\nended=%s\nsize=%s\nnow=%s\n' "$1" "$state" "$code" "$started" "$ended" "${size:-0}" "$(now)"
  printf 'name=%s\n' "$(head -n 1 "$D/name" 2>/dev/null)"
  printf 'command=%s\n' "$(head -n 1 "$D/cmdline" 2>/dev/null | cut -c1-160)"
}
"#;

/// The runner. `run.sh DIR TIMEOUT_SECONDS`.
const RUNNER: &str = r#"#!/bin/sh
D=$1
T=$2
cd "$D" || exit 1
echo $$ > runner.pid
trap '' HUP
set -m 2>/dev/null
sh ./cmd.sh > log 2>&1 < /dev/null &
child=$!
echo "$child" > child.pid
date +%s > started.tmp && mv started.tmp started
wd=
if [ -n "$T" ] && [ "$T" -gt 0 ] 2>/dev/null; then
  (
    sleep "$T"
    : > timedout
    kill -s TERM -- "-$child" 2>/dev/null || kill -s TERM "$child" 2>/dev/null
    sleep 5
    kill -s KILL -- "-$child" 2>/dev/null || kill -s KILL "$child" 2>/dev/null
  ) &
  wd=$!
fi
wait "$child"
code=$?
[ -n "$wd" ] && { kill -s KILL -- "-$wd" 2>/dev/null || kill -s KILL "$wd" 2>/dev/null; }
printf '%s %s\n' "$code" "$(date +%s)" > exit.tmp && mv exit.tmp exit
"#;

/// Single-quotes `s` for a POSIX shell.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn one_line(s: &str, max: usize) -> String {
    let flat: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    flat.trim().chars().take(max).collect()
}

fn valid_env_name(k: &str) -> bool {
    let mut c = k.chars();
    c.next()
        .is_some_and(|f| f.is_ascii_alphabetic() || f == '_')
        && c.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Splits `owner/repo#123` into `(Some("owner/repo"), "123")`.
fn split_pr(pr: &str) -> (Option<String>, String) {
    match pr.split_once('#') {
        Some((repo, n)) if repo.contains('/') && !n.is_empty() => {
            (Some(repo.to_string()), n.to_string())
        }
        _ => (None, pr.to_string()),
    }
}

/// The shell for a condition (`until`): it ends with exit 0 when the
/// condition holds (or the `gh` exit status for `pr_checks`).
pub fn until_script(u: &i::JobUntil) -> Result<String> {
    let given = [
        u.file.is_some(),
        u.port.is_some(),
        u.command.is_some(),
        u.pr_checks.is_some(),
    ]
    .iter()
    .filter(|b| **b)
    .count();
    if given != 1 {
        return Err(Error::invalid(
            "until: give exactly one of file, port, command or pr_checks",
        ));
    }
    let every = |default: u64| u.interval.unwrap_or(default).clamp(1, 300);
    if let Some(f) = &u.file {
        return Ok(format!(
            "echo {wait}\nuntil [ -e {q} ]; do sleep {n}; done\necho {found}\n",
            wait = sh_quote(&format!("waiting for file {}", one_line(f, 200))),
            found = sh_quote(&format!("found {}", one_line(f, 200))),
            q = sh_quote(f),
            n = every(5)
        ));
    }
    if let Some(port) = u.port {
        let host = u.host.as_deref().unwrap_or("127.0.0.1");
        if host.is_empty()
            || !host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_'))
        {
            return Err(Error::invalid(
                "until.host: letters, digits, '.', '-', ':' and '_' only",
            ));
        }
        return Ok(format!(
            r#"port_open() {{
  if command -v nc >/dev/null 2>&1; then nc -z -w 2 "$1" "$2" >/dev/null 2>&1
  elif command -v bash >/dev/null 2>&1; then bash -c "exec 3<>/dev/tcp/$1/$2" >/dev/null 2>&1
  elif command -v python3 >/dev/null 2>&1; then python3 -c "import socket,sys; socket.create_connection((sys.argv[1], int(sys.argv[2])), 2).close()" "$1" "$2" >/dev/null 2>&1
  else return 1; fi
}}
echo "waiting for {host}:{port}"
until port_open {host} {port}; do sleep {n}; done
echo "{host}:{port} is open"
"#,
            n = every(5)
        ));
    }
    if let Some(c) = &u.command {
        return Ok(format!(
            "echo {msg}\nuntil ( {c}\n) >/dev/null 2>&1; do sleep {n}; done\necho \"condition met\"\n",
            msg = sh_quote(&format!("waiting for: {}", one_line(c, 200))),
            n = every(5)
        ));
    }
    let pr = u.pr_checks.as_deref().unwrap_or_default().trim();
    if pr.is_empty() {
        return Err(Error::invalid("until.pr_checks is empty"));
    }
    let (repo, number) = split_pr(pr);
    let repo_flag = repo
        .map(|r| format!(" --repo {}", sh_quote(&r)))
        .unwrap_or_default();
    Ok(format!(
        r#"command -v gh >/dev/null 2>&1 || {{ echo "gh is not installed in this Space" >&2; exit 127; }}
echo {msg}
gh pr checks {n}{repo_flag} --required --watch --interval {iv}
rc=$?
gh pr checks {n}{repo_flag} --required 2>&1
exit $rc
"#,
        msg = sh_quote(&format!("waiting for the required checks of {pr}")),
        n = sh_quote(&number),
        iv = every(30)
    ))
}

/// A fresh job id: `j` and twelve hex digits.
pub fn new_job_id() -> String {
    let n: u64 = rand::random::<u64>() & 0xffff_ffff_ffff;
    format!("j{n:012x}")
}

fn valid_job_id(id: &str) -> Result<&str> {
    if !id.is_empty()
        && id.len() <= 40
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Ok(id)
    } else {
        Err(Error::invalid(format!("job: `{id}` is not a job id")))
    }
}

/// The shell that creates job `id` and starts its runner, detached.
pub fn start_script(id: &str, a: &i::SpaceJob) -> Result<String> {
    valid_job_id(id)?;
    let body = match (&a.command, &a.until) {
        (Some(_), Some(_)) => {
            return Err(Error::invalid("start: give `command` or `until`, not both"));
        }
        (Some(c), None) if !c.trim().is_empty() => c.clone(),
        (None, Some(u)) => until_script(u)?,
        _ => return Err(Error::invalid("start: `command` (or `until`) is required")),
    };
    let mut cmd = String::new();
    if let Some(dir) = a.cwd.as_deref().filter(|d| !d.is_empty()) {
        cmd += &format!("cd {} || exit 126\n", sh_quote(dir));
    }
    for (k, v) in a.env.iter().flatten() {
        if !valid_env_name(k) {
            return Err(Error::invalid(format!("env: `{k}` is not a variable name")));
        }
        cmd += &format!("export {k}={}\n", sh_quote(v));
    }
    cmd += &body;
    let label = one_line(a.name.as_deref().unwrap_or_default(), MAX_NAME);
    let cmdline = a.command.clone().unwrap_or_else(|| {
        format!(
            "until {}",
            serde_json::to_string(&a.until).unwrap_or_default()
        )
    });
    let (d_cmd, d_run) = (format!("CUA_JOB_{id}_CMD"), format!("CUA_JOB_{id}_RUN"));
    let d_line = format!("CUA_JOB_{id}_LINE");
    for (d, text) in [(&d_cmd, &cmd), (&d_line, &cmdline)] {
        if text.lines().any(|l| l == d) {
            return Err(Error::invalid("the command contains a reserved word"));
        }
    }
    let timeout = a.timeout.filter(|t| *t > 0).unwrap_or(0);
    Ok(format!(
        r#"{PRELUDE}
D="$J/{id}"
mkdir -p "$D" || exit 1
for e in $(find "$J" -maxdepth 2 -name exit -mtime +{KEEP_DAYS} 2>/dev/null); do rm -rf "$(dirname "$e")"; done
cat > "$D/cmd.sh" <<'{d_cmd}'
{cmd}
{d_cmd}
cat > "$D/run.sh" <<'{d_run}'
{RUNNER}
{d_run}
cat > "$D/cmdline" <<'{d_line}'
{cmdline}
{d_line}
printf '%s\n' {name} > "$D/name"
if command -v setsid >/dev/null 2>&1; then
  setsid sh "$D/run.sh" "$D" {timeout} </dev/null >/dev/null 2>&1 &
else
  nohup sh "$D/run.sh" "$D" {timeout} </dev/null >/dev/null 2>&1 &
fi
i=0
while [ ! -f "$D/started" ] && [ "$i" -lt 100 ]; do sleep 0.1 2>/dev/null || sleep 1; i=$((i+1)); done
[ -f "$D/started" ] || {{ echo "the job did not start" >&2; exit 1; }}
show {id}
"#,
        name = sh_quote(&label),
    ))
}

/// The shell that reads a job's state.
pub fn status_script(id: &str) -> Result<String> {
    let id = valid_job_id(id)?;
    Ok(format!("{PRELUDE}\nshow {id}\n"))
}

/// The shell that reads a job's state and then its log: from `offset`, or the
/// last `limit` bytes without one.
pub fn logs_script(id: &str, offset: Option<u64>, limit: u64) -> Result<String> {
    let id = valid_job_id(id)?;
    let read = match offset {
        Some(o) => format!(
            "off={o}\necho \"offset=$off\"\necho '---CUA-LOG---'\ntail -c +$((off+1)) \"$J/{id}/log\" 2>/dev/null | head -c {limit}\n"
        ),
        None => format!(
            "sz=$(wc -c < \"$J/{id}/log\" 2>/dev/null | tr -d ' ')\nsz=${{sz:-0}}\nif [ \"$sz\" -gt {limit} ]; then off=$((sz-{limit})); else off=0; fi\necho \"offset=$off\"\necho '---CUA-LOG---'\ntail -c +$((off+1)) \"$J/{id}/log\" 2>/dev/null | head -c {limit}\n"
        ),
    };
    Ok(format!("{PRELUDE}\nshow {id}\n{read}"))
}

/// The shell that stops a job (TERM, then KILL after a grace period) and
/// reads its state.
pub fn cancel_script(id: &str) -> Result<String> {
    let id = valid_job_id(id)?;
    Ok(format!(
        r#"{PRELUDE}
D="$J/{id}"
if [ -d "$D" ] && [ ! -f "$D/exit" ]; then
  : > "$D/cancelled"
  c=$(cat "$D/child.pid" 2>/dev/null)
  if [ -n "$c" ]; then
    kill -s TERM -- "-$c" 2>/dev/null || kill -s TERM "$c" 2>/dev/null
    i=0
    while [ ! -f "$D/exit" ] && [ "$i" -lt 50 ]; do sleep 0.1 2>/dev/null || sleep 1; i=$((i+1)); done
    if [ ! -f "$D/exit" ]; then
      kill -s KILL -- "-$c" 2>/dev/null || kill -s KILL "$c" 2>/dev/null
      i=0
      while [ ! -f "$D/exit" ] && [ "$i" -lt 50 ]; do sleep 0.1 2>/dev/null || sleep 1; i=$((i+1)); done
    fi
  fi
fi
show {id}
"#
    ))
}

/// The shell that lists the jobs, newest first.
pub fn list_script() -> String {
    format!(
        r#"{PRELUDE}
for d in "$J"/*/; do
  [ -d "$d" ] || continue
  show "$(basename "$d")"
  echo '{ROW_MARK}'
done
"#
    )
}

// --------------------------------------------------------------- parsing

fn fields(header: &str) -> BTreeMap<String, String> {
    header
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn status_of(f: &BTreeMap<String, String>) -> Result<JobStatus> {
    let get = |k: &str| f.get(k).map(String::as_str).unwrap_or_default();
    let num = |k: &str| get(k).trim().parse::<u64>().ok();
    let state = get("state");
    if state == "notfound" {
        return Err(Error::NotFound(format!("job {}", get("job"))));
    }
    if !STATES.contains(&state) {
        return Err(Error::Agent(format!(
            "job {}: unreadable state `{state}`",
            get("job")
        )));
    }
    let code = get("code").trim().parse::<i32>().ok();
    let started = num("started");
    let ended = num("ended");
    let now = num("now").unwrap_or(0);
    let elapsed_s = match (started, ended) {
        (Some(s), Some(e)) => e.saturating_sub(s),
        (Some(s), None) => now.saturating_sub(s),
        _ => 0,
    };
    Ok(JobStatus {
        job: get("job").to_string(),
        name: get("name").to_string(),
        state: state.to_string(),
        exit_code: code.filter(|c| *c <= 128),
        signal: code.filter(|c| *c > 128).map(|c| c - 128),
        started_at: started,
        ended_at: ended,
        elapsed_s,
        size: num("size").unwrap_or(0),
        command: get("command").to_string(),
    })
}

/// Reads the header a status script prints.
pub fn parse_status(out: &str) -> Result<JobStatus> {
    status_of(&fields(out))
}

/// Reads the output of [`list_script`].
pub fn parse_list(out: &str) -> Vec<JobStatus> {
    let mut jobs: Vec<JobStatus> = out
        .split(ROW_MARK)
        .filter(|b| !b.trim().is_empty())
        .filter_map(|b| status_of(&fields(b)).ok())
        .collect();
    jobs.sort_by(|a, b| b.started_at.cmp(&a.started_at).then(b.job.cmp(&a.job)));
    jobs.truncate(MAX_LISTED);
    jobs
}

/// Reads the output of [`logs_script`]: the header, and the log bytes after
/// the mark.
pub fn parse_logs(out: &[u8], limit: u64) -> Result<JobLogs> {
    let mark = LOG_MARK.as_bytes();
    let at = out
        .windows(mark.len())
        .position(|w| w == mark)
        .ok_or_else(|| Error::Agent("job logs: no log marker in the output".into()))?;
    let header = String::from_utf8_lossy(&out[..at]);
    let mut body = &out[at + mark.len()..];
    let f = fields(&header);
    let status = status_of(&f)?;
    let offset = f
        .get("offset")
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(0);
    if body.len() as u64 > limit {
        body = &body[..limit as usize];
    }
    let next_offset = offset + body.len() as u64;
    Ok(JobLogs {
        more: status.size > next_offset,
        status,
        offset,
        next_offset,
        text: String::from_utf8_lossy(body).into_owned(),
    })
}

// -------------------------------------------------------------- the Space

impl Space {
    async fn job_run(&self, script: &str, timeout: Duration) -> Result<Vec<u8>> {
        if self.is_windows() {
            return Err(Error::Unsupported(
                "space_job runs in Linux and macOS guests; this Space is Windows".into(),
            ));
        }
        let out = self
            .spacesd()?
            .run(self.shell_command(script).timeout(timeout))
            .await?;
        if out.status.code != Some(0) {
            let err = out.stderr_str();
            return Err(Error::Agent(format!(
                "space_job: {}",
                if err.trim().is_empty() {
                    out.stdout_str()
                } else {
                    err
                }
                .trim()
            )));
        }
        Ok(out.stdout)
    }

    /// Starts a job. The command keeps running when the caller goes away.
    pub async fn job_start(&self, a: &i::SpaceJob) -> Result<JobStatus> {
        let id = new_job_id();
        let script = start_script(&id, a)?;
        let out = self.job_run(&script, Duration::from_secs(60)).await?;
        parse_status(&String::from_utf8_lossy(&out))
    }

    /// A job's state.
    pub async fn job_status(&self, id: &str) -> Result<JobStatus> {
        let out = self
            .job_run(&status_script(id)?, Duration::from_secs(30))
            .await?;
        parse_status(&String::from_utf8_lossy(&out))
    }

    /// Reads a job's log from `offset` (the last `limit` bytes without one).
    /// With `follow` and nothing new after `offset` while the job runs, waits
    /// for output or the end.
    pub async fn job_logs(
        &self,
        id: &str,
        offset: Option<u64>,
        limit: u64,
        follow: Duration,
    ) -> Result<JobLogs> {
        let limit = limit.clamp(1, MAX_LOG_BYTES);
        let deadline =
            tokio::time::Instant::now() + follow.min(Duration::from_millis(MAX_FOLLOW_MS));
        loop {
            let out = self
                .job_run(&logs_script(id, offset, limit)?, Duration::from_secs(30))
                .await?;
            let logs = parse_logs(&out, limit)?;
            let quiet = logs.text.is_empty() && !logs.status.finished();
            if !quiet || offset.is_none() || tokio::time::Instant::now() >= deadline {
                return Ok(logs);
            }
            tokio::time::sleep(Duration::from_millis(1000)).await;
        }
    }

    /// Stops a job and returns its state.
    pub async fn job_cancel(&self, id: &str) -> Result<JobStatus> {
        let out = self
            .job_run(&cancel_script(id)?, Duration::from_secs(30))
            .await?;
        parse_status(&String::from_utf8_lossy(&out))
    }

    /// The Space's jobs, newest first.
    pub async fn job_list(&self) -> Result<Vec<JobStatus>> {
        let out = self
            .job_run(&list_script(), Duration::from_secs(30))
            .await?;
        Ok(parse_list(&String::from_utf8_lossy(&out)))
    }

    /// The `space_job` tool: one call, as the contract describes it.
    pub async fn job(&self, a: &i::SpaceJob) -> Result<Value> {
        let job = || {
            a.job
                .as_deref()
                .filter(|j| !j.is_empty())
                .ok_or_else(|| Error::invalid(format!("{}: `job` is required", a.action)))
        };
        match a.action.as_str() {
            "start" => {
                let st = self.job_start(a).await?;
                let mut v = status_json(&st);
                v["log_command"] = json!(format!(
                    "space_job {{action: \"logs\", job: \"{}\"}}",
                    st.job
                ));
                v["hint"] = json!(if a.notify.unwrap_or(false) {
                    "Running. You will be woken in the Space thread when it ends: finish your turn instead of polling. Read progress any time with action logs (offset = next_offset)."
                } else {
                    "Running. Read progress with action logs (offset = next_offset, follow_ms to wait), or restart with notify: true to be woken when it ends."
                });
                Ok(v)
            }
            "status" => {
                let logs = self.job_logs(job()?, None, 4096, Duration::ZERO).await?;
                Ok(logs_json(&logs))
            }
            "logs" => {
                let logs = self
                    .job_logs(
                        job()?,
                        a.offset,
                        a.limit.unwrap_or(DEFAULT_LOG_BYTES),
                        Duration::from_millis(a.follow_ms.unwrap_or(0)),
                    )
                    .await?;
                Ok(logs_json(&logs))
            }
            "cancel" => Ok(status_json(&self.job_cancel(job()?).await?)),
            "list" => {
                let jobs = self.job_list().await?;
                Ok(json!({"jobs": jobs.iter().map(status_json).collect::<Vec<_>>()}))
            }
            other => Err(Error::invalid(format!(
                "action: `{other}` is not one of start, status, logs, cancel, list"
            ))),
        }
    }
}

/// A status as the tool returns it.
pub fn status_json(s: &JobStatus) -> Value {
    json!({
        "job": s.job, "name": s.name, "state": s.state, "exit_code": s.exit_code,
        "signal": s.signal, "started_at": s.started_at, "ended_at": s.ended_at,
        "elapsed_s": s.elapsed_s, "elapsed": s.elapsed_text(), "size": s.size,
        "command": s.command,
    })
}

fn logs_json(l: &JobLogs) -> Value {
    let mut v = status_json(&l.status);
    v["offset"] = json!(l.offset);
    v["next_offset"] = json!(l.next_offset);
    v["more"] = json!(l.more);
    v["text"] = json!(l.text);
    v
}

/// The extension operation `space_job` calls when a job starts with `notify`
/// (`{space, job, name, wake?}`): the daemon follows the job and, when it
/// ends, posts to the Space's thread (waking the agent the call came from, or
/// `wake`) and tells the person.
pub const WATCH_OP: &str = "space_job.watch";

/// Registers `notify` for the job `started` (the result of `start`). The
/// answer goes in the tool result: `{registered: true, ...}`, or
/// `{registered: false, reason}` where nothing can follow the job (the job
/// still runs).
pub async fn watch(
    spaces: &crate::Spaces,
    space: &Space,
    started: &Value,
    a: &i::SpaceJob,
) -> Value {
    let Some(ext) = spaces.extension_for(WATCH_OP) else {
        return json!({"registered": false,
            "reason": "this runtime has no Cua Spaces daemon to follow the job; read it with action logs"});
    };
    let call = crate::extension::ToolCall {
        spaces,
        tool: WATCH_OP,
        args: json!({"space": space.id().to_string(), "job": started["job"],
                     "name": started["name"], "wake": a.wake}),
        broker: None,
        pinned: Some(space),
        caller: crate::extension::Caller::current(),
    };
    match ext.call_tool(call).await {
        Ok(_) => json!({"registered": true}),
        Err(e) => json!({"registered": false, "reason": e.to_string()}),
    }
}

/// The one line that says how a job ended, the same everywhere it is posted:
/// `JOB <id> <state> exit=<n> <elapsed> log=<size>B [name]`.
pub fn result_line(s: &JobStatus) -> String {
    let mut line = format!("JOB {} {}", s.job, s.state);
    match (s.exit_code, s.signal) {
        (Some(c), _) => line += &format!(" exit={c}"),
        (None, Some(sig)) => line += &format!(" signal={sig}"),
        _ => {}
    }
    line += &format!(" {} log={}B", s.elapsed_text(), s.size);
    if !s.name.is_empty() {
        line += &format!(" | {}", s.name);
    }
    line
}

/// The checks every runtime's Spaces must pass: the same calls, the same
/// answers, whatever the Space runs on (container, gVisor, microVM, VM,
/// cloud). Nothing here knows the runtime; a backend's e2e suite calls
/// [`conformance::run`] on a Space it made.
pub mod conformance {
    use super::*;

    async fn until_ended(space: &Space, id: &str, within: Duration) -> Result<JobStatus> {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let st = space.job_status(id).await?;
            if st.finished() {
                return Ok(st);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Timeout(format!(
                    "job {id} did not end in {within:?}"
                )));
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    fn fail(what: &str, got: impl std::fmt::Debug) -> Error {
        Error::Agent(format!("space_job conformance: {what}: {got:?}"))
    }

    fn start(command: &str) -> i::SpaceJob {
        i::SpaceJob {
            action: "start".into(),
            command: Some(command.into()),
            ..Default::default()
        }
    }

    /// Runs the conformance checks on `space` (a Linux or macOS guest).
    pub async fn run(space: &Space) -> Result<()> {
        // 1. A job returns at once, ends with its exit status, and its log
        //    reads by offset without losing or repeating a byte.
        let t0 = tokio::time::Instant::now();
        let st = space
            .job_start(&start(
                "printf 'one\\n'; sleep 1; printf 'two\\n'; echo oops >&2; exit 3",
            ))
            .await?;
        if t0.elapsed() > Duration::from_secs(20) || st.state != "running" {
            return Err(fail("start returns at once, running", &st));
        }
        let done = until_ended(space, &st.job, Duration::from_secs(60)).await?;
        if (done.state.as_str(), done.exit_code) != ("exited", Some(3)) {
            return Err(fail("exit status", &done));
        }
        let first = space.job_logs(&st.job, Some(0), 5, Duration::ZERO).await?;
        let rest = space
            .job_logs(&st.job, Some(first.next_offset), 1000, Duration::ZERO)
            .await?;
        let all = format!("{}{}", first.text, rest.text);
        if !first.more || rest.more || !all.starts_with("one\ntwo\n") || !all.contains("oops") {
            return Err(fail("log offsets", (&first, &rest)));
        }
        if rest.next_offset != done.size {
            return Err(fail("next_offset reaches the log size", (&rest, &done)));
        }
        let tail = space.job_logs(&st.job, None, 4, Duration::ZERO).await?;
        if tail.text != all[all.len() - 4..] {
            return Err(fail("the end of the log without an offset", &tail));
        }

        // 2. cwd and env reach the command.
        let mut a = start("printf '%s %s' \"$CUA_JOB_CONFORMANCE\" \"$(pwd)\"");
        a.cwd = Some("/".into());
        a.env = Some(BTreeMap::from([(
            "CUA_JOB_CONFORMANCE".to_string(),
            "it's set".to_string(),
        )]));
        let st = space.job_start(&a).await?;
        until_ended(space, &st.job, Duration::from_secs(60)).await?;
        let l = space.job_logs(&st.job, None, 100, Duration::ZERO).await?;
        if l.text != "it's set /" {
            return Err(fail("cwd and env", &l));
        }

        // 3. follow waits for output; cancel stops the whole tree.
        let st = space
            .job_start(&start("sleep 300 & sleep 300; echo never"))
            .await?;
        let waited = tokio::time::Instant::now();
        let quiet = space
            .job_logs(&st.job, Some(0), 100, Duration::from_millis(2500))
            .await?;
        if quiet.status.state != "running" || waited.elapsed() < Duration::from_secs(2) {
            return Err(fail("follow waits while the job is quiet", &quiet));
        }
        let c = space.job_cancel(&st.job).await?;
        if c.state != "cancelled" || c.signal != Some(15) {
            return Err(fail("cancel", &c));
        }
        let again = space.job_cancel(&st.job).await?;
        if again.state != "cancelled" {
            return Err(fail("cancel twice", &again));
        }

        // 4. A timeout stops the job and says so.
        let mut a = start("sleep 300");
        a.timeout = Some(2);
        let st = space.job_start(&a).await?;
        let done = until_ended(space, &st.job, Duration::from_secs(60)).await?;
        if done.state != "timed_out" {
            return Err(fail("timeout", &done));
        }

        // 5. A condition ends the job when it holds.
        let flag = format!("/tmp/cua-job-conformance-{}", new_job_id());
        let st = space
            .job_start(&i::SpaceJob {
                action: "start".into(),
                until: Some(i::JobUntil {
                    file: Some(flag.clone()),
                    interval: Some(1),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .await?;
        tokio::time::sleep(Duration::from_secs(2)).await;
        if space.job_status(&st.job).await?.state != "running" {
            return Err(fail("a condition waits", &st));
        }
        space
            .bash(&format!("touch {flag}"), Duration::from_secs(30))
            .await?;
        let done = until_ended(space, &st.job, Duration::from_secs(60)).await?;
        space
            .bash(&format!("rm -f {flag}"), Duration::from_secs(30))
            .await?;
        if (done.state.as_str(), done.exit_code) != ("exited", Some(0)) {
            return Err(fail("a condition ends the job", &done));
        }

        // 6. The Space's jobs list, and an unknown job is not found.
        let jobs = space.job_list().await?;
        if !jobs.iter().any(|j| j.job == st.job) {
            return Err(fail("list includes the job", &jobs));
        }
        match space.job_status("jnosuchjob").await {
            Err(Error::NotFound(_)) => {}
            other => return Err(fail("an unknown job is not found", other)),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn run_sh(home: &std::path::Path, script: &str) -> (bool, Vec<u8>, String) {
        let out = Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .env("HOME", home)
            .output()
            .expect("sh runs");
        (
            out.status.success(),
            out.stdout,
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn start(home: &std::path::Path, a: &i::SpaceJob) -> JobStatus {
        let id = new_job_id();
        let (ok, out, err) = run_sh(home, &start_script(&id, a).unwrap());
        assert!(ok, "start failed: {err}");
        parse_status(&String::from_utf8_lossy(&out)).unwrap()
    }

    fn status(home: &std::path::Path, id: &str) -> JobStatus {
        let (ok, out, err) = run_sh(home, &status_script(id).unwrap());
        assert!(ok, "{err}");
        parse_status(&String::from_utf8_lossy(&out)).unwrap()
    }

    fn wait_done(home: &std::path::Path, id: &str) -> JobStatus {
        for _ in 0..100 {
            let s = status(home, id);
            if s.finished() {
                return s;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("job {id} did not end");
    }

    fn job(command: &str) -> i::SpaceJob {
        i::SpaceJob {
            action: "start".into(),
            command: Some(command.into()),
            ..Default::default()
        }
    }

    #[test]
    fn ids_and_quoting_are_safe() {
        assert_eq!(sh_quote("a'b"), r"'a'\''b'");
        let id = new_job_id();
        assert_eq!(id.len(), 13);
        assert!(valid_job_id(&id).is_ok());
        assert!(valid_job_id("../x").is_err());
        assert!(valid_job_id("a b").is_err());
        assert!(valid_job_id("").is_err());
    }

    #[test]
    fn a_job_runs_detached_and_keeps_its_exit_status() {
        let home = tempfile::tempdir().unwrap();
        let st = start(home.path(), &job("echo hello; echo oops >&2; exit 3"));
        let done = wait_done(home.path(), &st.job);
        assert_eq!(done.state, "exited");
        assert_eq!(done.exit_code, Some(3));
        let (ok, out, _) = run_sh(home.path(), &logs_script(&st.job, None, 100).unwrap());
        assert!(ok);
        let logs = parse_logs(&out, 100).unwrap();
        assert!(logs.text.contains("hello") && logs.text.contains("oops"));
        assert_eq!(logs.offset, 0);
        assert_eq!(logs.next_offset, logs.status.size);
        assert!(!logs.more);
    }

    #[test]
    fn logs_read_on_from_an_offset_without_losing_bytes() {
        let home = tempfile::tempdir().unwrap();
        let st = start(home.path(), &job("printf 'abcdefghij'"));
        wait_done(home.path(), &st.job);
        let (_, out, _) = run_sh(home.path(), &logs_script(&st.job, Some(0), 4).unwrap());
        let a = parse_logs(&out, 4).unwrap();
        assert_eq!((a.text.as_str(), a.next_offset, a.more), ("abcd", 4, true));
        let (_, out, _) = run_sh(
            home.path(),
            &logs_script(&st.job, Some(a.next_offset), 100).unwrap(),
        );
        let b = parse_logs(&out, 100).unwrap();
        assert_eq!(
            (b.text.as_str(), b.next_offset, b.more),
            ("efghij", 10, false)
        );
        // Without an offset: the end of the log.
        let (_, out, _) = run_sh(home.path(), &logs_script(&st.job, None, 3).unwrap());
        let c = parse_logs(&out, 3).unwrap();
        assert_eq!((c.text.as_str(), c.offset), ("hij", 7));
    }

    #[test]
    fn cwd_and_env_reach_the_command() {
        let home = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let mut a = job("pwd; echo \"$GREETING\"");
        a.cwd = Some(work.path().to_string_lossy().into_owned());
        a.env = Some(BTreeMap::from([("GREETING".into(), "it's fine".into())]));
        let st = start(home.path(), &a);
        wait_done(home.path(), &st.job);
        let (_, out, _) = run_sh(home.path(), &logs_script(&st.job, None, 1000).unwrap());
        let text = parse_logs(&out, 1000).unwrap().text;
        assert!(text.contains("it's fine"), "{text}");
        let real = work.path().canonicalize().unwrap();
        assert!(
            text.contains(real.to_str().unwrap()) || text.contains(work.path().to_str().unwrap()),
            "{text}"
        );
        a.env = Some(BTreeMap::from([("bad name".into(), "x".into())]));
        assert!(start_script("j1", &a).is_err());
    }

    #[test]
    fn cancel_stops_the_whole_process_tree() {
        let home = tempfile::tempdir().unwrap();
        let st = start(home.path(), &job("sleep 300 & sleep 300; echo never"));
        assert_eq!(status(home.path(), &st.job).state, "running");
        let (ok, out, err) = run_sh(home.path(), &cancel_script(&st.job).unwrap());
        assert!(ok, "{err}");
        let done = parse_status(&String::from_utf8_lossy(&out)).unwrap();
        assert_eq!(done.state, "cancelled");
        assert_eq!(done.signal, Some(15));
        let (_, out, _) = run_sh(home.path(), &logs_script(&st.job, None, 100).unwrap());
        assert!(!parse_logs(&out, 100).unwrap().text.contains("never"));
        // Cancelling again changes nothing.
        let (_, out, _) = run_sh(home.path(), &cancel_script(&st.job).unwrap());
        assert_eq!(
            parse_status(&String::from_utf8_lossy(&out)).unwrap().state,
            "cancelled"
        );
    }

    #[test]
    fn a_timeout_stops_the_job_and_says_so() {
        let home = tempfile::tempdir().unwrap();
        let mut a = job("sleep 300");
        a.timeout = Some(1);
        let st = start(home.path(), &a);
        let done = wait_done(home.path(), &st.job);
        assert_eq!(done.state, "timed_out");
    }

    #[test]
    fn a_runner_that_died_without_a_record_is_lost() {
        let home = tempfile::tempdir().unwrap();
        let st = start(home.path(), &job("sleep 300"));
        let dir = home.path().join(".cua/jobs").join(&st.job);
        let runner: i32 = std::fs::read_to_string(dir.join("runner.pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let child: i32 = std::fs::read_to_string(dir.join("child.pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let _ = Command::new("kill")
            .args(["-9", &runner.to_string()])
            .status();
        let _ = Command::new("kill")
            .args(["-9", "--", &format!("-{child}")])
            .status();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(status(home.path(), &st.job).state, "lost");
    }

    #[test]
    fn listing_is_newest_first_and_unknown_jobs_are_not_found() {
        let home = tempfile::tempdir().unwrap();
        let mut a = job("true");
        a.name = Some("first".into());
        let one = start(home.path(), &a);
        std::thread::sleep(Duration::from_millis(1100));
        a.name = Some("second\nline".into());
        let two = start(home.path(), &a);
        wait_done(home.path(), &one.job);
        wait_done(home.path(), &two.job);
        let (_, out, _) = run_sh(home.path(), &list_script());
        let jobs = parse_list(&String::from_utf8_lossy(&out));
        assert_eq!(
            jobs.iter().map(|j| j.name.as_str()).collect::<Vec<_>>(),
            ["second line", "first"]
        );
        let (_, out, _) = run_sh(home.path(), &status_script("nope").unwrap());
        assert!(matches!(
            parse_status(&String::from_utf8_lossy(&out)),
            Err(Error::NotFound(_))
        ));
    }

    #[test]
    fn a_file_condition_ends_when_the_file_appears() {
        let home = tempfile::tempdir().unwrap();
        let flag = home.path().join("ready.flag");
        let a = i::SpaceJob {
            action: "start".into(),
            until: Some(i::JobUntil {
                file: Some(flag.to_string_lossy().into_owned()),
                interval: Some(1),
                ..Default::default()
            }),
            name: Some("wait for flag".into()),
            ..Default::default()
        };
        let st = start(home.path(), &a);
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(status(home.path(), &st.job).state, "running");
        std::fs::write(&flag, "x").unwrap();
        let done = wait_done(home.path(), &st.job);
        assert_eq!((done.state.as_str(), done.exit_code), ("exited", Some(0)));
    }

    #[test]
    fn a_command_condition_ends_when_it_passes() {
        let home = tempfile::tempdir().unwrap();
        let flag = home.path().join("go");
        let a = i::SpaceJob {
            action: "start".into(),
            until: Some(i::JobUntil {
                command: Some(format!("test -f {}", sh_quote(&flag.to_string_lossy()))),
                interval: Some(1),
                ..Default::default()
            }),
            ..Default::default()
        };
        let st = start(home.path(), &a);
        std::fs::write(&flag, "x").unwrap();
        assert_eq!(wait_done(home.path(), &st.job).exit_code, Some(0));
    }

    #[test]
    fn conditions_are_checked() {
        let none = i::JobUntil::default();
        assert!(until_script(&none).is_err());
        let two = i::JobUntil {
            file: Some("/x".into()),
            port: Some(1),
            ..Default::default()
        };
        assert!(until_script(&two).is_err());
        let bad_host = i::JobUntil {
            port: Some(80),
            host: Some("a;b".into()),
            ..Default::default()
        };
        assert!(until_script(&bad_host).is_err());
        let pr = until_script(&i::JobUntil {
            pr_checks: Some("acme/widgets#42".into()),
            ..Default::default()
        })
        .unwrap();
        assert!(pr.contains("gh pr checks '42' --repo 'acme/widgets' --required --watch"));
        assert!(start_script("j1", &i::SpaceJob::default()).is_err());
        let both = i::SpaceJob {
            command: Some("true".into()),
            until: Some(i::JobUntil {
                file: Some("/x".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(start_script("j1", &both).is_err());
    }

    #[test]
    fn the_result_line_is_structured() {
        let mut s = JobStatus {
            job: "j1".into(),
            name: "unit tests".into(),
            state: "exited".into(),
            exit_code: Some(0),
            signal: None,
            started_at: Some(10),
            ended_at: Some(785),
            elapsed_s: 775,
            size: 2048,
            command: "cargo test".into(),
        };
        assert_eq!(
            result_line(&s),
            "JOB j1 exited exit=0 12m55s log=2048B | unit tests"
        );
        s.state = "cancelled".into();
        s.exit_code = None;
        s.signal = Some(15);
        s.name.clear();
        assert_eq!(
            result_line(&s),
            "JOB j1 cancelled signal=15 12m55s log=2048B"
        );
        assert_eq!(elapsed_text(3723), "1h02m03s");
        assert_eq!(elapsed_text(9), "9s");
    }
}
