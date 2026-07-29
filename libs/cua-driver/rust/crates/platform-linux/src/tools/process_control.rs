use std::io;
#[cfg(target_os = "linux")]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::process::{Child, Command, Stdio};
use std::sync::{
    mpsc::{self, Receiver, RecvTimeoutError, Sender},
    OnceLock,
};
use std::time::Duration;
#[cfg(target_os = "linux")]
use std::time::Instant;

const REAP_INTERVAL: Duration = Duration::from_millis(50);
#[cfg(target_os = "linux")]
const TERMINATION_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ProcessObservation {
    pub(super) state: char,
    pub(super) start_time: u64,
}

impl ProcessObservation {
    #[cfg(target_os = "linux")]
    fn is_terminal(self) -> bool {
        matches!(self.state, 'Z' | 'X' | 'x')
    }
}

pub(super) fn spawn_launch_command(command: &str, additional_args: &[String]) -> io::Result<Child> {
    let mut argv = shlex::split(command).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "launch_path contains an unmatched shell quote",
        )
    })?;
    if argv.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "launch_path must contain a program",
        ));
    }
    let program = argv.remove(0);
    let mut launch = Command::new(program);
    launch
        .args(argv)
        .args(additional_args)
        .env("ACCESSIBILITY_ENABLED", "1")
        .env("NO_AT_BRIDGE", "0");
    spawn_background(&mut launch)
}

pub(super) fn spawn_background(command: &mut Command) -> io::Result<Child> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

pub(super) fn track_child(child: Child) -> io::Result<()> {
    match child_reaper()?.send(child) {
        Ok(()) => Ok(()),
        Err(error) => {
            let mut child = error.0;
            let _ = child.kill();
            let _ = child.wait();
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "Linux launch child reaper stopped",
            ))
        }
    }
}

#[cfg(target_os = "linux")]
pub(super) fn track_launched_child(child: Child) -> io::Result<(u32, Option<ProcessObservation>)> {
    track_launched_child_with(child, observe_process)
}

#[cfg(target_os = "linux")]
fn track_launched_child_with<F>(
    child: Child,
    observe: F,
) -> io::Result<(u32, Option<ProcessObservation>)>
where
    F: FnOnce(i32) -> io::Result<Option<ProcessObservation>>,
{
    let pid = child.id();
    let identity = observe(pid as i32);
    track_child(child)?;
    Ok((pid, identity?))
}

fn child_reaper() -> io::Result<&'static Sender<Child>> {
    static REAPER: OnceLock<Result<Sender<Child>, String>> = OnceLock::new();
    match REAPER.get_or_init(start_child_reaper) {
        Ok(sender) => Ok(sender),
        Err(message) => Err(io::Error::other(message.clone())),
    }
}

fn start_child_reaper() -> Result<Sender<Child>, String> {
    let (sender, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name("cua-driver-child-reaper".to_owned())
        .spawn(move || child_reaper_loop(receiver))
        .map_err(|error| format!("failed to start Linux launch child reaper: {error}"))?;
    Ok(sender)
}

fn child_reaper_loop(receiver: Receiver<Child>) {
    let mut children = Vec::new();
    loop {
        match receiver.recv_timeout(REAP_INTERVAL) {
            Ok(child) => children.push(child),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        children.extend(receiver.try_iter());
        reap_finished_children(&mut children);
    }
}

fn reap_finished_children(children: &mut Vec<Child>) {
    let mut index = 0;
    while index < children.len() {
        match children[index].try_wait() {
            Ok(Some(_)) => {
                children.swap_remove(index);
            }
            Ok(None) => index += 1,
            Err(error) => {
                tracing::warn!(%error, "failed to poll directly launched child");
                children.swap_remove(index);
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub(super) fn observe_process(pid: i32) -> io::Result<Option<ProcessObservation>> {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => parse_proc_stat(&stat).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(target_os = "linux")]
pub(super) fn process_still_running(
    pid: i32,
    expected: Option<ProcessObservation>,
) -> io::Result<bool> {
    let Some(expected) = expected else {
        return Ok(false);
    };
    Ok(observe_process(pid)?
        .is_some_and(|current| !current.is_terminal() && current.start_time == expected.start_time))
}

fn parse_proc_stat(stat: &str) -> io::Result<ProcessObservation> {
    let fields = stat
        .rfind(") ")
        .and_then(|name_end| stat.get(name_end + 2..))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "malformed /proc stat"))?;
    let mut fields = fields.split_whitespace();
    let state = fields
        .next()
        .and_then(|value| value.chars().next())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing process state"))?;
    let start_time = fields
        .nth(18)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing process start time"))?
        .parse()
        .map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid process start time: {error}"),
            )
        })?;
    Ok(ProcessObservation { state, start_time })
}

#[cfg(target_os = "linux")]
pub(super) fn terminate_process(pid: i32) -> io::Result<bool> {
    let process = PidFd::open(pid)?;
    process.send_signal(libc::SIGKILL)?;
    process.wait_for_exit(TERMINATION_TIMEOUT)
}

#[cfg(target_os = "linux")]
struct PidFd(OwnedFd);

#[cfg(target_os = "linux")]
impl PidFd {
    fn open(pid: i32) -> io::Result<Self> {
        // SAFETY: pidfd_open takes a numeric pid and zero flags. On success the
        // returned descriptor is uniquely owned and transferred to OwnedFd.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0u32) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the successful syscall returned a fresh owned descriptor.
        Ok(Self(unsafe { OwnedFd::from_raw_fd(fd as i32) }))
    }

    fn send_signal(&self, signal: i32) -> io::Result<()> {
        // SAFETY: the pidfd remains owned by self for the duration of the
        // syscall; a null siginfo pointer and zero flags are the documented
        // pidfd_send_signal form for ordinary signals.
        let result = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.0.as_raw_fd(),
                signal,
                std::ptr::null::<libc::siginfo_t>(),
                0u32,
            )
        };
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn wait_for_exit(&self, timeout: Duration) -> io::Result<bool> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let timeout_ms = if remaining.is_zero() {
                0
            } else {
                remaining.as_millis().clamp(1, i32::MAX as u128) as i32
            };
            let mut descriptor = libc::pollfd {
                fd: self.0.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: descriptor points to one initialized pollfd for the
            // duration of the call; the timeout is bounded to i32 milliseconds.
            let result = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
            if result > 0 {
                if descriptor.revents & libc::POLLNVAL != 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "pidfd became invalid while waiting for process exit",
                    ));
                }
                return Ok(descriptor.revents & (libc::POLLIN | libc::POLLHUP) != 0);
            }
            if result == 0 {
                return Ok(false);
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use super::{parse_proc_stat, spawn_launch_command, track_child, ProcessObservation};

    #[test]
    fn launch_path_preserves_shell_quoting() {
        let output_path = std::env::temp_dir().join(format!(
            "cua-driver-launch-quoting-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time after Unix epoch")
                .as_nanos()
        ));
        let script = format!(
            "printf '%s' 'quoted value' > {}",
            shell_quote(output_path.to_str().expect("UTF-8 temp path"))
        );
        let command = format!("sh -c {}", shell_quote(&script));
        let mut child = spawn_launch_command(&command, &[]).expect("spawn launch command");
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if child.try_wait().expect("poll launch command").is_some() {
                break;
            }
            assert!(Instant::now() < deadline, "launch command did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
        let actual = std::fs::read_to_string(&output_path).expect("launch output");
        assert_eq!(actual, "quoted value");
        let _ = std::fs::remove_file(output_path);
    }

    #[test]
    fn launch_path_appends_arguments_without_shell_interpolation() {
        let output =
            std::env::temp_dir().join(format!("cua-driver-launch-args-{}", std::process::id()));
        let script = format!(
            "printf '%s' \"$0\" > {}",
            shell_quote(output.to_str().expect("UTF-8 temp path"))
        );
        let command = format!("sh -c {}", shell_quote(&script));
        let arguments = vec!["value with spaces; $(false)".to_owned()];
        let mut child = spawn_launch_command(&command, &arguments).expect("spawn launch command");
        assert!(
            child.wait().expect("wait for launch command").success(),
            "launch command failed"
        );
        assert_eq!(
            std::fs::read_to_string(&output).expect("launch output"),
            arguments[0]
        );
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn launched_children_are_reaped_after_exit() {
        let child = spawn_launch_command("true", &[]).expect("spawn short-lived child");
        let pid = child.id();
        track_child(child).expect("hand child to reaper");

        let deadline = Instant::now() + Duration::from_secs(2);
        while process_exists(pid) {
            assert!(Instant::now() < deadline, "child {pid} was not reaped");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn launch_path_rejects_unmatched_quotes_before_spawning() {
        let error = spawn_launch_command("sh -c 'unterminated", &[])
            .expect_err("unmatched quote must fail");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn proc_stat_parser_handles_spaces_and_parentheses_in_process_names() {
        let stat = format!(
            "123 (worker ) with spaces) S {} 4242",
            vec!["0"; 18].join(" ")
        );
        assert_eq!(
            parse_proc_stat(&stat).expect("parse process stat"),
            ProcessObservation {
                state: 'S',
                start_time: 4242,
            }
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pidfd_sigkill_reaches_a_terminal_process_state() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep process");
        let pid = child.id() as i32;
        assert!(
            super::terminate_process(pid).expect("terminate process through pidfd"),
            "SIGKILLed process remained live"
        );
        let _ = child.wait();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn observation_failure_still_hands_child_to_reaper() {
        let child = spawn_launch_command("true", &[]).expect("spawn short-lived child");
        let pid = child.id();
        let error = super::track_launched_child_with(child, |_| {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "simulated /proc restriction",
            ))
        })
        .expect_err("observation failure must be returned");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);

        let deadline = Instant::now() + Duration::from_secs(2);
        while process_exists(pid) {
            assert!(
                Instant::now() < deadline,
                "child {pid} was not reaped after observation failure"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn shell_quote(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }

    fn process_exists(pid: u32) -> bool {
        std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "state="])
            .output()
            .is_ok_and(|output| output.status.success() && !output.stdout.is_empty())
    }
}
