//! The service that keeps `cua ci serve --pool NAME` running: a
//! launchd agent in your login session on macOS
//! (`~/Library/LaunchAgents/ai.cua.runner.<pool>.plist`; it can read the
//! login keychain and the Spaces your account sees), a systemd user unit on
//! Linux (`~/.config/systemd/user/cua-ci-<pool>.service`).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{Error, Result};

/// What the service runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceSpec {
    /// The pool.
    pub pool: String,
    /// The `cua` binary.
    pub exe: PathBuf,
    /// `$CUA_HOME` to pass on, when set.
    pub cua_home: Option<PathBuf>,
    /// Where its output goes.
    pub log: PathBuf,
}

/// The launchd label of a pool's agent.
pub fn launchd_label(pool: &str) -> String {
    format!("ai.cua.runner.{pool}")
}

/// The systemd unit name of a pool's service.
pub fn systemd_unit_name(pool: &str) -> String {
    format!("cua-ci-{pool}.service")
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The launchd agent's plist.
pub fn launchd_plist(spec: &ServiceSpec) -> String {
    let env = spec
        .cua_home
        .as_ref()
        .map(|h| {
            format!(
                "  <key>EnvironmentVariables</key>\n  <dict>\n    \
                 <key>CUA_HOME</key><string>{}</string>\n  </dict>\n",
                xml(&h.to_string_lossy())
            )
        })
        .unwrap_or_default();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string>
    <string>runner</string>
    <string>serve</string>
    <string>--pool</string>
    <string>{pool}</string>
  </array>
{env}  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ThrottleInterval</key><integer>30</integer>
  <key>ProcessType</key><string>Background</string>
  <key>StandardOutPath</key><string>{log}</string>
  <key>StandardErrorPath</key><string>{log}</string>
</dict>
</plist>
"#,
        label = xml(&launchd_label(&spec.pool)),
        exe = xml(&spec.exe.to_string_lossy()),
        pool = xml(&spec.pool),
        log = xml(&spec.log.to_string_lossy()),
    )
}

fn systemd_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The systemd user unit.
pub fn systemd_unit(spec: &ServiceSpec) -> String {
    let env = spec
        .cua_home
        .as_ref()
        .map(|h| {
            format!(
                "Environment={}\n",
                systemd_quote(&format!("CUA_HOME={}", h.display()))
            )
        })
        .unwrap_or_default();
    format!(
        "[Unit]\nDescription=Cua CI: GitHub Actions jobs on Cua Spaces (pool {pool})\n\
         After=network-online.target\nWants=network-online.target\n\n\
         [Service]\nType=simple\nExecStart={exe} runner serve --pool {pool}\n{env}\
         Restart=always\nRestartSec=30\nStandardOutput=append:{log}\nStandardError=append:{log}\n\n\
         [Install]\nWantedBy=default.target\n",
        pool = spec.pool,
        exe = systemd_quote(&spec.exe.to_string_lossy()),
        log = spec.log.display(),
    )
}

/// Where the service file of `pool` goes under `home` (the user's home).
pub fn service_path(home: &Path, pool: &str) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/LaunchAgents")
            .join(format!("{}.plist", launchd_label(pool)))
    } else {
        home.join(".config/systemd/user")
            .join(systemd_unit_name(pool))
    }
}

fn user_home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| Error::Config("HOME is not set".into()))
}

/// Refuses in a test process: tests never install services on the host.
fn guard() -> Result<()> {
    if cua_home::is_test_process() {
        return Err(Error::Config(
            "refusing to install or change a service from a test".into(),
        ));
    }
    Ok(())
}

fn run(cmd: &mut Command) -> Result<String> {
    let out = cmd
        .output()
        .map_err(|e| Error::Config(format!("{cmd:?}: {e}")))?;
    if !out.status.success() {
        return Err(Error::Config(format!(
            "{:?} failed: {}",
            cmd.get_program(),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(target_os = "macos")]
fn gui_domain() -> String {
    // SAFETY: getuid has no preconditions.
    format!("gui/{}", unsafe { libc::getuid() })
}

/// Writes the service file and starts it (replacing an earlier one).
/// Returns the file's path.
pub fn install(spec: &ServiceSpec) -> Result<PathBuf> {
    guard()?;
    let path = service_path(&user_home()?, &spec.pool);
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    if let Some(d) = spec.log.parent() {
        std::fs::create_dir_all(d)?;
    }
    #[cfg(target_os = "macos")]
    {
        let _ = stop(&spec.pool);
        std::fs::write(&path, launchd_plist(spec))?;
        run(Command::new("/bin/launchctl")
            .args(["bootstrap", &gui_domain()])
            .arg(&path))?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::fs::write(&path, systemd_unit(spec))?;
        run(Command::new("systemctl").args(["--user", "daemon-reload"]))?;
        run(Command::new("systemctl").args([
            "--user",
            "enable",
            "--now",
            &systemd_unit_name(&spec.pool),
        ]))?;
    }
    Ok(path)
}

/// Stops the service (keeps the file). `false` when it was not running.
pub fn stop(pool: &str) -> Result<bool> {
    guard()?;
    #[cfg(target_os = "macos")]
    {
        let target = format!("{}/{}", gui_domain(), launchd_label(pool));
        Ok(run(Command::new("/bin/launchctl").args(["bootout", &target])).is_ok())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(
            run(Command::new("systemctl").args(["--user", "stop", &systemd_unit_name(pool)]))
                .is_ok(),
        )
    }
}

/// Starts the installed service again.
pub fn start(pool: &str) -> Result<()> {
    guard()?;
    let path = service_path(&user_home()?, pool);
    if !path.exists() {
        return Err(Error::Config(format!(
            "pool {pool} has no service ({}); run `cua ci connect --own-app` again or use `cua ci \
             serve --pool {pool}`",
            path.display()
        )));
    }
    #[cfg(target_os = "macos")]
    {
        let _ = stop(pool);
        run(Command::new("/bin/launchctl")
            .args(["bootstrap", &gui_domain()])
            .arg(&path))?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        run(Command::new("systemctl").args(["--user", "start", &systemd_unit_name(pool)]))?;
    }
    Ok(())
}

/// Whether the service's file is installed for this user.
pub fn installed(pool: &str) -> bool {
    user_home().is_ok_and(|h| service_path(&h, pool).exists())
}

/// Whether the service runs now.
pub fn is_running(pool: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        let target = format!("{}/{}", gui_domain(), launchd_label(pool));
        Command::new("/bin/launchctl")
            .args(["print", &target])
            .output()
            .map(|o| {
                o.status.success() && String::from_utf8_lossy(&o.stdout).contains("state = running")
            })
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Command::new("systemctl")
            .args(["--user", "is-active", "--quiet", &systemd_unit_name(pool)])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

/// Stops the service and removes its file. `false` when there was none.
pub fn uninstall(pool: &str) -> Result<bool> {
    guard()?;
    let path = service_path(&user_home()?, pool);
    let _ = stop(pool);
    #[cfg(not(target_os = "macos"))]
    {
        let _ =
            run(Command::new("systemctl").args(["--user", "disable", &systemd_unit_name(pool)]));
    }
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> ServiceSpec {
        ServiceSpec {
            pool: "trycua".into(),
            exe: "/Applications/Cua Spaces.app/Contents/Resources/native/cua".into(),
            cua_home: Some("/Users/me/.cua".into()),
            log: "/Users/me/.cua/ci/trycua/serve.log".into(),
        }
    }

    #[test]
    fn launchd_agent_runs_serve_for_the_pool() {
        let p = launchd_plist(&spec());
        assert!(p.contains("<key>Label</key><string>ai.cua.runner.trycua</string>"));
        assert!(p.contains(
            "<string>/Applications/Cua Spaces.app/Contents/Resources/native/cua</string>\n    \
             <string>runner</string>\n    <string>serve</string>\n    <string>--pool</string>\n    \
             <string>trycua</string>"
        ));
        assert!(p.contains("<key>CUA_HOME</key><string>/Users/me/.cua</string>"));
        assert!(p.contains("<key>KeepAlive</key><true/>"));
        let mut no_home = spec();
        no_home.cua_home = None;
        assert!(!launchd_plist(&no_home).contains("EnvironmentVariables"));
    }

    #[test]
    fn systemd_unit_runs_serve_for_the_pool() {
        let u = systemd_unit(&spec());
        assert!(u.contains(
            "ExecStart=\"/Applications/Cua Spaces.app/Contents/Resources/native/cua\" runner \
             serve --pool trycua"
        ));
        assert!(u.contains("Environment=\"CUA_HOME=/Users/me/.cua\""));
        assert!(u.contains("Restart=always"));
        assert!(u.contains("WantedBy=default.target"));
    }

    #[test]
    fn service_files_live_in_the_users_home() {
        let p = service_path(Path::new("/home/me"), "trycua");
        if cfg!(target_os = "macos") {
            assert_eq!(
                p,
                Path::new("/home/me/Library/LaunchAgents/ai.cua.runner.trycua.plist")
            );
        } else {
            assert_eq!(
                p,
                Path::new("/home/me/.config/systemd/user/cua-ci-trycua.service")
            );
        }
    }

    #[test]
    fn tests_never_install_a_service() {
        let e = install(&spec()).unwrap_err().to_string();
        assert!(e.contains("refusing"), "{e}");
        assert!(stop("trycua").is_err() && uninstall("trycua").is_err());
    }
}
