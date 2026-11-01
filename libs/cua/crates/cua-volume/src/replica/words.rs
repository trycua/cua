// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The one place that says, in plain words, what happens when you close the
//! laptop. Shown when a durable agent starts and in the app. It uses the
//! names from the machines list and nothing else: no settings, no new
//! nouns, no promise the facts do not back.

use serde::{Deserialize, Serialize};

/// The facts the sentence is made of.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    /// This machine's name ("MacBook Pro").
    pub this_machine: String,
    /// The machine running the durable agent's driver, when known.
    pub run_on: Option<String>,
    /// Machines signed in to the account, including this one.
    pub machines: usize,
    /// Names of machines that hold a copy of the files and are up now.
    pub holders_up: Vec<String>,
    /// Names of machines that hold a copy and are away now.
    pub holders_away: Vec<String>,
    /// Changes saved only on this machine, waiting for another one.
    pub pending_here: u64,
    /// The machine the changes are waiting for, when known.
    pub pending_for: Option<String>,
    /// This machine can sleep when its lid closes.
    pub this_sleeps: bool,
    /// A sleep assertion is held here for the run.
    pub sleep_blocked: bool,
}

fn machines(n: usize) -> String {
    if n == 1 {
        "1 machine".into()
    } else {
        format!("{n} machines")
    }
}

fn list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [a] => a.clone(),
        [a, b] => format!("{a} and {b}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

/// What will happen when this machine closes, in one short paragraph.
pub fn closing_summary(s: &Summary) -> String {
    let here = s.this_machine.as_str();
    let on_other = s
        .run_on
        .as_deref()
        .filter(|r| !r.is_empty() && *r != here)
        .map(str::to_string);
    let mut out = String::new();
    match (&on_other, s.machines) {
        (Some(other), _) => {
            out.push_str(&format!("Your agent keeps running on {other}. "));
            let copies = s.holders_up.len();
            let other_holds = s.holders_up.iter().any(|h| h == other);
            if copies >= 2 {
                out.push_str(&format!(
                    "Files are on {}. You can close this machine.",
                    machines(copies)
                ));
            } else if other_holds {
                let missing = if s.holders_away.is_empty() {
                    here.to_string()
                } else {
                    list(&s.holders_away)
                };
                out.push_str(&format!(
                    "Files are on 1 machine until {missing} is back. You can close this machine; nothing is lost, there is just one copy until then."
                ));
            } else {
                out.push_str("Files are not on that machine yet; wait for them to copy before you close this machine.");
            }
        }
        (None, n) if n <= 1 => {
            out.push_str(&format!(
                "Your agent runs on {here} and will pause when it sleeps. "
            ));
            if s.sleep_blocked {
                out.push_str("It stays awake while the agent works, but closing the lid still puts a laptop to sleep unless it is plugged in with a display attached. ");
            }
            out.push_str("Add another machine and it keeps running when you close this one.");
        }
        (None, _) => {
            out.push_str(&format!("Your agent runs on {here}"));
            if s.this_sleeps {
                out.push_str(" and will pause when it sleeps. ");
            } else {
                out.push_str(". ");
            }
            if s.holders_away.is_empty() {
                out.push_str("Your other machines are not available to take over right now.");
            } else {
                out.push_str(&format!("{} is away right now.", list(&s.holders_away)));
            }
        }
    }
    if s.pending_here > 0 {
        let who = s.pending_for.as_deref().unwrap_or("another machine");
        out.push_str(&format!(
            " {} saved only on {here} will sync when {who} is back.",
            if s.pending_here == 1 {
                "1 change".to_string()
            } else {
                format!("{} changes", s.pending_here)
            }
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Summary {
        Summary {
            this_machine: "MacBook".into(),
            machines: 2,
            ..Default::default()
        }
    }

    #[test]
    fn two_machines_both_holding_files() {
        let s = Summary {
            run_on: Some("Mac Studio".into()),
            holders_up: vec!["Mac Studio".into(), "MacBook".into()],
            ..base()
        };
        assert_eq!(
            closing_summary(&s),
            "Your agent keeps running on Mac Studio. Files are on 2 machines. You can close this machine."
        );
    }

    #[test]
    fn only_the_studio_holds_files_until_the_laptop_is_back() {
        let s = Summary {
            run_on: Some("Mac Studio".into()),
            holders_up: vec!["Mac Studio".into()],
            holders_away: vec!["MacBook".into()],
            ..base()
        };
        let t = closing_summary(&s);
        assert!(t.starts_with("Your agent keeps running on Mac Studio. Files are on 1 machine until MacBook is back."), "{t}");
        assert!(!t.contains("2 machines"));
    }

    #[test]
    fn one_machine_says_it_will_pause_and_is_honest_about_the_lid() {
        let s = Summary {
            machines: 1,
            run_on: Some("MacBook".into()),
            sleep_blocked: true,
            ..base()
        };
        let t = closing_summary(&s);
        assert!(t.contains("will pause when it sleeps"), "{t}");
        assert!(
            t.contains("closing the lid still puts a laptop to sleep"),
            "{t}"
        );
        assert!(t.contains("Add another machine"), "{t}");
    }

    #[test]
    fn pending_changes_name_who_they_wait_for() {
        let s = Summary {
            run_on: Some("Mac Studio".into()),
            holders_up: vec!["Mac Studio".into(), "MacBook".into()],
            pending_here: 3,
            pending_for: Some("Mac Studio".into()),
            ..base()
        };
        let t = closing_summary(&s);
        assert!(
            t.ends_with("3 changes saved only on MacBook will sync when Mac Studio is back."),
            "{t}"
        );
    }

    #[test]
    fn the_agent_here_with_a_studio_away_does_not_promise_takeover() {
        let s = Summary {
            run_on: Some("MacBook".into()),
            this_sleeps: true,
            holders_away: vec!["Mac Studio".into()],
            ..base()
        };
        let t = closing_summary(&s);
        assert!(
            t.starts_with("Your agent runs on MacBook and will pause when it sleeps."),
            "{t}"
        );
        assert!(t.contains("Mac Studio is away right now."), "{t}");
    }
}
