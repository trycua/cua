//! Wayland presentation-timestamp latency fixture for cua-driver.
//!
//! The fixture owns one `wl_surface` and requests feedback for each content
//! commit by fixture update ID. The active action makes one update; the
//! supersede control deliberately makes two. It records, in one clock:
//!
//! 1. when the input event was received,
//! 2. when application-owned state changed,
//! 3. when the corresponding content update was committed,
//! 4. whether the compositor presented or discarded that exact update, and
//!    when.
//!
//! The Driver-side stamps (`request_started_ns`, `driver_returned_ns`) belong
//! to the runner, which joins them against these rows. Nothing here talks to
//! the Driver, and no model or provider is involved.
//!
//! Deliberately not in this fixture: percentile machinery, panel
//! click-to-photon claims, and any production telemetry surface.

// The Wayland client is the only consumer of several journal and accounting
// helpers, so a non-Linux host build (which still compiles and runs the unit
// tests) sees them as dead code.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

mod journal;
mod layout;
mod sample;

#[cfg(target_os = "linux")]
mod wayland;

use std::path::PathBuf;
use std::process::ExitCode;

/// Exit code used when this compositor cannot attribute a content update to a
/// presentation: it does not implement stable presentation-time, or it
/// advertises the protocol but completes no feedback (a headless wlroots
/// session does the latter). It is a typed environment limitation, not a
/// measurement result, so the runner can record it as such instead of reading
/// a missing row as a fast action.
pub const EXIT_NO_PRESENTATION: u8 = 3;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    pub journal: PathBuf,
    pub state: Option<PathBuf>,
    pub title: String,
    pub deadline_ms: u64,
    pub width: i32,
    pub height: i32,
    /// Exit once this many content updates have been accounted for. Zero runs
    /// until the compositor closes the toplevel.
    pub exit_after: u64,
    /// Map one content update, wait for feedback, and exit. Lets a runner record
    /// a typed environment limitation instead of reading a missing measurement
    /// as a fast action.
    pub probe: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            journal: PathBuf::new(),
            state: None,
            // The canonical Sway lane floats and resizes windows by this title
            // prefix; keep it stable.
            title: "CuaTestHarness Presentation".to_owned(),
            deadline_ms: 1_000,
            width: 800,
            height: 600,
            exit_after: 0,
            probe: false,
        }
    }
}

const USAGE: &str = "\
Usage: cua-harness-wayland-presentation --journal <path> [options]

  --journal <path>     JSONL evidence file; must not already exist (required)
  --state <path>       application-owned state file, replaced atomically
  --title <text>       toplevel title prefix (default: CuaTestHarness Presentation)
  --deadline-ms <ms>   presentation feedback deadline (default: 1000)
  --width <px>         initial surface width (default: 800)
  --height <px>        initial surface height (default: 600)
  --exit-after <n>     exit after n accounted content updates (default: 0 = never)
  --probe              commit one update, report feedback support, and exit
";

impl Config {
    pub fn from_args<I: Iterator<Item = String>>(mut args: I) -> Result<Self, String> {
        let mut config = Self::default();
        let mut journal_seen = false;
        while let Some(argument) = args.next() {
            let mut value = |name: &str| -> Result<String, String> {
                args.next()
                    .ok_or_else(|| format!("{name} requires a value"))
            };
            match argument.as_str() {
                "--journal" => {
                    config.journal = PathBuf::from(value("--journal")?);
                    journal_seen = true;
                }
                "--state" => config.state = Some(PathBuf::from(value("--state")?)),
                "--title" => config.title = value("--title")?,
                "--deadline-ms" => {
                    config.deadline_ms = parse_number(&value("--deadline-ms")?, "--deadline-ms")?;
                }
                "--width" => config.width = parse_number(&value("--width")?, "--width")?,
                "--height" => config.height = parse_number(&value("--height")?, "--height")?,
                "--exit-after" => {
                    config.exit_after = parse_number(&value("--exit-after")?, "--exit-after")?;
                }
                "--probe" => config.probe = true,
                "-h" | "--help" => return Err(USAGE.to_owned()),
                other => return Err(format!("unknown argument: {other}")),
            }
        }
        if !journal_seen {
            return Err("--journal is required".to_owned());
        }
        if config.deadline_ms == 0 {
            return Err("--deadline-ms must be greater than zero".to_owned());
        }
        if config.width <= 0 || config.height <= 0 {
            return Err("--width and --height must be positive".to_owned());
        }
        Ok(config)
    }

    pub fn deadline_ns(&self) -> u64 {
        self.deadline_ms.saturating_mul(1_000_000)
    }
}

fn parse_number<T: std::str::FromStr>(value: &str, name: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("{name} expects a number, got {value:?}"))
}

fn main() -> ExitCode {
    let config = match Config::from_args(std::env::args().skip(1)) {
        Ok(config) => config,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    #[cfg(target_os = "linux")]
    {
        match wayland::run(&config) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error @ wayland::RunError::NoPresentationSupport)
            | Err(error @ wayland::RunError::NoPresentationFeedback) => {
                eprintln!("{error}; recorded as an environment limitation");
                ExitCode::from(EXIT_NO_PRESENTATION)
            }
            Err(error) => {
                eprintln!("wayland presentation fixture failed: {error}");
                ExitCode::FAILURE
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = &config;
        eprintln!("this fixture requires a Wayland session on Linux");
        ExitCode::from(2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Config, String> {
        Config::from_args(args.iter().map(|value| (*value).to_owned()))
    }

    #[test]
    fn a_journal_path_is_required() {
        let error = parse(&["--state", "/tmp/state.json"]).expect_err("journal is required");
        assert!(error.contains("--journal"), "{error}");
    }

    #[test]
    fn defaults_match_the_canonical_lane_expectations() {
        let config = parse(&["--journal", "/tmp/journal.jsonl"]).expect("parse");
        assert_eq!(config.journal, PathBuf::from("/tmp/journal.jsonl"));
        assert_eq!(config.state, None);
        assert_eq!(config.title, "CuaTestHarness Presentation");
        assert_eq!(config.deadline_ms, 1_000);
        assert_eq!(config.deadline_ns(), 1_000_000_000);
        assert_eq!((config.width, config.height), (800, 600));
        assert_eq!(config.exit_after, 0);
        assert!(!config.probe);
    }

    #[test]
    fn probe_mode_is_opt_in() {
        let config = parse(&["--journal", "/tmp/journal.jsonl", "--probe"]).expect("parse");
        assert!(config.probe);
    }

    #[test]
    fn every_option_is_accepted() {
        let config = parse(&[
            "--journal",
            "/tmp/j.jsonl",
            "--state",
            "/tmp/s.json",
            "--title",
            "CuaTestHarness Presentation Alt",
            "--deadline-ms",
            "250",
            "--width",
            "940",
            "--height",
            "780",
            "--exit-after",
            "6",
        ])
        .expect("parse");
        assert_eq!(config.state, Some(PathBuf::from("/tmp/s.json")));
        assert_eq!(config.title, "CuaTestHarness Presentation Alt");
        assert_eq!(config.deadline_ns(), 250_000_000);
        assert_eq!((config.width, config.height), (940, 780));
        assert_eq!(config.exit_after, 6);
    }

    #[test]
    fn malformed_input_is_refused_instead_of_defaulted() {
        for args in [
            vec!["--journal"],
            vec!["--journal", "/tmp/j.jsonl", "--deadline-ms", "soon"],
            vec!["--journal", "/tmp/j.jsonl", "--deadline-ms", "0"],
            vec!["--journal", "/tmp/j.jsonl", "--width", "0"],
            vec!["--journal", "/tmp/j.jsonl", "--height", "-2"],
            vec!["--journal", "/tmp/j.jsonl", "--counter"],
        ] {
            assert!(parse(&args).is_err(), "{args:?} must be refused");
        }
    }
}
