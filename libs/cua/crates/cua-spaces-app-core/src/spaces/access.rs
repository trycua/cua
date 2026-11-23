// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Whose a Space is and what you may do with it ([`SpaceAccess`]), from the
//! SDK's sharing facts on its row ([`SpaceSharingFacts`]): yours, shared
//! with you (as an editor or a viewer), or a team's; the meta line the
//! page shows under its name ("This Mac", "Sam's Mac mini", "QA"); who else
//! has one of yours; and which actions it offers.

use crate::model::{
    SpaceAccess, SpaceAccessKind, SpaceAccessRole, SpacePerson, SpaceProvider, SpaceRow,
    SpaceSharingFacts,
};

/// The access of the Space on `row`. `power` says whether it turns off and
/// on at all.
pub fn access_of(row: &SpaceRow, power: bool) -> SpaceAccess {
    let facts = row.sharing.as_ref();
    let role = match facts.map(|f| f.role.as_str()) {
        Some("viewer") => SpaceAccessRole::Viewer,
        Some("editor") | Some("shared") => SpaceAccessRole::Editor,
        _ => SpaceAccessRole::Owner,
    };
    let team = facts.and_then(|f| text(&f.team));
    let made_for_team =
        role == SpaceAccessRole::Owner && facts.is_some_and(|f| !f.teams.is_empty());
    let kind = if team.is_some() || made_for_team {
        SpaceAccessKind::Team
    } else if role == SpaceAccessRole::Owner {
        SpaceAccessKind::Mine
    } else {
        SpaceAccessKind::WithMe
    };
    let team_name = match (&team, facts) {
        (Some(key), Some(f)) => Some(text(&f.team_name).unwrap_or_else(|| key.clone())),
        // A team Space you made: the (first) team it is shared with.
        (None, Some(f)) if made_for_team => f.teams.first().map(|t| {
            if t.name.trim().is_empty() {
                t.key.clone()
            } else {
                t.name.clone()
            }
        }),
        _ => None,
    };
    let (owner_name, owner_email) = match facts {
        Some(f) if role != SpaceAccessRole::Owner => (Some(owner_name_of(f)), text(&f.owner_email)),
        _ => (None, None),
    };
    let place = match kind {
        SpaceAccessKind::Team => team_name.clone().unwrap_or_default(),
        SpaceAccessKind::WithMe => {
            let owner = owner_name.clone().unwrap_or_default();
            match text(&row.host_name) {
                Some(computer) => possessive(&owner, &computer),
                None => owner,
            }
        }
        SpaceAccessKind::Mine => own_place(row),
    };
    let people = match facts {
        Some(f) if role == SpaceAccessRole::Owner => people_of(f),
        _ => Vec::new(),
    };
    let owner = role == SpaceAccessRole::Owner;
    let editor = role != SpaceAccessRole::Viewer;
    SpaceAccess {
        kind,
        role,
        owner_name,
        owner_email,
        team_name,
        place,
        people,
        can_power: power && editor,
        can_teleport: editor,
        can_share: owner,
        can_rename: owner,
        can_delete: owner && row.cloud_delete.as_deref() != Some("elsewhere"),
        can_leave: !owner,
    }
}

fn text(v: &Option<String>) -> Option<String> {
    v.as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// The owner's name: their display name, else their email's local part,
/// else their account id.
pub fn owner_name_of(f: &SpaceSharingFacts) -> String {
    text(&f.owner_name)
        .or_else(|| {
            text(&f.owner_email).map(|e| e.split('@').next().unwrap_or_default().to_string())
        })
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| f.owner_id.clone())
}

/// "Sam's Mac mini" from "Sam Lee" and "Mac mini"; a computer already
/// named after its owner ("Sam's Mac mini") stays as it is.
pub fn possessive(owner: &str, computer: &str) -> String {
    let first = owner.split_whitespace().next().unwrap_or(owner);
    if first.is_empty() {
        return computer.to_string();
    }
    let lower = computer.to_lowercase();
    let first_lower = first.to_lowercase();
    if lower.starts_with(&format!("{first_lower}'s "))
        || lower.starts_with(&format!("{first_lower}\u{2019}s "))
    {
        return computer.to_string();
    }
    let s = if first.ends_with('s') { "'" } else { "'s" };
    format!("{first}{s} {computer}")
}

/// Where a Space of yours runs, in words.
fn own_place(row: &SpaceRow) -> String {
    match super::normalize_provider(&row.provider) {
        SpaceProvider::Local => "This Mac".into(),
        SpaceProvider::Direct => row
            .id
            .strip_prefix("direct:")
            .unwrap_or(&row.id)
            .to_string(),
        SpaceProvider::Relay => text(&row.cloud_place)
            .or_else(|| text(&row.host_name))
            .or_else(|| text(&row.cloud))
            .unwrap_or_else(|| "My machines".into()),
    }
}

/// Who has a Space of yours: editors, viewers (a viewer listed as both is
/// a viewer, as the relay reads it) and whole teams.
fn people_of(f: &SpaceSharingFacts) -> Vec<SpacePerson> {
    let viewer = |who: &str| f.viewers.iter().any(|v| v.eq_ignore_ascii_case(who));
    let mut out: Vec<SpacePerson> = f
        .editors
        .iter()
        .filter(|e| !viewer(e))
        .map(|who| SpacePerson {
            who: who.clone(),
            name: None,
            role: SpaceAccessRole::Editor,
            team: false,
        })
        .collect();
    out.extend(f.viewers.iter().map(|who| SpacePerson {
        who: who.clone(),
        name: None,
        role: SpaceAccessRole::Viewer,
        team: false,
    }));
    out.extend(f.teams.iter().map(|t| SpacePerson {
        who: format!("team:{}", t.key),
        name: Some(if t.name.trim().is_empty() {
            t.key.clone()
        } else {
            t.name.clone()
        }),
        role: if t.role == "viewer" {
            SpaceAccessRole::Viewer
        } else {
            SpaceAccessRole::Editor
        },
        team: true,
    }));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SpaceTeamFact;

    fn row(id: &str, provider: &str) -> SpaceRow {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": id, "provider": provider, "spacesdVersion": "",
            "features": [], "reachable": true,
        }))
        .unwrap()
    }

    fn facts(role: &str) -> SpaceSharingFacts {
        SpaceSharingFacts {
            owner_id: "sam".into(),
            owner_email: Some("sam.lee@example.com".into()),
            owner_name: Some("Sam Lee".into()),
            role: role.into(),
            team: None,
            team_name: None,
            editors: vec![],
            viewers: vec![],
            teams: vec![],
        }
    }

    #[test]
    fn a_space_without_sharing_facts_is_yours_with_every_action() {
        let a = access_of(&row("local:ci-runner", "local"), true);
        assert_eq!(a.kind, SpaceAccessKind::Mine);
        assert_eq!(a.role, SpaceAccessRole::Owner);
        assert_eq!(a.place, "This Mac");
        assert!(a.people.is_empty());
        assert!(a.can_power && a.can_teleport && a.can_share && a.can_rename && a.can_delete);
        assert!(!a.can_leave);
        // One it cannot turn off; one only another device can delete.
        assert!(!access_of(&row("cloud:x", "cloud"), false).can_power);
        let mut r = row("relay:space-aws", "relay");
        r.cloud_delete = Some("elsewhere".into());
        r.cloud_place = Some("AWS us-west-2".into());
        let a = access_of(&r, true);
        assert!(!a.can_delete);
        assert_eq!(a.place, "AWS us-west-2");
    }

    #[test]
    fn yours_shared_lists_who_has_it_viewers_winning_over_editors() {
        let mut r = row("relay:space-mine", "relay");
        let mut f = facts("owner");
        f.editors = vec!["sam@example.com".into(), "priya@example.com".into()];
        f.viewers = vec!["Sam@example.com".into()];
        f.teams = vec![SpaceTeamFact {
            key: "org-qa".into(),
            name: String::new(),
            role: "viewer".into(),
        }];
        r.sharing = Some(f.clone());
        r.host_name = Some("Mac mini".into());
        let a = access_of(&r, true);
        // Shared with a team: a team Space you made.
        assert_eq!(a.kind, SpaceAccessKind::Team);
        assert_eq!(a.team_name.as_deref(), Some("org-qa"));
        assert!(a.can_delete && !a.can_leave);
        let people: Vec<(&str, SpaceAccessRole, bool)> = a
            .people
            .iter()
            .map(|p| (p.who.as_str(), p.role, p.team))
            .collect();
        assert_eq!(
            people,
            vec![
                ("priya@example.com", SpaceAccessRole::Editor, false),
                ("Sam@example.com", SpaceAccessRole::Viewer, false),
                ("team:org-qa", SpaceAccessRole::Viewer, true),
            ]
        );
        // Without the team: plain yours, on its computer.
        f.teams.clear();
        r.sharing = Some(f);
        let a = access_of(&r, true);
        assert_eq!(a.kind, SpaceAccessKind::Mine);
        assert_eq!(a.place, "Mac mini");
        assert_eq!(a.people.len(), 2);
    }

    #[test]
    fn shared_with_you_names_the_owner_and_their_computer() {
        let mut r = row("relay:space-qa", "relay");
        r.sharing = Some(facts("editor"));
        r.host_name = Some("Mac mini".into());
        let a = access_of(&r, true);
        assert_eq!(a.kind, SpaceAccessKind::WithMe);
        assert_eq!(a.role, SpaceAccessRole::Editor);
        assert_eq!(a.owner_name.as_deref(), Some("Sam Lee"));
        assert_eq!(a.owner_email.as_deref(), Some("sam.lee@example.com"));
        assert_eq!(a.place, "Sam's Mac mini");
        assert!(a.can_power && a.can_teleport && a.can_leave);
        assert!(!a.can_share && !a.can_rename && !a.can_delete);
        assert!(a.people.is_empty());
        // No computer known: the owner's name. No name: the email's local part.
        r.host_name = None;
        let mut f = facts("viewer");
        f.owner_name = None;
        r.sharing = Some(f);
        let a = access_of(&r, true);
        assert_eq!(a.place, "sam.lee");
        assert_eq!(a.owner_name.as_deref(), Some("sam.lee"));
        // A viewer only watches.
        assert!(!a.can_power && !a.can_teleport && a.can_leave);
    }

    #[test]
    fn a_teams_space_says_the_team_and_your_real_role() {
        let mut r = row("relay:space-team", "relay");
        let mut f = facts("viewer");
        f.team = Some("org-qa".into());
        f.team_name = Some("QA".into());
        r.sharing = Some(f.clone());
        let a = access_of(&r, true);
        assert_eq!(a.kind, SpaceAccessKind::Team);
        assert_eq!(a.place, "QA");
        assert_eq!(a.team_name.as_deref(), Some("QA"));
        assert!(!a.can_power && !a.can_teleport && a.can_leave && !a.can_delete);
        // The key when the name is not known.
        f.team_name = None;
        f.role = "editor".into();
        r.sharing = Some(f);
        let a = access_of(&r, true);
        assert_eq!(a.place, "org-qa");
        assert!(a.can_power && a.can_teleport);
    }

    #[test]
    fn possessives_read_naturally() {
        assert_eq!(possessive("Sam Lee", "Mac mini"), "Sam's Mac mini");
        assert_eq!(possessive("Priya", "PC"), "Priya's PC");
        assert_eq!(possessive("James", "Mac"), "James' Mac");
        assert_eq!(possessive("Sam", "Sam's Mac mini"), "Sam's Mac mini");
        assert_eq!(possessive("", "PC"), "PC");
    }
}
