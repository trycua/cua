//! What runs in a runner's Space.
//!
//! The runner lives in `~/actions-runner` of the Space's user (`lume` on
//! the macOS images). An image that already has `~/actions-runner/run.sh`
//! (a golden image with the runner baked in) is used as it is; otherwise
//! the pool copies the `actions/runner` tarball from the host's cache
//! ([`crate::cache`]) into the Space and unpacks it there, so a job never
//! waits for a download.
//!
//! The runner starts through a small wrapper, `~/actions-runner/cua-run.sh`
//! ([`WRAPPER`]): it reads the JIT configuration from
//! `~/actions-runner/.cua-jitconfig` (written through the Space's file API
//! with mode 0600, never on a host command line), deletes the file, runs
//! `./run.sh --jitconfig <config>` (as GitHub documents) and, when the
//! runner exits after its one job, writes the exit code to `.cua-exit`. The
//! pool reads that marker (and the wrapper's pid) to know the job is over.

/// The runner directory under the Space user's home.
pub const RUNNER_DIR: &str = "actions-runner";

/// The work folder, relative to the runner directory.
pub const WORK_FOLDER: &str = "_work";

/// The launch wrapper (bash; every image has it).
pub const WRAPPER: &str = r#"#!/bin/bash
# Started by `cua ci`: one GitHub Actions job, then exit.
set -u
cd "$(dirname "$0")" || exit 96
echo $$ > .cua-pid
cfg_file=.cua-jitconfig
if [ ! -s "$cfg_file" ]; then
  echo "cua-run: no JIT configuration in $cfg_file" > cua-run.log
  echo 97 > .cua-exit
  exit 97
fi
cfg="$(cat "$cfg_file")"
rm -f "$cfg_file"
# cua-ci:exports
# The .NET runner needs ICU; without it, run in invariant mode.
if [ "$(uname -s)" = Linux ] && ! (ldconfig -p 2>/dev/null | grep -q libicu); then
  export DOTNET_SYSTEM_GLOBALIZATION_INVARIANT=1
fi
./run.sh --jitconfig "$cfg" > cua-run.log 2>&1
code=$?
echo "$code" > .cua-exit.tmp && mv .cua-exit.tmp .cua-exit
exit "$code"
"#;

/// The wrapper with the performance core's exports in (the job hooks, the
/// cache folders, git's alternate object directory when a mirror is
/// mounted). Every export is conditional on the files being there, so it is
/// the same script for every VM.
pub fn wrapper_script() -> String {
    WRAPPER.replace("# cua-ci:exports\n", &crate::perf::boot::wrapper_exports())
}

/// Single-quotes `s` for `/bin/sh`.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The runner directory of a home.
pub fn runner_dir(home: &str) -> String {
    format!("{}/{RUNNER_DIR}", home.trim_end_matches('/'))
}

/// Where the JIT configuration is written.
pub fn jit_path(home: &str) -> String {
    format!("{}/.cua-jitconfig", runner_dir(home))
}

/// Where the wrapper is written.
pub fn wrapper_path(home: &str) -> String {
    format!("{}/cua-run.sh", runner_dir(home))
}

/// Where the tarball is copied before it is unpacked.
pub fn tarball_path(home: &str) -> String {
    format!("{}/.cua-actions-runner.tar.gz", home.trim_end_matches('/'))
}

/// The runner's diagnostic logs.
pub fn diag_dir(home: &str) -> String {
    format!("{}/_diag", runner_dir(home))
}

/// The runner's console output.
pub fn console_log(home: &str) -> String {
    format!("{}/cua-run.log", runner_dir(home))
}

/// The `spawn` tag of a slot's runner.
pub fn process_tag(slot: &str) -> String {
    format!("cua-ci-{slot}")
}

/// Prints `key=value` lines: the home, OS, architecture, whether the
/// runner is there, and the resolver lines of the guest's resolv.conf
/// files.
pub const PROBE: &str = r#"# cua-ci:probe
echo "home=$HOME"
echo "os=$(uname -s)"
echo "arch=$(uname -m)"
echo "now_ms=$(perl -MTime::HiRes=time -e 'printf "%d", time()*1000' 2>/dev/null)"
if [ -x "$HOME/actions-runner/run.sh" ]; then echo runner=present; else echo runner=missing; fi
for f in /etc/resolv.conf /run/systemd/resolve/resolv.conf; do
  [ -r "$f" ] && grep '^nameserver' "$f" | sed 's/^/resolv=/'
done
exit 0
"#;

/// What [`PROBE`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Probe {
    /// `$HOME`.
    pub home: String,
    /// `uname -s` (`Darwin`, `Linux`).
    pub os: String,
    /// `uname -m` (`arm64`, `x86_64`, `aarch64`).
    pub arch: String,
    /// `~/actions-runner/run.sh` exists.
    pub runner_present: bool,
    /// The `nameserver` lines.
    pub resolv: String,
    /// The guest's clock, Unix ms, when it could be read: the offset to
    /// the host's clock shifts the guest's marks onto it.
    pub clock_ms: Option<u64>,
}

/// Parses [`PROBE`]'s output.
pub fn parse_probe(out: &str) -> Result<Probe, String> {
    let mut p = Probe::default();
    for line in out.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        match k {
            "home" => p.home = v.trim().to_string(),
            "os" => p.os = v.trim().to_string(),
            "arch" => p.arch = v.trim().to_string(),
            "runner" => p.runner_present = v.trim() == "present",
            "now_ms" => p.clock_ms = v.trim().parse().ok(),
            "resolv" => {
                p.resolv.push_str(v.trim());
                p.resolv.push('\n');
            }
            _ => {}
        }
    }
    if !p.home.starts_with('/') {
        return Err(format!(
            "the Space did not report its home directory ({out:?})"
        ));
    }
    Ok(p)
}

/// The `actions/runner` platform of a guest (`osx-arm64`, `linux-x64`,
/// `linux-arm64`), from `uname -s` and `uname -m`.
pub fn platform(os: &str, arch: &str) -> Option<&'static str> {
    let arm = matches!(arch, "arm64" | "aarch64");
    let x64 = matches!(arch, "x86_64" | "amd64");
    match os {
        "Darwin" if arm => Some("osx-arm64"),
        "Darwin" if x64 => Some("osx-x64"),
        "Linux" if arm => Some("linux-arm64"),
        "Linux" if x64 => Some("linux-x64"),
        _ => None,
    }
}

/// Unpacks the tarball at [`tarball_path`] into the runner directory.
pub fn install_script(home: &str) -> String {
    format!(
        "# cua-ci:install\nset -e\nd={dir}\nmkdir -p \"$d\"\ntar -xzf {tgz} -C \"$d\"\nrm -f \
         {tgz}\ntest -x \"$d/run.sh\"\n",
        dir = sh_quote(&runner_dir(home)),
        tgz = sh_quote(&tarball_path(home)),
    )
}

/// Clears the markers of an earlier run (a golden image may carry them).
pub fn prepare_script(home: &str) -> String {
    let h = home.trim_end_matches('/');
    format!(
        "# cua-ci:prepare\ncd {} && rm -f .cua-exit .cua-exit.tmp .cua-pid .cua-jitconfig\nrm -rf {t} {s} {d}\nexit 0\n",
        sh_quote(&runner_dir(home)),
        t = sh_quote(&format!("{h}/{}/timers.jsonl", crate::perf::boot::DIR)),
        s = sh_quote(&format!("{h}/{}/restore.stamp", crate::perf::boot::DIR)),
        d = sh_quote(&crate::perf::boot::delta_dir(home)),
    )
}

/// Prints `exit=<code>`, `running`, `starting` or `gone`.
pub fn status_script(home: &str) -> String {
    format!(
        "# cua-ci:status\nd={}\nif [ -f \"$d/.cua-exit\" ]; then echo \"exit=$(cat \
         \"$d/.cua-exit\")\"; else\nif [ -f \"$d/.cua-pid\" ]; then\n  if kill -0 \"$(cat \
         \"$d/.cua-pid\")\" 2>/dev/null; then echo running; else echo gone; fi\nelse echo \
         starting; fi\nfi\nif grep -q '\"k\":\"job_end\"' {} 2>/dev/null; then echo job_done=1; fi\nl=$(grep -m1 'Running job:' \"$d/cua-run.log\" 2>/dev/null); [ -n \"$l\" ] && echo \"assigned=${{l%%: Running job:*}}\"\nexit 0\n",
        sh_quote(&runner_dir(home)),
        sh_quote(&crate::perf::boot::timers_path(home)),
    )
}

/// The runner process as [`status_script`] saw it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuestState {
    /// The wrapper (and the runner) runs.
    Running,
    /// The wrapper has not written its pid yet.
    Starting,
    /// The runner exited with this code.
    Exited(i32),
    /// The wrapper is gone without an exit code (killed, rebooted).
    Gone,
}

/// Parses [`status_script`]'s output.
pub fn parse_status(out: &str) -> GuestState {
    let s = out.lines().next().unwrap_or("").trim();
    if let Some(code) = s.strip_prefix("exit=") {
        return GuestState::Exited(code.trim().parse().unwrap_or(-1));
    }
    match s {
        "running" => GuestState::Running,
        "starting" => GuestState::Starting,
        _ => GuestState::Gone,
    }
}

/// [`status_script`] saw the job-completed hook's mark: the job's steps are
/// over and the runner is reporting the result and exiting (the pool
/// looks again soon, but never deletes the VM before the runner exits).
pub fn parse_job_done(out: &str) -> bool {
    out.lines().any(|l| l.trim() == "job_done=1")
}

/// When the runner says it was given a job (`assigned=2026-10-28 12:00:03Z`
/// from its console log, the guest's clock): the exact moment GitHub
/// handed the job over, without waiting for the next poll of GitHub's
/// runner list.
pub fn parse_assigned(out: &str) -> Option<String> {
    out.lines()
        .find_map(|l| l.trim().strip_prefix("assigned="))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// What the runner's console output says about its job.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JobSummary {
    /// The job's name (`Running job: <name>`).
    pub job: Option<String>,
    /// Its result (`Job <name> completed with result: <result>`).
    pub result: Option<String>,
}

/// Reads [`JobSummary`] from the console log.
pub fn parse_console(log: &str) -> JobSummary {
    let mut s = JobSummary::default();
    for line in log.lines() {
        if let Some(i) = line.find("Running job: ") {
            s.job = Some(line[i + 13..].trim().to_string());
        }
        if let Some(i) = line.find("completed with result: ") {
            s.result = Some(line[i + 23..].trim().to_string());
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_output() {
        let p = parse_probe(
            "home=/Users/lume\nos=Darwin\narch=arm64\nrunner=missing\nresolv=nameserver 192.168.64.1\n",
        )
        .unwrap();
        assert_eq!(p.home, "/Users/lume");
        assert!(!p.runner_present);
        assert_eq!(platform(&p.os, &p.arch), Some("osx-arm64"));
        assert_eq!(p.resolv, "nameserver 192.168.64.1\n");
        assert_eq!(platform("Linux", "aarch64"), Some("linux-arm64"));
        assert_eq!(platform("Linux", "x86_64"), Some("linux-x64"));
        assert_eq!(platform("Windows_NT", "x86_64"), None);
        assert!(parse_probe("nothing").is_err());
    }

    #[test]
    fn status_words() {
        assert_eq!(parse_status("exit=0\n"), GuestState::Exited(0));
        assert_eq!(parse_status("exit=1"), GuestState::Exited(1));
        assert_eq!(parse_status("running\n"), GuestState::Running);
        assert_eq!(parse_status("starting"), GuestState::Starting);
        assert_eq!(parse_status("gone"), GuestState::Gone);
        // The job-done line follows the state and does not change it.
        assert_eq!(parse_status("running\njob_done=1\n"), GuestState::Running);
        assert_eq!(parse_status("exit=3\njob_done=1\n"), GuestState::Exited(3));
        assert!(parse_job_done("running\njob_done=1\n"));
        assert_eq!(
            parse_assigned("running\nassigned=2026-10-28 12:00:03Z\n").as_deref(),
            Some("2026-10-28 12:00:03Z")
        );
        assert_eq!(parse_assigned("running\n"), None);
        assert!(!parse_job_done("running\n"));
    }

    #[test]
    fn probe_reads_the_guest_clock() {
        let p =
            parse_probe("home=/h\nos=Linux\narch=arm64\nnow_ms=1800000000123\nrunner=missing\n")
                .unwrap();
        assert_eq!(p.clock_ms, Some(1_800_000_000_123));
        let p = parse_probe("home=/h\nos=Linux\narch=arm64\nnow_ms=\nrunner=missing\n").unwrap();
        assert_eq!(p.clock_ms, None);
    }

    #[test]
    fn the_wrapper_script_carries_the_exports() {
        let w = wrapper_script();
        assert!(!w.contains("# cua-ci:exports"));
        assert!(w.contains("ACTIONS_RUNNER_HOOK_JOB_STARTED"));
        assert!(w.find("ACTIONS_RUNNER_HOOK_JOB_STARTED").unwrap() < w.find("./run.sh").unwrap());
        assert!(w.contains("rm -f \"$cfg_file\""));
    }

    #[test]
    fn scripts_quote_the_home_and_never_carry_the_config() {
        let home = "/Users/it's me";
        assert!(install_script(home).contains(r"'/Users/it'\''s me/actions-runner'"));
        assert!(status_script(home).contains(".cua-exit"));
        assert!(prepare_script(home).contains("rm -f .cua-exit"));
        // The wrapper reads the configuration from the file and removes it.
        assert!(WRAPPER.contains("cfg=\"$(cat \"$cfg_file\")\""));
        assert!(WRAPPER.contains("rm -f \"$cfg_file\""));
        assert!(WRAPPER.contains("./run.sh --jitconfig \"$cfg\""));
        assert_eq!(
            jit_path("/Users/lume/"),
            "/Users/lume/actions-runner/.cua-jitconfig"
        );
    }

    #[test]
    fn console_job_summary() {
        let log = "√ Connected to GitHub\n2026-10-26 10:00:00Z: Listening for Jobs\n\
                   2026-10-26 10:00:03Z: Running job: build (macos)\n\
                   2026-10-26 10:04:00Z: Job build (macos) completed with result: Succeeded\n";
        assert_eq!(
            parse_console(log),
            JobSummary {
                job: Some("build (macos)".into()),
                result: Some("Succeeded".into())
            }
        );
    }

    /// The wrapper, run for real against a fake `run.sh`: it consumes the
    /// configuration file, passes it to run.sh, and records the exit code.
    #[cfg(unix)]
    #[test]
    fn the_wrapper_runs_one_job_and_records_its_exit() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        std::fs::write(dir.join("cua-run.sh"), WRAPPER).unwrap();
        std::fs::write(dir.join("run.sh"), "#!/bin/sh\necho \"args: $*\"\nexit 3\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.join("run.sh"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        std::fs::write(dir.join(".cua-jitconfig"), "CFG").unwrap();
        let st = std::process::Command::new("/bin/bash")
            .arg(dir.join("cua-run.sh"))
            .status()
            .unwrap();
        assert_eq!(st.code(), Some(3));
        assert!(
            !dir.join(".cua-jitconfig").exists(),
            "the config file is consumed"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join(".cua-exit"))
                .unwrap()
                .trim(),
            "3"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("cua-run.log"))
                .unwrap()
                .trim(),
            "args: --jitconfig CFG"
        );
        // The status script reads the marker.
        let home = dir.parent().unwrap();
        let name = dir.file_name().unwrap().to_str().unwrap();
        let script = status_script(home.to_str().unwrap()).replace(RUNNER_DIR, name);
        let out = std::process::Command::new("/bin/sh")
            .args(["-c", &script])
            .output()
            .unwrap();
        assert_eq!(
            parse_status(&String::from_utf8_lossy(&out.stdout)),
            GuestState::Exited(3)
        );
        // Without a configuration it refuses to start the runner.
        std::fs::remove_file(dir.join(".cua-exit")).unwrap();
        let st = std::process::Command::new("/bin/bash")
            .arg(dir.join("cua-run.sh"))
            .status()
            .unwrap();
        assert_eq!(st.code(), Some(97));
    }
}
