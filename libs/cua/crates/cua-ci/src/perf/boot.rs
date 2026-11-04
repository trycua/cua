//! What runs in the guest around a job.
//!
//! - **Job hooks.** The runner calls `ACTIONS_RUNNER_HOOK_JOB_STARTED`
//!   before a job's first step and `ACTIONS_RUNNER_HOOK_JOB_COMPLETED`
//!   after its last. The started hook restores the dependency caches from
//!   the read-only mount into local folders and records, in
//!   `~/.cua-ci/timers.jsonl`, when the job began and how long each
//!   restore took. The completed hook records when the job ended. The
//!   marks are what the host's phase events are made from
//!   ([`super::timers`]). A hook never fails a job: both end in `exit 0`.
//! - **No teardown at the hook.** The completed hook runs before the runner
//!   has reported the result to GitHub; deleting the VM then would lose
//!   the result. The pool still waits for the runner to exit
//!   ([`crate::guest::status_script`]) and only speeds up how often it
//!   looks once the mark is there.
//! - **Cache write-back.** [`delta_script`] runs after a trusted job
//!   succeeded: for each store folder that has files newer than the
//!   restore stamp it writes one tar of just those files. The host takes
//!   it from there ([`super::cachevol::CacheRoot::promote`]); the guest
//!   never writes the cache.
//! - **Trim.** [`trim_script`] turns off what a CI VM does not need
//!   (Spotlight indexing, photo and media analysis, update checks, sleep
//!   on macOS; update timers and snapd on Linux). It is baked into the
//!   golden image; a VM from a plain image runs it in the background at
//!   claim.

use super::cachevol::{STORE_ENV, STORES};
use crate::config::GuestOs;
use crate::guest;

/// Where the guest keeps the hooks and the marks, under `$HOME`.
pub const DIR: &str = ".cua-ci";

/// The hooks directory.
pub fn hooks_dir(home: &str) -> String {
    format!("{}/{DIR}/hooks", home.trim_end_matches('/'))
}

/// The job-started hook.
pub fn job_started_path(home: &str) -> String {
    format!("{}/job-started.sh", hooks_dir(home))
}

/// The job-completed hook.
pub fn job_completed_path(home: &str) -> String {
    format!("{}/job-completed.sh", hooks_dir(home))
}

/// The marks file.
pub fn timers_path(home: &str) -> String {
    format!("{}/{DIR}/timers.jsonl", home.trim_end_matches('/'))
}

/// The job environment file the host writes before the runner starts.
pub fn job_env_path(home: &str) -> String {
    format!("{}/{DIR}/job.env", home.trim_end_matches('/'))
}

/// The folder [`delta_script`] writes its tars to.
pub fn delta_dir(home: &str) -> String {
    format!("{}/{DIR}/delta", home.trim_end_matches('/'))
}

/// Milliseconds since the epoch in a guest shell (macOS `date` has no
/// `%N`; perl ships with macOS and every Linux image).
const MS_FN: &str = r#"ms() { perl -MTime::HiRes=time -e 'printf "%d", time()*1000' 2>/dev/null || echo $(( $(date +%s) * 1000 )); }
clean() { printf %s "$1" | tr -cd 'A-Za-z0-9._/:@+=-'; }
"#;

/// `case` arms that map a store name to its `cache-subfolder:guest-path`
/// pairs, from the one table the host also uses.
fn store_case() -> String {
    let mut s = String::new();
    for st in STORES.iter() {
        let pairs: Vec<String> = st
            .dirs
            .iter()
            .map(|(sub, dest)| format!("{sub}:{dest}"))
            .collect();
        s.push_str(&format!(
            "      {}) pairs=\"{}\";;\n",
            st.name,
            pairs.join(" ")
        ));
    }
    s.push_str("      *) pairs=\"\";;\n");
    s
}

/// The job-started hook.
pub fn job_started_hook() -> String {
    format!(
        r#"#!/bin/bash
# cua ci: runs before a job's first step. Restores the read-only caches into
# local folders, records the moments the bench measures. Never fails a job.
set +e
D="$HOME/{DIR}"; mkdir -p "$D"
{MS_FN}
[ -f "$D/job.env" ] && . "$D/job.env"
root="${{CUA_CI_CACHE_ROOT:-}}"
if [ -n "$root" ] && [ -d "$root" ] && [ -n "${{CUA_CI_CACHE_STORES:-}}" ]; then
  for item in $CUA_CI_CACHE_STORES; do
    store="${{item%%/*}}"
    src="$root/$item/current"
    case "$store" in
{cases}    esac
    t0=$(ms); hit=false; bytes=0
    if [ -d "$src" ]; then
      for p in $pairs; do
        sub="${{p%%:*}}"; dest="$HOME/${{p#*:}}"
        [ -d "$src/$sub" ] || continue
        mkdir -p "$dest"
        if command -v rsync >/dev/null 2>&1; then rsync -a "$src/$sub/" "$dest/" 2>/dev/null
        else cp -R "$src/$sub/." "$dest/" 2>/dev/null; fi
        hit=true
        k=$(du -sk "$dest" 2>/dev/null | cut -f1); bytes=$(( bytes + ${{k:-0}} * 1024 ))
      done
    fi
    t1=$(ms)
    printf '{{"k":"cache_restore","at":%s,"ms":%s,"store":"%s","bytes":%s,"hit":%s,"scope":"%s"}}\n' \
      "$t1" "$((t1 - t0))" "$(clean "$store")" "$bytes" "$hit" "$(clean "${{CUA_CI_CACHE_SCOPE:-}}")" >> "$D/timers.jsonl"
  done
fi
{registry}# Only what changes after this point is a cache write-back candidate.
touch "$D/restore.stamp"
if [ -n "${{GIT_ALTERNATE_OBJECT_DIRECTORIES:-}}" ]; then
  printf '{{"k":"mirror","used":true}}\n' >> "$D/timers.jsonl"
fi
printf '{{"k":"job_start","at":%s,"event":"%s","repo":"%s","ref":"%s","bench":"%s"}}\n' \
  "$(ms)" "$(clean "${{GITHUB_EVENT_NAME:-}}")" "$(clean "${{GITHUB_REPOSITORY:-}}")" \
  "$(clean "${{GITHUB_REF:-}}")" "$(clean "${{CUA_CI_BENCH_RUN_ID:-}}")" >> "$D/timers.jsonl"
exit 0
"#,
        cases = store_case(),
        registry = super::registry::hook_snippet()
    )
}

/// The job-completed hook: one mark.
pub fn job_completed_hook() -> String {
    format!(
        r#"#!/bin/bash
# cua ci: runs after a job's last step. Records when. Never fails a job.
set +e
D="$HOME/{DIR}"; mkdir -p "$D"
{MS_FN}
printf '{{"k":"job_end","at":%s}}\n' "$(ms)" >> "$D/timers.jsonl"
exit 0
"#
    )
}

/// The files a golden image carries: the hooks. `(path, content)`.
pub fn baked_files(home: &str, _trim: bool) -> Vec<(String, String)> {
    vec![
        (job_started_path(home), job_started_hook()),
        (job_completed_path(home), job_completed_hook()),
    ]
}

/// What the wrapper exports for the job: the hooks, the store folders and,
/// when the mirror is mounted, git's alternate object directory. The
/// wrapper reads `job.env` first, so `CUA_CI_MIRROR` is known.
pub fn wrapper_exports() -> String {
    let mut s = String::new();
    // The job's tools: a Space starts the runner with the bare system PATH,
    // so Homebrew's and rustup's folders (the golden image installs both)
    // are put in front when they exist, as on GitHub's own images.
    s.push_str(
        "for d in \"$HOME/.cargo/bin\" /opt/homebrew/sbin /opt/homebrew/bin; do\n  \
         [ -d \"$d\" ] && case \":$PATH:\" in *\":$d:\"*) ;; *) PATH=\"$d:$PATH\" ;; esac\n\
         done\nexport PATH\n",
    );
    s.push_str(&format!(
        "[ -f \"$HOME/{DIR}/job.env\" ] && . \"$HOME/{DIR}/job.env\"\n"
    ));
    s.push_str(&format!(
        "[ -x \"$HOME/{DIR}/hooks/job-started.sh\" ] && export ACTIONS_RUNNER_HOOK_JOB_STARTED=\"$HOME/{DIR}/hooks/job-started.sh\"\n"
    ));
    s.push_str(&format!(
        "[ -x \"$HOME/{DIR}/hooks/job-completed.sh\" ] && export ACTIONS_RUNNER_HOOK_JOB_COMPLETED=\"$HOME/{DIR}/hooks/job-completed.sh\"\n"
    ));
    for (k, rel) in STORE_ENV {
        s.push_str(&format!("export {k}=\"$HOME/{rel}\"\n"));
    }
    // The mirror only when it is really there: otherwise git would warn
    // about a missing object directory in every command of the job.
    s.push_str(
        "if [ -n \"${CUA_CI_MIRROR:-}\" ] && [ -d \"$CUA_CI_MIRROR/repo.git/objects\" ]; then\n  \
         export GIT_ALTERNATE_OBJECT_DIRECTORIES=\"$CUA_CI_MIRROR/repo.git/objects\"\nfi\n",
    );
    s
}

/// With `tar`, writes one tar per store folder that has files newer than the restore
/// stamp (`~/.cua-ci/delta/<store>--<sub>.tar`, entries relative to the
/// folder) and prints `dirty=<store>:<sub>` for each (without `tar` it only
/// prints: a job that may not write still gets its refused write reported). Runs after the
/// runner has finished a trusted job that succeeded.
pub fn delta_script(home: &str, tar: bool) -> String {
    let mut arms = String::new();
    for st in STORES.iter() {
        for (sub, dest) in st.dirs {
            arms.push_str(&format!(
                "  one {} {} {}\n",
                st.name,
                sub,
                guest::sh_quote(dest)
            ));
        }
    }
    format!(
        r#"# cua-ci:delta
H={home}
T={tar}
D="$H/{DIR}"; O="$D/delta"
rm -rf "$O"; mkdir -p "$O"
[ -f "$D/restore.stamp" ] || exit 0
one() {{
  d="$H/$3"
  [ -d "$d" ] || return 0
  if [ -n "$(cd "$d" && find . -newer "$D/restore.stamp" \( -type f -o -type l \) -print 2>/dev/null | head -1)" ]; then
    if [ "$T" = 1 ]; then
      (cd "$d" && find . -newer "$D/restore.stamp" \( -type f -o -type l \) -print0 \
        | tar -cf "$O/$1--$2.tar" --null -T - 2>/dev/null) && echo "dirty=$1:$2"
    else
      echo "dirty=$1:$2"
    fi
  fi
}}
{arms}exit 0
"#,
        home = guest::sh_quote(home.trim_end_matches('/')),
        tar = u8::from(tar),
    )
}

/// Parses [`delta_script`]'s output: `(store, sub)` of each tar.
pub fn parse_delta(out: &str) -> Vec<(String, String)> {
    out.lines()
        .filter_map(|l| l.strip_prefix("dirty="))
        .filter_map(|l| l.split_once(':'))
        .filter(|(s, sub)| {
            // Only what the table names: never a path the guest chose.
            STORES
                .iter()
                .any(|st| st.name == *s && st.dirs.iter().any(|(d, _)| d == sub))
        })
        .map(|(s, sub)| (s.to_string(), sub.to_string()))
        .collect()
}

/// Reads the marks and the step telemetry lines back (host side): the
/// marks, a separator line, then the worker log's telemetry lines.
pub fn collect_script(home: &str) -> String {
    format!(
        "# cua-ci:collect\ncat {t} 2>/dev/null\necho '---worker'\ncd {d} 2>/dev/null && grep -h \
         'Publish step telemetry' Worker_*.log 2>/dev/null | head -200\nexit 0\n",
        t = guest::sh_quote(&timers_path(home)),
        d = guest::sh_quote(&guest::diag_dir(home)),
    )
}

/// The post-job secret scan (`EVENTS.md` `secret_scan`): looks in the VM,
/// as the job's user, for what must never be there after a job: cloud
/// credential files and tokens in the environment of any process of the
/// user, GitHub App keys, SSH keys, registry logins. It prints counts and
/// names only, never a value, and exits 0. The runner's own variables
/// (`GITHUB_TOKEN`, `ACTIONS_*`) and its one-job configuration are expected.
pub fn secret_scan_script() -> String {
    r#"# cua-ci:secret-scan
found=0; names=""
hit() { found=$((found+1)); names="${names}${names:+,}$1"; }
for p in .aws/credentials .aws/config .config/gcloud .azure .ssh/id_rsa .ssh/id_ed25519 .ssh/id_ecdsa          .docker/config.json .netrc .npmrc .config/gh/hosts.yml .kube/config .hcloud .modal.toml; do
  [ -e "$HOME/$p" ] && hit "file:$p"
done
for f in "$HOME"/*.pem "$HOME"/*/*.pem "$HOME"/github-app* "$HOME"/*/github-app*; do
  [ -f "$f" ] && hit "pem:$(basename "$f")"
done
secret_name() {
  case "$1" in
    AWS_SECRET_ACCESS_KEY|AWS_SESSION_TOKEN|AWS_ACCESS_KEY_ID|GOOGLE_APPLICATION_CREDENTIALS|AZURE_CLIENT_SECRET|HCLOUD_TOKEN|MODAL_TOKEN_*|GITHUB_APP_*|CUA_RELAY_TOKEN*|CUA_CLIENT_SECRET|CUA_CLIENT_ID|FLEETS_TOKEN|CUA_API_KEY) return 0;;
  esac
  return 1
}
envnames=$( { env | cut -d= -f1; for e in /proc/[0-9]*/environ; do [ -r "$e" ] && tr '\0' '\n' < "$e" | cut -d= -f1; done; } 2>/dev/null | sort -u )
envhits=""
for n in $envnames; do
  secret_name "$n" && envhits="$envhits $n"
done
for n in $envhits; do hit "env:$n"; done
echo "found=$found"
echo "detail=$names"
exit 0
"#
    .to_string()
}

/// Reads [`secret_scan_script`]'s output: how many findings and their
/// names (a detail longer than 500 characters is cut).
pub fn parse_secret_scan(out: &str) -> Option<(u32, String)> {
    let mut found = None;
    let mut detail = String::new();
    for l in out.lines() {
        if let Some(n) = l.strip_prefix("found=") {
            found = n.trim().parse::<u32>().ok();
        } else if let Some(d) = l.strip_prefix("detail=") {
            detail = d
                .trim()
                .chars()
                .filter(|c| {
                    c.is_ascii_alphanumeric() || matches!(c, ':' | ',' | '_' | '.' | '-' | '/')
                })
                .take(500)
                .collect();
        }
    }
    found.map(|n| (n, detail))
}

/// Splits [`collect_script`]'s output.
pub fn split_collect(out: &str) -> (&str, &str) {
    match out.split_once("---worker\n") {
        Some((a, b)) => (a, b),
        None => (out, ""),
    }
}

/// Turns off what a CI VM does not need. Every line is best effort (no
/// sudo, no such service: skipped) and the script exits 0.
pub fn trim_script(os: GuestOs) -> String {
    match os {
        GuestOs::Macos => r#"# cua-ci:trim
sudo -n mdutil -a -i off >/dev/null 2>&1
sudo -n tmutil disable >/dev/null 2>&1
sudo -n pmset -a sleep 0 displaysleep 0 disksleep 0 powernap 0 autopoweroff 0 standby 0 >/dev/null 2>&1
sudo -n defaults write /Library/Preferences/com.apple.SoftwareUpdate AutomaticCheckEnabled -bool false >/dev/null 2>&1
sudo -n defaults write /Library/Preferences/com.apple.SoftwareUpdate AutomaticDownload -bool false >/dev/null 2>&1
defaults -currentHost write com.apple.screensaver idleTime 0 >/dev/null 2>&1
for svc in com.apple.photoanalysisd com.apple.mediaanalysisd com.apple.parsecd com.apple.suggestd; do
  launchctl disable "gui/$(id -u)/$svc" >/dev/null 2>&1
  launchctl bootout "gui/$(id -u)/$svc" >/dev/null 2>&1
done
for svc in com.apple.softwareupdated com.apple.metadata.mds; do
  sudo -n launchctl disable "system/$svc" >/dev/null 2>&1
done
exit 0
"#
        .to_string(),
        GuestOs::Linux => r#"# cua-ci:trim
sudo -n systemctl disable --now apt-daily.timer apt-daily-upgrade.timer motd-news.timer \
  man-db.timer unattended-upgrades.service snapd.service snapd.socket >/dev/null 2>&1
exit 0
"#
        .to_string(),
    }
}

#[cfg(test)]
mod tests;
