//! Experiments: new behavior that is off until it is turned on.
//!
//! One switch per experiment, one source of truth in the cua home
//! (`<home>/experiments/<name>`, `on` or `off`). The CLI (`cua experiments`),
//! the Spaces apps and the daemon all read and write that file.
//! `CUA_EXPERIMENTS` turns experiments on (`a,b`) or off (`-a`) for one
//! process over the file.

use std::sync::atomic::{AtomicU8, Ordering};

/// Environment variable that turns experiments on or off for one process:
/// a comma or space separated list of names, `-name` for off.
pub const EXPERIMENTS_ENV: &str = "CUA_EXPERIMENTS";

/// The folder, in the cua home, that holds the switches.
pub const EXPERIMENTS_DIR: &str = "experiments";

/// An experiment: a feature that is still being built and is off by default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Gate {
    /// CI runners: GitHub Actions jobs run in a fresh Space each (`cua ci`).
    CiRunners,
    /// Public TCP shares: a raw TCP service in a Space (a game server) at a
    /// public address (`cua share --tcp`).
    TcpShares,
    /// Mobile: the Cua Spaces phone app. The daemon answers a phone's tools
    /// (starting a task, tool rows, changes and diffs, approval scopes) and
    /// reports events for its push alerts.
    Mobile,
    /// Windows on KVM: Windows Spaces from a Mac coordinator on a Cua-managed
    /// AWS x86 pool host with nested KVM.
    WindowsKvm,
}

impl Gate {
    /// Every experiment, in the order they are listed.
    pub const ALL: &'static [Gate] = &[
        Gate::CiRunners,
        Gate::TcpShares,
        Gate::Mobile,
        Gate::WindowsKvm,
    ];

    /// The stable name (`secure-spaces`): the file name, the CLI word.
    pub fn name(self) -> &'static str {
        match self {
            Gate::CiRunners => "ci-runners",
            Gate::TcpShares => "tcp-shares",
            Gate::Mobile => "mobile",
            Gate::WindowsKvm => "windows-kvm",
        }
    }

    /// The same name with underscores (`secure_spaces`): the app's id.
    pub fn id(self) -> &'static str {
        match self {
            Gate::CiRunners => "ci_runners",
            Gate::TcpShares => "tcp_shares",
            Gate::Mobile => "mobile",
            Gate::WindowsKvm => "windows_kvm",
        }
    }

    /// The experiment named `text` (dashes or underscores, any case).
    pub fn parse(text: &str) -> Option<Gate> {
        let t = text.trim().to_ascii_lowercase().replace('_', "-");
        Gate::ALL.iter().copied().find(|g| g.name() == t)
    }

    /// The label people see.
    pub fn title(self) -> &'static str {
        match self {
            Gate::CiRunners => "CI runners",
            Gate::TcpShares => "Public TCP shares",
            Gate::Mobile => "Phone app",
            Gate::WindowsKvm => "Windows on KVM",
        }
    }

    /// One line on what it does.
    pub fn summary(self) -> &'static str {
        match self {
            Gate::CiRunners => {
                "Run GitHub Actions jobs in a fresh Space each, on your computers, with `cua ci`."
            }
            Gate::TcpShares => {
                "Share a raw TCP service in a Space, such as a game server, at a public address."
            }
            Gate::Mobile => {
                "Your computers answer the Cua Spaces phone app: start a task, follow the thread, review changes and get alerts."
            }
            Gate::WindowsKvm => {
                "Windows Spaces from a Mac run on a Cua-managed cloud host with nested virtualization."
            }
        }
    }

    /// What a call says while the experiment is off.
    pub fn off_message(self) -> String {
        format!(
            "{} is an experiment that is off. Turn it on with `cua experiments enable {}` (or in the app: Settings, Advanced, Labs).",
            self.title(),
            self.name()
        )
    }

    fn slot(self) -> usize {
        Gate::ALL.iter().position(|g| *g == self).unwrap_or(0)
    }
}

/// Process-wide overrides, one per gate: 0 follows the environment and the
/// file, 1 off, 2 on.
static GATE_OVERRIDES: [AtomicU8; 4] = [
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
];

/// Turns `gate` on or off for this whole process (`None` follows the
/// environment and the file again). For tests and embedders.
pub fn set_enabled(gate: Gate, on: Option<bool>) {
    let v = match on {
        None => 0,
        Some(false) => 1,
        Some(true) => 2,
    };
    GATE_OVERRIDES[gate.slot()].store(v, Ordering::Relaxed);
}

/// Holds `gate` on or off until dropped, then follows the environment and the
/// file again. The override is process-wide and test harnesses run tests on
/// parallel threads, so holders of the same gate take turns: a test that
/// flips a gate takes one of these instead of calling [`set_enabled`].
#[must_use = "the override ends when this is dropped"]
pub struct Override {
    gate: Gate,
    _turn: std::sync::MutexGuard<'static, ()>,
}

/// One turn lock per gate (as many as [`GATE_OVERRIDES`]).
static GATE_TURNS: [std::sync::Mutex<()>; 4] = [
    std::sync::Mutex::new(()),
    std::sync::Mutex::new(()),
    std::sync::Mutex::new(()),
    std::sync::Mutex::new(()),
];

/// Waits for `gate`'s turn, then holds it `on` (see [`Override`]).
pub fn hold(gate: Gate, on: bool) -> Override {
    let turn = GATE_TURNS[gate.slot()]
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    set_enabled(gate, Some(on));
    Override { gate, _turn: turn }
}

impl Override {
    /// Switches the held gate without giving up the turn.
    pub fn set(&self, on: bool) {
        set_enabled(self.gate, Some(on));
    }
}

impl Drop for Override {
    fn drop(&mut self) {
        set_enabled(self.gate, None);
    }
}

/// What [`EXPERIMENTS_ENV`] says about `gate`: `Some(true)` when listed,
/// `Some(false)` as `-name`, `None` when it does not mention it.
pub fn env_says(value: Option<&str>, gate: Gate) -> Option<bool> {
    let mut said = None;
    for word in value?.split(|c: char| c == ',' || c.is_whitespace()) {
        let word = word.trim();
        if let Some(off) = word.strip_prefix('-') {
            if Gate::parse(off) == Some(gate) {
                said = Some(false);
            }
        } else if Gate::parse(word) == Some(gate) {
            said = Some(true);
        }
    }
    said
}

/// Whether `gate` is on in this process: the override, else
/// [`EXPERIMENTS_ENV`], else the switch in `home`, else off.
pub fn enabled_in(home: &std::path::Path, gate: Gate) -> bool {
    match GATE_OVERRIDES[gate.slot()].load(Ordering::Relaxed) {
        1 => return false,
        2 => return true,
        _ => {}
    }
    if let Some(v) = env_says(std::env::var(EXPERIMENTS_ENV).ok().as_deref(), gate) {
        return v;
    }
    switch(home, gate).unwrap_or(false)
}

/// [`enabled_in`] with the switches in [`crate::cua_home`].
pub fn enabled(gate: Gate) -> bool {
    enabled_in(&cua_home::cua_home(), gate)
}

fn switch_path(home: &std::path::Path, gate: Gate) -> std::path::PathBuf {
    home.join(EXPERIMENTS_DIR).join(gate.name())
}

/// The switch `home` keeps for `gate`; `None` when it was never set.
pub fn switch(home: &std::path::Path, gate: Gate) -> Option<bool> {
    let text = std::fs::read_to_string(switch_path(home, gate)).ok()?;
    Some(text.trim() == "on")
}

/// Switches that were experiments once and are now always on (Secure Spaces, Ask first, Child
/// Spaces, Warm start). A file
/// one of them left in `<home>/experiments` is ignored and removed by [`forget_retired`]; naming
/// one in [`EXPERIMENTS_ENV`] does nothing.
pub const RETIRED: &[&str] = &["secure-spaces", "ask-first", "child-spaces", "warm-pools"];

/// Removes the switch files of the [`RETIRED`] experiments from `home`, so a person who once
/// turned one off keeps nothing of it. Missing files are fine; a test never touches the user's
/// real home.
pub fn forget_retired(home: &std::path::Path) {
    for name in RETIRED {
        let path = home.join(EXPERIMENTS_DIR).join(name);
        if path.exists() && cua_home::guard_write(&path).is_ok() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Keeps the switch for `gate` in `home`. Writes only a change: a switch
/// that was never set and stays off writes nothing. Also clears the files of retired experiments.
pub fn set_switch(home: &std::path::Path, gate: Gate, on: bool) -> std::io::Result<()> {
    forget_retired(home);
    let now = switch(home, gate);
    if now == Some(on) || (now.is_none() && !on) {
        return Ok(());
    }
    let path = switch_path(home, gate);
    // A test never writes the user's real cua home.
    cua_home::guard_write(&path)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, if on { "on\n" } else { "off\n" })?;
    std::fs::rename(&tmp, &path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_gate_is_off_until_set_and_follows_the_file() {
        for g in Gate::ALL.iter().copied() {
            let home = tempfile::tempdir().unwrap();
            assert_eq!(super::switch(home.path(), g), None);
            assert!(!super::enabled_in(home.path(), g), "{g:?}");
            // Off and never set: nothing is written.
            super::set_switch(home.path(), g, false).unwrap();
            assert!(!home.path().join("experiments").join(g.name()).exists());
            assert_eq!(super::switch(home.path(), g), None);
            super::set_switch(home.path(), g, true).unwrap();
            assert_eq!(super::switch(home.path(), g), Some(true));
            assert!(super::enabled_in(home.path(), g));
            super::set_switch(home.path(), g, false).unwrap();
            assert_eq!(super::switch(home.path(), g), Some(false));
        }
    }

    #[test]
    fn the_listing_has_no_retired_experiment_and_all_the_others() {
        let names: Vec<_> = Gate::ALL.iter().map(|g| g.name()).collect();
        assert_eq!(names, ["ci-runners", "tcp-shares", "mobile", "windows-kvm"]);
        for retired in super::RETIRED {
            assert_eq!(Gate::parse(retired), None, "{retired}");
            assert_eq!(Gate::parse(&retired.replace('-', "_")), None, "{retired}");
        }
    }

    #[test]
    fn a_switch_a_retired_experiment_left_is_ignored_and_removed_on_write() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("experiments");
        std::fs::create_dir_all(&dir).unwrap();
        for name in super::RETIRED {
            std::fs::write(dir.join(name), "off\n").unwrap();
        }
        std::fs::write(dir.join("mobile"), "on\n").unwrap();
        // Ignored: the other switches read as before.
        assert!(super::enabled_in(home.path(), Gate::Mobile));
        // Removed on the next write; the others stay.
        super::set_switch(home.path(), Gate::CiRunners, true).unwrap();
        for name in super::RETIRED {
            assert!(!dir.join(name).exists(), "{name}");
        }
        assert!(dir.join("mobile").exists());
    }

    #[test]
    fn windows_kvm_is_a_named_experiment_and_off_by_default() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(Gate::parse("windows_kvm"), Some(Gate::WindowsKvm));
        assert_eq!(Gate::WindowsKvm.name(), "windows-kvm");
        assert!(Gate::ALL.contains(&Gate::WindowsKvm));
        assert!(
            !super::enabled_in(home.path(), Gate::WindowsKvm)
                || super::env_says(
                    std::env::var(EXPERIMENTS_ENV).ok().as_deref(),
                    Gate::WindowsKvm
                ) == Some(true)
        );
    }

    #[test]
    fn the_environment_list_names_experiments_on_or_off() {
        let g = Gate::CiRunners;
        assert_eq!(super::env_says(None, g), None);
        assert_eq!(super::env_says(Some("mobile"), g), None);
        assert_eq!(super::env_says(Some("ci-runners"), g), Some(true));
        assert_eq!(super::env_says(Some("a, CI_Runners"), g), Some(true));
        assert_eq!(
            super::env_says(Some("ci-runners -ci-runners"), g),
            Some(false)
        );
        assert_eq!(super::env_says(Some("-ci_runners"), g), Some(false));
        assert_eq!(Gate::parse("CI_RUNNERS"), Some(g));
        assert_eq!(Gate::parse("nope"), None);
        assert_eq!(Gate::parse("Mobile"), Some(Gate::Mobile));
        assert_eq!(Gate::Mobile.id(), "mobile");
        // A retired experiment named in the environment does nothing.
        for retired in super::RETIRED {
            assert_eq!(super::env_says(Some(&format!("-{retired}")), g), None);
        }
    }
}
