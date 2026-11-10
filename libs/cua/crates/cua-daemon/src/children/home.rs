//! The agent home's abilities, as this host enforces them.
//!
//! An agent home is a Space made on a person's first ask-first request and
//! reused by the next ones (image `linux:24.04-slim`): the root of their
//! tree of Spaces. It starts with the
//! narrowest reach there is, its own children, and the person widens it one
//! ability at a time:
//!
//! - **list Spaces** ([`Ability::ListSpaces`]): the Spaces the person
//!   @mentioned by name, or every Space once the person approved listing all;
//! - **spawn a child** ([`Ability::SpawnChild`]): always on for a home (the
//!   spawn capability, inside the tree's budget); it cannot widen what a
//!   child gets;
//! - **use a machine** ([`Ability::UseMachine`]): put a child on one of the
//!   person's machines (`host:<name>`), only the ones granted.
//!
//! The grants ride on the home's record as `cua.home.*` labels, written by
//! the host when it makes the home (a grant remembered for the agent, or an
//! @mention, is known by then). Labels starting with `cua.` cannot be set
//! by a Space on a child ([`super::api::Scoped::child_request`]), so a home
//! cannot hand its own reach on or widen it.
//!
//! A grant the person gives while the home runs (list all, use a machine)
//! is written by the app to `<cua home>/agent-home/grants.json`, outside
//! the home, and read with [`read_grants`] each time the home asks, so it
//! takes effect at once ([`HomeScope::with_grants`]). Only the app, on this
//! machine, writes it: the agent in the home cannot. A Space or machine the
//! person @mentions in a later request to the same home is written there as
//! a `mentions` entry: an @mention counts as the grant, with no card.
//!
//! [`HomeScope`] is the whole decision, pure and free of the runtime, so a
//! different home (a wasm module, say) answers the same abilities with the
//! same checks: see `AgentHomeCapabilities` in the web UI's
//! `lib/agent-home` for the interface the agent sees.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Marks a Space as an agent home.
pub const LABEL_HOME: &str = "cua.home";
/// The agent (harness id) the home was made for; grants are remembered per agent.
pub const LABEL_AGENT: &str = "cua.home.agent";
/// `all` once the person approved listing every Space; absent or `mentions` otherwise.
pub const LABEL_LIST: &str = "cua.home.list";
/// The Spaces the person @mentioned (ids or names, comma separated).
pub const LABEL_MENTIONS: &str = "cua.home.mentions";
/// The machines the agent may use (ids or names, comma separated).
pub const LABEL_MACHINES: &str = "cua.home.machines";

/// The home warming for Home's box (label `cua.hidden`): no list, count or tree shows it, and an agent's "list every Space"
/// leaves it out too, whatever it was granted.
pub fn is_warming_home<S: std::hash::BuildHasher>(
    labels: &std::collections::HashMap<String, String, S>,
) -> bool {
    labels.contains_key("cua.hidden")
}

/// Where the app writes what the person approved while a home runs.
pub fn grants_path(cua_home: &Path) -> PathBuf {
    cua_home.join("agent-home").join("grants.json")
}

/// What the person approved for one home while it runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grants {
    pub list_all: bool,
    pub machines: BTreeSet<String>,
    /// Spaces and machines the person @mentioned in a request to this home
    /// after it was made (ids or names): the same as naming them when it
    /// was made. An @mention counts as the grant, so no card asks.
    pub mentions: BTreeSet<String>,
}

fn string_set(v: Option<&serde_json::Value>) -> BTreeSet<String> {
    v.and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|m| m.as_str())
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// The grants kept for `space` (none when there is no file, no entry or it
/// does not read: a home then has what its labels gave it).
pub fn read_grants(cua_home: &Path, space: &str) -> Grants {
    let Ok(text) = std::fs::read_to_string(grants_path(cua_home)) else {
        return Grants::default();
    };
    let Ok(all) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Grants::default();
    };
    let Some(entry) = all.get(space) else {
        return Grants::default();
    };
    Grants {
        list_all: entry.get("list_all").and_then(|v| v.as_bool()) == Some(true),
        machines: string_set(entry.get("machines")),
        mentions: string_set(entry.get("mentions")),
    }
}

/// What an agent home can ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ability {
    ListSpaces,
    SpawnChild,
    UseMachine,
}

/// What one agent home may reach beyond its own tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HomeScope {
    /// The Space is an agent home. Any other Space is unchanged by this module.
    pub is_home: bool,
    /// The agent that runs in it.
    pub agent: String,
    /// The person approved listing every Space.
    pub list_all: bool,
    /// Spaces named (@mentioned): listable, whatever `list_all` says.
    pub mentions: BTreeSet<String>,
    /// Machines the agent may place children on.
    pub machines: BTreeSet<String>,
}

fn words(v: Option<&String>) -> BTreeSet<String> {
    v.map(|s| {
        s.split(',')
            .map(|w| w.trim().to_owned())
            .filter(|w| !w.is_empty())
            .collect()
    })
    .unwrap_or_default()
}

impl HomeScope {
    /// Reads a Space's labels. A Space without `cua.home` is no home.
    pub fn from_labels(labels: &BTreeMap<String, String>) -> Self {
        if labels.get(LABEL_HOME).map(String::as_str) != Some("1") {
            return Self::default();
        }
        Self {
            is_home: true,
            agent: labels.get(LABEL_AGENT).cloned().unwrap_or_default(),
            list_all: labels.get(LABEL_LIST).map(String::as_str) == Some("all"),
            mentions: words(labels.get(LABEL_MENTIONS)),
            machines: words(labels.get(LABEL_MACHINES)),
        }
    }

    /// The labels a host puts on a new home (the inverse of [`Self::from_labels`]).
    pub fn to_labels(&self) -> BTreeMap<String, String> {
        let mut l = BTreeMap::new();
        if !self.is_home {
            return l;
        }
        l.insert(LABEL_HOME.into(), "1".into());
        if !self.agent.is_empty() {
            l.insert(LABEL_AGENT.into(), self.agent.clone());
        }
        if self.list_all {
            l.insert(LABEL_LIST.into(), "all".into());
        }
        if !self.mentions.is_empty() {
            l.insert(LABEL_MENTIONS.into(), join(&self.mentions));
        }
        if !self.machines.is_empty() {
            l.insert(LABEL_MACHINES.into(), join(&self.machines));
        }
        l
    }

    /// Adds what the person approved while the home runs. It only widens a
    /// home that is one.
    pub fn with_grants(mut self, g: Grants) -> Self {
        if self.is_home {
            self.list_all |= g.list_all;
            self.machines.extend(g.machines);
            self.mentions.extend(g.mentions);
        }
        self
    }

    /// May the home see this Space (outside its own tree) in a listing?
    pub fn may_list(&self, id: &str, name: &str) -> bool {
        self.is_home
            && (self.list_all || self.mentions.contains(id) || self.mentions.contains(name))
    }

    /// May the home put a child on this machine? A machine named in
    /// `machines` or @mentioned counts as granted.
    pub fn may_use_machine(&self, machine: &str) -> bool {
        self.is_home && (self.machines.contains(machine) || self.mentions.contains(machine))
    }

    /// Whether `ability` is available at all with no further question
    /// (spawning is; listing everything and machines are not).
    pub fn has(&self, ability: Ability) -> bool {
        match ability {
            Ability::SpawnChild => self.is_home,
            Ability::ListSpaces => self.is_home && self.list_all,
            Ability::UseMachine => self.is_home && !self.machines.is_empty(),
        }
    }
}

fn join(s: &BTreeSet<String>) -> String {
    s.iter().cloned().collect::<Vec<_>>().join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect()
    }

    #[test]
    fn a_space_without_the_mark_is_no_home() {
        let s = HomeScope::from_labels(&labels(&[(LABEL_LIST, "all")]));
        assert!(!s.is_home && !s.may_list("a", "a") && !s.has(Ability::SpawnChild));
    }

    #[test]
    fn a_new_home_sees_only_what_is_mentioned() {
        let s = HomeScope::from_labels(&labels(&[
            (LABEL_HOME, "1"),
            (LABEL_MENTIONS, "studio, local:ci"),
        ]));
        assert!(s.is_home);
        assert!(s.may_list("x", "studio") && s.may_list("local:ci", "whatever"));
        assert!(!s.may_list("other", "other"));
        assert!(!s.has(Ability::ListSpaces));
    }

    #[test]
    fn listing_all_is_the_persons_approval() {
        let s = HomeScope::from_labels(&labels(&[(LABEL_HOME, "1"), (LABEL_LIST, "all")]));
        assert!(s.may_list("any", "any") && s.has(Ability::ListSpaces));
    }

    #[test]
    fn a_machine_needs_a_grant_or_a_mention() {
        let s = HomeScope::from_labels(&labels(&[
            (LABEL_HOME, "1"),
            (LABEL_MACHINES, "my-mac"),
            (LABEL_MENTIONS, "spare"),
        ]));
        assert!(s.may_use_machine("my-mac") && s.may_use_machine("spare"));
        assert!(!s.may_use_machine("other"));
    }

    #[test]
    fn a_grant_given_while_it_runs_widens_a_home_and_only_a_home() {
        let home = HomeScope::from_labels(&labels(&[(LABEL_HOME, "1")]));
        let g = Grants {
            list_all: true,
            machines: ["my-mac".to_owned()].into(),
            ..Default::default()
        };
        let s = home.with_grants(g.clone());
        assert!(s.may_list("any", "any") && s.may_use_machine("my-mac"));
        let plain = HomeScope::default().with_grants(g);
        assert!(
            !plain.is_home && !plain.may_list("any", "any") && !plain.may_use_machine("my-mac")
        );
    }

    #[test]
    fn the_grants_file_is_read_per_space_and_a_bad_one_grants_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_grants(dir.path(), "local:h"), Grants::default());
        std::fs::create_dir_all(grants_path(dir.path()).parent().unwrap()).unwrap();
        std::fs::write(
            grants_path(dir.path()),
            r#"{"local:h":{"list_all":true,"machines":["m-air"," ",7]},"local:o":{"list_all":false}}"#,
        )
        .unwrap();
        let g = read_grants(dir.path(), "local:h");
        assert!(g.list_all);
        assert_eq!(g.machines, BTreeSet::from(["m-air".to_owned()]));
        assert_eq!(read_grants(dir.path(), "local:o"), Grants::default());
        assert_eq!(read_grants(dir.path(), "local:none"), Grants::default());
        std::fs::write(grants_path(dir.path()), "not json").unwrap();
        assert_eq!(read_grants(dir.path(), "local:h"), Grants::default());
    }

    #[test]
    fn a_space_mentioned_after_the_home_was_made_is_listable_and_a_machine_usable_with_no_card() {
        let home = HomeScope::from_labels(&labels(&[(LABEL_HOME, "1")]));
        assert!(!home.may_list("local:ci", "ci") && !home.may_use_machine("studio"));
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(grants_path(dir.path()).parent().unwrap()).unwrap();
        std::fs::write(
            grants_path(dir.path()),
            r#"{"local:h":{"mentions":["local:ci","ci","studio",""," ",4]}}"#,
        )
        .unwrap();
        let g = read_grants(dir.path(), "local:h");
        assert_eq!(
            g.mentions,
            BTreeSet::from(["local:ci".to_owned(), "ci".to_owned(), "studio".to_owned()])
        );
        let s = home.with_grants(g);
        // By id or by name, with the daemon reading the file each time (no restart).
        assert!(s.may_list("local:ci", "x") && s.may_list("y", "ci"));
        assert!(s.may_use_machine("studio"));
        assert!(!s.may_list("other", "other") && !s.has(Ability::ListSpaces));
        // A mention never lists everything.
        assert!(!s.list_all);
    }

    #[test]
    fn labels_round_trip() {
        let s = HomeScope {
            is_home: true,
            agent: "claude-code".into(),
            list_all: true,
            mentions: ["a".to_owned()].into(),
            machines: ["m".to_owned()].into(),
        };
        assert_eq!(HomeScope::from_labels(&s.to_labels()), s);
    }
}

#[cfg(test)]
mod warming_tests {
    use super::is_warming_home;
    use std::collections::HashMap;

    #[test]
    fn the_warming_home_is_not_listed_to_an_agent() {
        let warming: HashMap<String, String> = [("cua.hidden".to_string(), "home".to_string())]
            .into_iter()
            .collect();
        assert!(is_warming_home(&warming));
        assert!(!is_warming_home(&HashMap::new()));
    }
}
