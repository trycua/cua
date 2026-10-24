//! Fault injection. A [`FaultTable`] maps a fault name of a plan to the
//! shell command that causes it and the one that undoes it, with `{arg}`
//! placeholders filled from the plan step. The table is data, so each
//! client (a gateway, the daemon, a Space on a pool host) supplies its own
//! commands; [`FaultTable::host_defaults`] has the generic Linux ones.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How a fault is applied and undone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaultCmd {
    /// A `sh -c` script; `{name}` is replaced by the step's argument `name`.
    pub apply: String,
    /// Run when the run ends (or by a heal step), newest first. Empty: none.
    #[serde(default)]
    pub undo: String,
}

/// Fault name -> commands.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FaultTable {
    pub faults: BTreeMap<String, FaultCmd>,
}

/// A fault that was applied and may need undoing.
#[derive(Debug, Clone)]
pub struct Applied {
    pub name: String,
    pub undo: Option<String>,
}

fn fill(tpl: &str, args: &BTreeMap<String, serde_json::Value>) -> String {
    let mut s = tpl.to_string();
    for (k, v) in args {
        let val = match v {
            serde_json::Value::String(x) => x.clone(),
            o => o.to_string(),
        };
        // Arguments come from a plan we generated; keep them shell-inert.
        let safe: String = val
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || "-_./:=,@".contains(*c))
            .collect();
        s = s.replace(&format!("{{{k}}}"), &safe);
    }
    s
}

/// The KILL RULE: a chaos script may signal only a PID it recorded when it
/// started the target (`kill -KILL "$(cat "$CHAOS_ROOT/x.pid")"`). Pattern
/// kills (`pkill`, `killall`, `pgrep`) and process-group or all-process
/// signals (`kill 0`, `kill -1`) are refused, so a typo in a pattern can never
/// signal a process the run did not start. (A broad `pkill -f x` once
/// terminated most of a developer laptop's processes.)
pub fn lint_script(script: &str) -> Result<(), String> {
    for word in ["pkill", "killall", "pgrep", "skill "] {
        if script.contains(word) {
            return Err(format!(
                "refusing '{word}' in a chaos script: signal recorded PIDs only"
            ));
        }
    }
    let toks: Vec<&str> = script
        .split(|c: char| c.is_whitespace() || ";&|()".contains(c))
        .filter(|t| !t.is_empty())
        .collect();
    for (i, t) in toks.iter().enumerate() {
        if *t != "kill" {
            continue;
        }
        // The targets after the signal option.
        for (j, a) in toks[i + 1..].iter().take(3).enumerate() {
            let a = a.trim_matches('"');
            // `kill -0 PID` and `kill -KILL PID`: the first word is the signal.
            if j == 0
                && (a == "-0"
                    || a.starts_with("-") && a[1..].starts_with(|c: char| c.is_ascii_alphabetic()))
            {
                continue;
            }
            if matches!(a, "0" | "-0" | "-1" | "1") {
                return Err(format!("refusing 'kill {a}': broad signal"));
            }
        }
    }
    Ok(())
}

/// Runs a script with a deadline; returns (success, combined output).
pub fn run_sh(script: &str, env: &[(String, String)], timeout: Duration) -> (bool, String) {
    let mut c = Command::new("sh");
    c.arg("-c").arg(script);
    for (k, v) in env {
        c.env(k, v);
    }
    c.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = match c.spawn() {
        Ok(c) => c,
        Err(e) => return (false, e.to_string()),
    };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                let out = child.wait_with_output().ok();
                let text = out
                    .map(|o| {
                        format!(
                            "{}{}",
                            String::from_utf8_lossy(&o.stdout),
                            String::from_utf8_lossy(&o.stderr)
                        )
                    })
                    .unwrap_or_default();
                return (st.success(), text);
            }
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return (false, "timed out".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return (false, e.to_string()),
        }
    }
}

impl FaultTable {
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }

    /// Generic Linux faults. `$CHAOS_DIR` holds pidfiles written by the
    /// scenario (`gateway.pid`, `daemon.pid`) and start scripts
    /// (`gateway.start`, `daemon.start`); nothing here matches processes by
    /// name, so a campaign only ever kills what it started.
    pub fn host_defaults() -> Self {
        let mut f = BTreeMap::new();
        let mut add = |n: &str, a: &str, u: &str| {
            f.insert(
                n.to_string(),
                FaultCmd {
                    apply: a.into(),
                    undo: u.into(),
                },
            );
        };
        add(
            "gateway_kill",
            "kill -KILL \"$(cat \"$CHAOS_DIR/gateway.pid\")\"",
            "",
        );
        add("gateway_start", "sh \"$CHAOS_DIR/gateway.start\"", "");
        add(
            "daemon_kill",
            "kill -KILL \"$(cat \"$CHAOS_DIR/daemon.pid\")\"",
            "",
        );
        add("daemon_start", "sh \"$CHAOS_DIR/daemon.start\"", "");
        add(
            "partition",
            "sudo -n iptables -I OUTPUT -p tcp --dport {port} -m comment --comment cua-chaos -j DROP",
            "sudo -n iptables -D OUTPUT -p tcp --dport {port} -m comment --comment cua-chaos -j DROP",
        );
        add(
            "partition_heal",
            "sudo -n iptables -D OUTPUT -p tcp --dport {port} -m comment --comment cua-chaos -j DROP",
            "",
        );
        f.into()
    }
}

impl From<BTreeMap<String, FaultCmd>> for FaultTable {
    fn from(faults: BTreeMap<String, FaultCmd>) -> Self {
        FaultTable { faults }
    }
}

impl FaultTable {
    /// Applies the fault `name`; `Err` carries the command output.
    pub fn apply(
        &self,
        name: &str,
        args: &BTreeMap<String, serde_json::Value>,
        env: &[(String, String)],
    ) -> Result<Applied, String> {
        let Some(cmd) = self.faults.get(name) else {
            return Err(format!("no command for fault '{name}'"));
        };
        let script = fill(&cmd.apply, args);
        lint_script(&script)?;
        lint_script(&cmd.undo)?;
        let (ok, out) = run_sh(&script, env, Duration::from_secs(120));
        if !ok {
            return Err(format!("fault '{name}' failed: {}", out.trim()));
        }
        Ok(Applied {
            name: name.into(),
            undo: (!cmd.undo.is_empty()).then(|| fill(&cmd.undo, args)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_filled_and_shell_inert() {
        let mut a = BTreeMap::new();
        a.insert("port".into(), serde_json::json!(8080));
        a.insert("path".into(), serde_json::json!("/x; rm -rf /"));
        assert_eq!(fill("p={port} q={path}", &a), "p=8080 q=/xrm-rf/");
    }

    #[test]
    fn apply_runs_the_command_and_remembers_the_undo() {
        let mut t = FaultTable::default();
        t.faults.insert(
            "touch".into(),
            FaultCmd {
                apply: "test \"$X\" = 1".into(),
                undo: "echo undo {n}".into(),
            },
        );
        let mut args = BTreeMap::new();
        args.insert("n".into(), serde_json::json!(3));
        let a = t
            .apply("touch", &args, &[("X".into(), "1".into())])
            .unwrap();
        assert_eq!(a.undo.as_deref(), Some("echo undo 3"));
        assert!(t.apply("touch", &args, &[]).is_err());
        assert!(t.apply("nope", &args, &[]).is_err());
    }

    #[test]
    fn the_kill_rule_refuses_pattern_and_broad_kills() {
        for bad in [
            "pkill -f x",
            "killall cua",
            "kill $(pgrep cua)",
            "kill -9 -1",
            "kill -KILL 0",
            "kill -KILL \"-1\"",
            "echo a; kill 1",
        ] {
            assert!(lint_script(bad).is_err(), "{bad}");
        }
        for good in [
            "kill -KILL \"$(cat \"$CHAOS_ROOT/gateway.pid\")\" || true",
            "kill -0 \"$pid\"",
            "rm -f x",
        ] {
            assert!(lint_script(good).is_ok(), "{good}");
        }
        let mut t = FaultTable::default();
        t.faults.insert(
            "x".into(),
            FaultCmd {
                apply: "pkill -f x".into(),
                undo: String::new(),
            },
        );
        assert!(
            t.apply("x", &BTreeMap::new(), &[])
                .unwrap_err()
                .contains("refusing")
        );
    }

    #[test]
    fn a_hung_command_times_out() {
        let (ok, out) = run_sh("sleep 5", &[], Duration::from_millis(200));
        assert!(!ok);
        assert_eq!(out, "timed out");
    }
}
