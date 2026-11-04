//! Guest users: providers run commands and file operations as a named
//! user (E2B's `root` and `user`, Daytona's `daytona`). cua-spacesd switches
//! users itself when it runs as root; when it runs as an ordinary user (the
//! Linux image's `cua`), the switch goes through `sudo -n -u <user>` in the
//! guest, which that user may use without a password.

use cua_spacesd_client::{Command, SpacesdClient};
use std::collections::HashMap;
use tokio::sync::Mutex;

/// The user each sandbox's cua-spacesd runs as, by sandbox id.
#[derive(Default)]
pub struct Drivers(Mutex<HashMap<String, String>>);

impl Drivers {
    /// The user `c`'s cua-spacesd runs as (`id -un`; cached per sandbox).
    pub async fn of(&self, id: &str, c: &SpacesdClient) -> String {
        if let Some(u) = self.0.lock().await.get(id) {
            return u.clone();
        }
        // An agent that switches users itself (`process.run_as` mode `root`
        // or `capabilities`, which also covers gVisor where `sudo` cannot
        // work) is as good as root: no `sudo` anywhere.
        if native_run_as(c).await {
            self.0.lock().await.insert(id.to_string(), "root".into());
            return "root".into();
        }
        let user = match c.run(Command::new("id").arg("-un")).await {
            Ok(o) if o.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            _ => "root".to_string(),
        };
        self.0.lock().await.insert(id.to_string(), user.clone());
        user
    }

    pub async fn forget(&self, id: &str) {
        self.0.lock().await.remove(id);
    }
}

/// Whether the guest agent switches users itself (feature `process.run_as`,
/// attribute `mode` = `root` | `capabilities`); older agents have no such
/// feature and get the `sudo` path.
pub async fn native_run_as(c: &SpacesdClient) -> bool {
    match c.capabilities().await {
        Ok(caps) => run_as_is_native(&caps.features),
        Err(_) => false,
    }
}

fn run_as_is_native(features: &[cua_spacesd_client::pb::Feature]) -> bool {
    features.iter().any(|f| {
        f.name == "process.run_as"
            && f.supported
            && matches!(
                f.attributes.get("mode").map(String::as_str),
                Some("root" | "capabilities")
            )
    })
}

/// How to run `command args` as `user` when cua-spacesd runs as `driver`:
/// the program, its arguments and the `ProcessConfig.user` to send.
pub fn as_user(
    driver: &str,
    user: &str,
    command: String,
    args: Vec<String>,
) -> (String, Vec<String>, String) {
    if user.is_empty() || user == driver {
        return (command, args, String::new());
    }
    if driver == "root" {
        return (command, args, user.to_string());
    }
    let mut a = vec![
        "-n".to_string(),
        "-H".to_string(),
        "-u".to_string(),
        user.to_string(),
        "--preserve-env".to_string(),
        "--".to_string(),
        command,
    ];
    a.extend(args);
    ("/usr/bin/sudo".to_string(), a, String::new())
}

/// [`as_user`] with a working directory: when the switch goes through
/// `sudo`, the directory is entered after it (the driver may not be allowed
/// into another user's home), so the cwd for cua-spacesd is empty.
pub fn as_user_in(
    driver: &str,
    user: &str,
    command: String,
    args: Vec<String>,
    cwd: String,
) -> (String, Vec<String>, String, String) {
    if user.is_empty() || user == driver || driver == "root" {
        let (c, a, u) = as_user(driver, user, command, args);
        return (c, a, u, cwd);
    }
    // Through sudo the driver's own directory may be closed to `user`:
    // always enter one it can use.
    let cwd = if cwd.is_empty() { "/".to_string() } else { cwd };
    let mut inner = vec![
        "-c".to_string(),
        "cd -- \"$0\" || exit 1; exec \"$@\"".to_string(),
        cwd,
        command,
    ];
    inner.extend(args);
    let (c, a, u) = as_user(driver, user, "/bin/sh".into(), inner);
    (c, a, u, String::new())
}

/// The command a process was started with, and the directory it was
/// given, without the user switch [`as_user_in`] added
/// (`sudo ... -- [/bin/sh -c 'cd ...' DIR] CMD ARGS`).
pub fn unwrap_switch(command: &str, args: &[String]) -> (String, Vec<String>, Option<String>) {
    if command != "sudo" && command != "/usr/bin/sudo" {
        return (command.to_string(), args.to_vec(), None);
    }
    let Some(i) = args.iter().position(|a| a == "--") else {
        return (command.to_string(), args.to_vec(), None);
    };
    let rest = &args[i + 1..];
    if rest.len() >= 5 && rest[0] == "/bin/sh" && rest[1] == "-c" && rest[2].starts_with("cd -- ") {
        return (rest[4].clone(), rest[5..].to_vec(), Some(rest[3].clone()));
    }
    match rest.split_first() {
        Some((c, a)) => (c.clone(), a.to_vec(), None),
        None => (command.to_string(), args.to_vec(), None),
    }
}

/// A short command as `user` (`sh -c script` with arguments), its output.
pub async fn run_as(
    c: &SpacesdClient,
    driver: &str,
    user: &str,
    script: &str,
    args: &[&str],
) -> Result<cua_spacesd_client::Output, String> {
    let mut a = vec!["-c".to_string(), script.to_string(), "sh".to_string()];
    a.extend(args.iter().map(|s| s.to_string()));
    let (cmd, args, u, cwd) = as_user_in(driver, user, "/bin/sh".into(), a, String::new());
    let mut command = Command::new(cmd).args(args);
    if !u.is_empty() {
        command = command.user(u);
    }
    if !cwd.is_empty() {
        command = command.cwd(cwd);
    }
    c.run(command).await.map_err(|e| e.to_string())
}

/// [`run_as`] for a script that is safe to run twice (preparing a new
/// sandbox): a transport error on a just-booted Space, where the first calls
/// of many boots at once can find the guest's agent connection reset, is
/// retried for a while instead of failing the create.
pub async fn run_as_idempotent(
    c: &SpacesdClient,
    driver: &str,
    user: &str,
    script: &str,
    args: &[&str],
) -> Result<cua_spacesd_client::Output, String> {
    let mut last = String::new();
    for attempt in 0..8u64 {
        match run_as(c, driver, user, script, args).await {
            Ok(o) => return Ok(o),
            Err(e) if e.contains("transport error") || e.contains("Aborted") => {
                last = e;
                tokio::time::sleep(std::time::Duration::from_millis(500 * (attempt + 1))).await;
            }
            Err(e) => return Err(e),
        }
    }
    Err(last)
}

/// Sets `user.e2b.<key>` extended attributes (E2B file metadata) with
/// root's rights.
pub async fn set_metadata(
    c: &SpacesdClient,
    driver: &str,
    path: &str,
    metadata: &std::collections::BTreeMap<String, String>,
) -> Result<(), String> {
    let script = "import json,os,sys\nfor k,v in json.loads(sys.argv[2]).items(): os.setxattr(sys.argv[1], 'user.e2b.'+k, v.encode())";
    let data = serde_json::to_string(metadata).expect("json");
    let out = run_as(
        c,
        driver,
        "root",
        "exec python3 -c \"$1\" \"$2\" \"$3\"",
        &[script, path, &data],
    )
    .await?;
    if out.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// The E2B metadata (`user.e2b.*` extended attributes) of `paths`, read
/// with root's rights in one command.
pub async fn get_metadata(
    c: &SpacesdClient,
    driver: &str,
    paths: &[String],
) -> HashMap<String, std::collections::BTreeMap<String, String>> {
    if paths.is_empty() {
        return HashMap::new();
    }
    let script = "import json,os,sys\nout={}\nfor p in json.loads(sys.argv[1]):\n  m={}\n  try:\n    for k in os.listxattr(p, follow_symlinks=False):\n      if k.startswith('user.e2b.'): m[k[9:]]=os.getxattr(p,k,follow_symlinks=False).decode('utf-8','replace')\n  except OSError: pass\n  if m: out[p]=m\nprint(json.dumps(out))";
    let data = serde_json::to_string(paths).expect("json");
    match run_as(
        c,
        driver,
        "root",
        "exec python3 -c \"$1\" \"$2\"",
        &[script, &data],
    )
    .await
    {
        Ok(o) if o.success() => serde_json::from_slice(&o.stdout).unwrap_or_default(),
        _ => HashMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switches_users_with_sudo_unless_the_driver_can() {
        let (c, a, u) = as_user("cua", "root", "/bin/bash".into(), vec!["-l".into()]);
        assert_eq!(c, "/usr/bin/sudo");
        assert_eq!(
            a,
            [
                "-n",
                "-H",
                "-u",
                "root",
                "--preserve-env",
                "--",
                "/bin/bash",
                "-l"
            ]
        );
        assert_eq!(u, "");
        let (c, _, u) = as_user("root", "user", "/bin/bash".into(), vec![]);
        assert_eq!((c.as_str(), u.as_str()), ("/bin/bash", "user"));
        let (c, _, u) = as_user("cua", "cua", "/bin/bash".into(), vec![]);
        assert_eq!((c.as_str(), u.as_str()), ("/bin/bash", ""));
        // The directory is entered after the switch.
        let (c, a, u, cwd) = as_user_in(
            "cua",
            "user",
            "/bin/bash".into(),
            vec!["-l".into()],
            "/home/user".into(),
        );
        assert_eq!(
            (c.as_str(), u.as_str(), cwd.as_str()),
            ("/usr/bin/sudo", "", "")
        );
        assert_eq!(
            &a[a.len() - 4..],
            [
                "cd -- \"$0\" || exit 1; exec \"$@\"",
                "/home/user",
                "/bin/bash",
                "-l"
            ]
        );
        let (_, _, _, cwd) = as_user_in(
            "root",
            "user",
            "/bin/bash".into(),
            vec![],
            "/home/user".into(),
        );
        assert_eq!(cwd, "/home/user");
        // Lists show the user's command, not the switch.
        let (c, a, _, _) = as_user_in(
            "cua",
            "user",
            "/bin/bash".into(),
            vec!["-l".into(), "-c".into(), "x".into()],
            "/tmp".into(),
        );
        assert_eq!(
            unwrap_switch(&c, &a),
            (
                "/bin/bash".to_string(),
                vec!["-l".to_string(), "-c".to_string(), "x".to_string()],
                Some("/tmp".to_string())
            )
        );
        let (c, a, _) = as_user("cua", "root", "id".into(), vec![]);
        assert_eq!(unwrap_switch(&c, &a), ("id".to_string(), vec![], None));
    }
}

#[cfg(test)]
mod run_as_tests {
    use super::*;
    use cua_spacesd_client::pb::Feature;

    fn feature(name: &str, supported: bool, mode: &str) -> Feature {
        Feature {
            name: name.into(),
            supported,
            limitation: String::new(),
            attributes: [("mode".to_string(), mode.to_string())].into(),
        }
    }

    #[test]
    fn native_modes_skip_sudo() {
        for mode in ["root", "capabilities"] {
            assert!(run_as_is_native(&[feature("process.run_as", true, mode)]));
        }
        // sudo, none, unsupported, another feature, an older agent (no feature).
        assert!(!run_as_is_native(&[feature(
            "process.run_as",
            true,
            "sudo"
        )]));
        assert!(!run_as_is_native(&[feature(
            "process.run_as",
            true,
            "none"
        )]));
        assert!(!run_as_is_native(&[feature(
            "process.run_as",
            false,
            "root"
        )]));
        assert!(!run_as_is_native(&[feature("fs.run_as", true, "root")]));
        assert!(!run_as_is_native(&[]));
    }
}
