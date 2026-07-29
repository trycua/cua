use std::io;
use std::process::{Child, Command, Stdio};
use std::sync::{
    mpsc::{self, Receiver, RecvTimeoutError, Sender},
    OnceLock,
};
use std::time::Duration;

const REAP_INTERVAL: Duration = Duration::from_millis(50);

pub(super) fn spawn_shell_command(command: &str, additional_args: &[String]) -> io::Result<Child> {
    let mut launch = Command::new("sh");
    launch
        .arg("-c")
        // `exec` keeps the returned pid bound to the launched application for
        // ordinary desktop-entry commands. `"$@"` appends caller-provided
        // arguments without interpolating them into the shell program.
        .arg(format!("exec {command} \"$@\""))
        .arg("cua-driver-launch")
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

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use super::{spawn_shell_command, track_child};

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
        let command = format!(
            "printf '%s' 'quoted value' > {}",
            shell_quote(output_path.to_str().expect("UTF-8 temp path"))
        );
        let mut child = spawn_shell_command(&command, &[]).expect("spawn launch command");
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
        let command = format!(
            "printf '%s' > {}",
            shell_quote(output.to_str().expect("UTF-8 temp path"))
        );
        let arguments = vec!["value with spaces; $(false)".to_owned()];
        let mut child = spawn_shell_command(&command, &arguments).expect("spawn launch command");
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
        let child = spawn_shell_command("true", &[]).expect("spawn short-lived child");
        let pid = child.id();
        track_child(child).expect("hand child to reaper");

        let deadline = Instant::now() + Duration::from_secs(2);
        while process_exists(pid) {
            assert!(Instant::now() < deadline, "child {pid} was not reaped");
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
