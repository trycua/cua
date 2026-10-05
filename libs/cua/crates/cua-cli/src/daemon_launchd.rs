//! `cua daemon start` through the Cua Spaces app's launchd agent (macOS).
//!
//! A daemon spawned as a child process answers to macOS privacy checks as
//! part of whoever spawned it: for the app's `cua daemon start`, the app.
//! Local Network access (the macOS VMs on vmnet, 192.168.64.x) then hangs
//! on the app process, and a daemon left running after the app quits can
//! lose it ("No route to host" when a Space is created). The app therefore
//! registers the daemon as its own launchd agent (`SMAppService`), which
//! macOS ties to the app's bundle and its Local Network permission whether
//! or not the app runs, and passes the agent's label in
//! [`LABEL_ENV`]. Then `cua daemon start` has launchd run the daemon
//! instead of spawning it; anything else (a terminal, an agent's MCP
//! server, a start with non-default arguments) spawns it as before.

/// The launchd label of the app's daemon agent, set by the app when the
/// agent is registered.
pub(crate) const LABEL_ENV: &str = "CUA_DAEMON_LAUNCHD_LABEL";

/// The label to start the daemon with, if this start may go through
/// launchd: the app passed one, and the agent runs exactly what a spawn
/// would (this executable, default arguments), since the agent's property
/// list fixes both.
pub(crate) fn label(env: Option<&str>, default_args: bool, exe_is_self: bool) -> Option<String> {
    let label = env.map(str::trim).filter(|l| !l.is_empty())?;
    let valid = label
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    (valid && default_args && exe_is_self).then(|| label.to_string())
}

/// The running process of a job, from `launchctl print <target>`: its
/// top-level `pid = N` line (a job that is loaded but not running has none).
pub(crate) fn job_pid(print: &str) -> Option<u32> {
    print.lines().find_map(|l| {
        // Top-level properties are indented by exactly one tab; nested
        // dictionaries (endpoints, environment) by more.
        let rest = l.strip_prefix('\t').filter(|r| !r.starts_with('\t'))?;
        rest.strip_prefix("pid = ")?.trim().parse().ok()
    })
}

/// `gui/<uid>/<label>`.
pub(crate) fn target(uid: u32, label: &str) -> String {
    format!("gui/{uid}/{label}")
}

/// Has launchd (re)start the job `label` in this user's GUI session and
/// returns its pid. `kickstart -k` replaces a process of the job that runs
/// but did not answer (the caller found no daemon answering).
#[cfg(target_os = "macos")]
pub(crate) fn kickstart(label: &str) -> Result<u32, String> {
    use std::process::Command;
    use std::time::{Duration, Instant};
    // SAFETY: getuid has no preconditions.
    let target = target(unsafe { libc::getuid() }, label);
    let out = Command::new("/bin/launchctl")
        .args(["kickstart", "-k", &target])
        .output()
        .map_err(|e| format!("launchctl: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "launchctl kickstart {target}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let print = Command::new("/bin/launchctl")
            .args(["print", &target])
            .output()
            .map_err(|e| format!("launchctl: {e}"))?;
        if let Some(pid) = job_pid(&String::from_utf8_lossy(&print.stdout)) {
            return Ok(pid);
        }
        if Instant::now() >= deadline {
            return Err(format!("launchd did not run {target}"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn kickstart(label: &str) -> Result<u32, String> {
    Err(format!("{label}: launchd runs only on macOS"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_apps_default_start_goes_through_launchd() {
        let l = "com.trycua.spaces.daemon";
        assert_eq!(label(Some(l), true, true).as_deref(), Some(l));
        assert_eq!(
            label(Some(" com.trycua.spaces.daemon\n"), true, true).as_deref(),
            Some(l)
        );
        // Not set by the app (a terminal, an MCP server): spawn.
        assert_eq!(label(None, true, true), None);
        assert_eq!(label(Some(""), true, true), None);
        // --socket, --state-dir or another --loopback: the agent would not
        // run them.
        assert_eq!(label(Some(l), false, true), None);
        // Another executable runs the daemon (an installed Cua Spaces CLI).
        assert_eq!(label(Some(l), true, false), None);
        // Never a target path of its own.
        assert_eq!(label(Some("gui/501/x"), true, true), None);
        assert_eq!(label(Some("a b"), true, true), None);
    }

    #[test]
    fn reads_the_jobs_own_pid() {
        let running = "gui/501/com.trycua.spaces.daemon = {\n\
            \tactive count = 1\n\
            \tpath = /Applications/Cua Spaces.app/Contents/Library/LaunchAgents/com.trycua.spaces.daemon.plist\n\
            \tstate = running\n\
            \tprogram = /Applications/Cua Spaces.app/Contents/MacOS/cua\n\
            \tenvironment = {\n\
            \t\tpid = 7\n\
            \t}\n\
            \tpid = 4242\n\
            \timmediate reason = kickstart\n\
            }\n";
        assert_eq!(job_pid(running), Some(4242));
        let idle = "gui/501/com.trycua.spaces.daemon = {\n\
            \tstate = not running\n\
            \tlast exit code = 0\n\
            }\n";
        assert_eq!(job_pid(idle), None);
        assert_eq!(job_pid(""), None);
        assert_eq!(job_pid("\tpid = x\n"), None);
    }

    #[test]
    fn targets_the_users_gui_session() {
        assert_eq!(
            target(501, "com.trycua.spaces.daemon"),
            "gui/501/com.trycua.spaces.daemon"
        );
    }
}
