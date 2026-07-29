use std::io;
use std::process::{Child, Command};

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
    launch.spawn()
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use super::spawn_shell_command;

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

    fn shell_quote(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}
