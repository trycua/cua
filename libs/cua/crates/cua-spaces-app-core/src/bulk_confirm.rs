// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Deleting and leaving several Spaces at once (the Spaces page's selection,
//! and a single Space's Delete Space or Leave): the one dialog that asks,
//! the selection bar's count while it runs, and the toast when it is done.
//!
//! * Your Spaces are deleted: the dialog lists each with its size, says
//!   which are running (they are turned off first) and who loses access,
//!   and counts what it frees (only yours, only known sizes).
//! * Someone else's Spaces are left: they stay with their owners, so the
//!   button is plain, not red.
//! * A selection of both says both ("Delete 2 Spaces and leave 1?") in two
//!   groups, Delete and Leave. Your Spaces on more than one computer are
//!   grouped by computer.
//!
//! Sizes read as the web's `lib/resources.ts` `size()` writes them: whole
//! GB from 10 GB up ("29 GB"), one decimal below ("3.1 GB"), TB from 1 TB.

use serde::{Deserialize, Serialize};

/// Someone who has one of your Spaces.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BulkPerson {
    /// Their email (or a team's key).
    pub who: String,
    /// Their display name, when known.
    pub name: Option<String>,
}

/// One selected Space.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BulkItem {
    /// The Space's id (as this client keys it).
    pub id: String,
    /// Its name.
    pub name: String,
    /// Its system (`linux`, `macos`, `windows`, ...), for the row's mark.
    pub os: String,
    /// It runs now.
    pub running: bool,
    /// Its size on disk (yours only; none: not known).
    pub bytes: Option<u64>,
    /// Yours (deleted); otherwise someone else's or a team's (left).
    pub mine: bool,
    /// Where it is: "This Mac", "Sam's Mac mini", "QA team".
    pub place: String,
    /// A team's Space (its place names the team).
    pub team: bool,
    /// Who else has it (yours only): they lose access.
    pub people: Vec<BulkPerson>,
}

/// One row in the dialog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkRow {
    /// The Space's id.
    pub id: String,
    /// Its name.
    pub name: String,
    /// Its system, for the mark.
    pub os: String,
    /// "Running" when it runs (it is turned off first).
    pub status: Option<String>,
    /// Who has it, for the small stack after the name.
    pub people: Vec<String>,
    /// On the right: the size of yours ("14 GB"), the computer of theirs
    /// ("Sam's Mac mini"); none: nothing known.
    pub detail: Option<String>,
}

/// A group of rows ("Delete", "Leave", or a computer's name).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkGroup {
    /// Its heading; none: a single group needs none.
    pub title: Option<String>,
    /// Its rows.
    pub rows: Vec<BulkRow>,
}

/// The confirm dialog as drawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkConfirmView {
    /// "Delete 3 Spaces?", "Delete ci-runner?", "Delete 2 Spaces and leave
    /// 1?", "Leave 2 Spaces?".
    pub title: String,
    /// The sentences under it, in one paragraph.
    pub body: String,
    /// The rows.
    pub groups: Vec<BulkGroup>,
    /// "Frees", before the amount.
    pub frees_label: String,
    /// "29 GB": what deleting yours frees; none when nothing of yours has
    /// a known size (or nothing is deleted).
    pub frees: Option<String>,
    /// "Cancel".
    pub cancel: String,
    /// "Delete", "Delete 3 Spaces", "Delete and leave", "Leave".
    pub button: String,
    /// The button is red (anything is deleted).
    pub destructive: bool,
    /// What to delete (`spaces.deleteMany`), in order.
    pub delete_ids: Vec<String>,
    /// What to leave (`spaces.leave`), in order.
    pub leave_ids: Vec<String>,
}

/// One Space's outcome (`spaces.deleteMany` / `spaces.leave` results).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BulkResult {
    /// The Space's id.
    pub id: String,
    /// It was deleted or left.
    pub ok: bool,
    /// Why not, in words.
    pub error: Option<String>,
}

/// How the done toast reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BulkTone {
    /// Everything was done.
    Ok,
    /// Some were done, some not.
    Partial,
    /// Nothing was done.
    Failed,
}

/// The toast when it is done.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkDone {
    /// "Deleted 3 Spaces. 29 GB freed."
    pub text: String,
    /// How it went.
    pub tone: BulkTone,
}

const GB: f64 = 1_000_000_000.0;

/// "29 GB", "3.1 GB", "1 TB", "1.5 TB" (the web's `size()`).
pub fn size_text(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 999.5 * GB {
        let tb = (b / (GB * 100.0)).round() / 10.0;
        return if tb.fract() == 0.0 {
            format!("{tb:.0} TB")
        } else {
            format!("{tb:.1} TB")
        };
    }
    let tenths = (b / GB * 10.0).round() / 10.0;
    if tenths >= 9.95 || tenths.fract() == 0.0 {
        format!("{} GB", tenths.round() as u64)
    } else {
        format!("{tenths:.1} GB")
    }
}

/// "Sam", "Sam and Priya", "Sam, Priya and Lee", "Sam, Priya and 3 others".
pub fn names(list: &[String]) -> String {
    match list {
        [] => String::new(),
        [a] => a.clone(),
        [a, b] => format!("{a} and {b}"),
        [a, b, c] => format!("{a}, {b} and {c}"),
        [a, b, rest @ ..] => format!("{a}, {b} and {} others", rest.len()),
    }
}

fn person_name(p: &BulkPerson) -> String {
    p.name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| p.who.split('@').next().unwrap_or(&p.who).to_owned())
}

fn people_of(item: &BulkItem) -> Vec<String> {
    let mut seen: Vec<&str> = vec![];
    let mut out = vec![];
    for p in &item.people {
        if seen.contains(&p.who.as_str()) {
            continue;
        }
        seen.push(&p.who);
        out.push(person_name(p));
    }
    out
}

fn spaces(n: usize) -> String {
    format!("{n} Space{}", if n == 1 { "" } else { "s" })
}

fn row(item: &BulkItem) -> BulkRow {
    BulkRow {
        id: item.id.clone(),
        name: item.name.clone(),
        os: item.os.clone(),
        status: item.running.then(|| "Running".into()),
        people: if item.mine { people_of(item) } else { vec![] },
        detail: if item.mine {
            item.bytes.map(size_text)
        } else {
            Some(item.place.clone()).filter(|p| !p.is_empty())
        },
    }
}

/// Distinct places, in the order they first show.
fn places<'a>(items: &[&'a BulkItem]) -> Vec<&'a str> {
    let mut out: Vec<&str> = vec![];
    for i in items {
        if !out.contains(&i.place.as_str()) {
            out.push(&i.place);
        }
    }
    out
}

/// The confirm dialog for `items` (none: nothing selected).
pub fn confirm(items: &[BulkItem]) -> Option<BulkConfirmView> {
    if items.is_empty() {
        return None;
    }
    let deletes: Vec<&BulkItem> = items.iter().filter(|i| i.mine).collect();
    let leaves: Vec<&BulkItem> = items.iter().filter(|i| !i.mine).collect();
    let (d, l) = (deletes.len(), leaves.len());

    let title = match (d, l) {
        (1, 0) => format!("Delete {}?", deletes[0].name),
        (_, 0) => format!("Delete {}?", spaces(d)),
        (0, 1) => format!("Leave {}?", leaves[0].name),
        (0, _) => format!("Leave {}?", spaces(l)),
        _ => format!("Delete {} and leave {l}?", spaces(d)),
    };
    let button = match (d, l) {
        (1, 0) => "Delete".to_owned(),
        (_, 0) => format!("Delete {}", spaces(d)),
        (0, _) => "Leave".to_owned(),
        _ => "Delete and leave".to_owned(),
    };

    let mut body: Vec<String> = vec![];
    let running: Vec<String> = deletes
        .iter()
        .filter(|i| i.running)
        .map(|i| i.name.clone())
        .collect();
    if !running.is_empty() {
        body.push(format!(
            "{} {} running and will be turned off first.",
            names(&running),
            if running.len() == 1 { "is" } else { "are" }
        ));
    }
    let mut who: Vec<String> = vec![];
    let mut seen: Vec<&str> = vec![];
    for i in &deletes {
        for p in &i.people {
            if !seen.contains(&p.who.as_str()) {
                seen.push(&p.who);
                who.push(person_name(p));
            }
        }
    }
    if !who.is_empty() {
        body.push(format!(
            "{} {} access.",
            names(&who),
            if who.len() == 1 { "loses" } else { "lose" }
        ));
    }
    if d > 0 {
        body.push(
            if d == 1 {
                "Its files and apps are gone for good."
            } else {
                "Their files and apps are gone for good."
            }
            .into(),
        );
    }
    if l == 1 && !leaves[0].place.is_empty() {
        let it = leaves[0];
        body.push(format!(
            "{} stays {} {}.",
            it.name,
            if it.team { "with" } else { "on" },
            it.place
        ));
    } else if l > 0 {
        body.push("The ones you leave stay with their owners.".into());
    }

    let groups = if d > 0 && l > 0 {
        vec![
            BulkGroup {
                title: Some("Delete".into()),
                rows: deletes.iter().map(|i| row(i)).collect(),
            },
            BulkGroup {
                title: Some("Leave".into()),
                rows: leaves.iter().map(|i| row(i)).collect(),
            },
        ]
    } else if l == 0 && places(&deletes).len() > 1 {
        places(&deletes)
            .into_iter()
            .map(|p| BulkGroup {
                title: Some(p.to_owned()),
                rows: deletes
                    .iter()
                    .filter(|i| i.place == p)
                    .map(|i| row(i))
                    .collect(),
            })
            .collect()
    } else {
        vec![BulkGroup {
            title: None,
            rows: items.iter().map(row).collect(),
        }]
    };

    let known: Vec<u64> = deletes.iter().filter_map(|i| i.bytes).collect();
    Some(BulkConfirmView {
        title,
        body: body.join(" "),
        groups,
        frees_label: "Frees".into(),
        frees: (!known.is_empty()).then(|| size_text(known.iter().sum())),
        cancel: "Cancel".into(),
        button,
        destructive: d > 0,
        delete_ids: deletes.iter().map(|i| i.id.clone()).collect(),
        leave_ids: leaves.iter().map(|i| i.id.clone()).collect(),
    })
}

/// The selection bar while it runs: "Deleting 1 of 3…", then "Leaving 1 of
/// 2…" (yours are deleted first). `finished`: the ids done so far (either
/// way). None once everything is finished.
pub fn progress(items: &[BulkItem], finished: &[String]) -> Option<String> {
    for (mine, verb) in [(true, "Deleting"), (false, "Leaving")] {
        let all: Vec<&BulkItem> = items.iter().filter(|i| i.mine == mine).collect();
        let n = all.iter().filter(|i| finished.contains(&i.id)).count();
        if n < all.len() {
            return Some(format!("{verb} {} of {}\u{2026}", n + 1, all.len()));
        }
    }
    None
}

fn sentence(s: &str) -> String {
    let s = s.trim().trim_end_matches('.');
    format!("{s}.")
}

/// The toast once every result is in: "Deleted 3 Spaces. 29 GB freed.",
/// "Left 2 Spaces.", "Deleted 2 Spaces and left 1.", and what failed.
/// Freed counts only yours that were deleted, with known sizes.
pub fn done(items: &[BulkItem], results: &[BulkResult]) -> BulkDone {
    let ok = |i: &BulkItem| results.iter().any(|r| r.id == i.id && r.ok);
    let deletes: Vec<&BulkItem> = items.iter().filter(|i| i.mine).collect();
    let leaves: Vec<&BulkItem> = items.iter().filter(|i| !i.mine).collect();
    let d_ok = deletes.iter().filter(|i| ok(i)).count();
    let l_ok = leaves.iter().filter(|i| ok(i)).count();
    let only_one = items.len() == 1;

    let count = |done: usize, all: &[&BulkItem]| -> String {
        if done == all.len() {
            if only_one {
                all[0].name.clone()
            } else {
                spaces(done)
            }
        } else {
            format!("{done} of {} Spaces", all.len())
        }
    };
    let mut parts: Vec<String> = vec![];
    match (d_ok, l_ok) {
        (0, 0) => {}
        (_, 0) => parts.push(format!("Deleted {}.", count(d_ok, &deletes))),
        (0, _) => parts.push(format!("Left {}.", count(l_ok, &leaves))),
        _ => parts.push(format!(
            "Deleted {} and left {}.",
            count(d_ok, &deletes),
            if l_ok == leaves.len() {
                l_ok.to_string()
            } else {
                format!("{l_ok} of {}", leaves.len())
            }
        )),
    }
    let freed: u64 = deletes
        .iter()
        .filter(|i| ok(i))
        .filter_map(|i| i.bytes)
        .sum();
    if freed > 0 {
        parts.push(format!("{} freed.", size_text(freed)));
    }

    let failed: Vec<(&BulkItem, Option<&str>)> = items
        .iter()
        .filter(|i| !ok(i))
        .map(|i| {
            let err = results
                .iter()
                .find(|r| r.id == i.id)
                .and_then(|r| r.error.as_deref())
                .map(str::trim)
                .filter(|e| !e.is_empty());
            (i, err)
        })
        .collect();
    if let [(item, Some(err))] = failed.as_slice() {
        parts.push(
            if err.starts_with("Couldn't") || err.starts_with("Couldn\u{2019}t") {
                sentence(err)
            } else {
                let verb = if item.mine { "delete" } else { "leave" };
                sentence(&format!("Couldn't {verb} {}: {err}", item.name))
            },
        );
    } else {
        for (mine, verb) in [(true, "delete"), (false, "leave")] {
            let who: Vec<String> = failed
                .iter()
                .filter(|(i, _)| i.mine == mine)
                .map(|(i, _)| i.name.clone())
                .collect();
            if !who.is_empty() {
                parts.push(format!("Couldn't {verb} {}.", names(&who)));
            }
        }
    }
    BulkDone {
        text: parts.join(" "),
        tone: if failed.is_empty() {
            BulkTone::Ok
        } else if d_ok + l_ok == 0 {
            BulkTone::Failed
        } else {
            BulkTone::Partial
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: u64 = 1_000_000_000;

    fn mine(name: &str, gb: u64, running: bool) -> BulkItem {
        BulkItem {
            id: format!("local:{name}"),
            name: name.into(),
            os: "linux".into(),
            running,
            bytes: Some(gb * G),
            mine: true,
            place: "This Mac".into(),
            ..Default::default()
        }
    }

    fn theirs(name: &str, place: &str) -> BulkItem {
        BulkItem {
            id: format!("relay:m/{name}"),
            name: name.into(),
            os: "linux".into(),
            place: place.into(),
            ..Default::default()
        }
    }

    fn person(who: &str, name: Option<&str>) -> BulkPerson {
        BulkPerson {
            who: who.into(),
            name: name.map(Into::into),
        }
    }

    fn rows(v: &BulkConfirmView) -> Vec<String> {
        v.groups
            .iter()
            .flat_map(|g| {
                let head = g.title.clone().map(|t| format!("# {t}"));
                head.into_iter().chain(g.rows.iter().map(|r| {
                    format!(
                        "{}{} {}",
                        r.name,
                        r.status
                            .as_deref()
                            .map(|s| format!(" ({s})"))
                            .unwrap_or_default(),
                        r.detail.as_deref().unwrap_or("-")
                    )
                }))
            })
            .collect()
    }

    #[test]
    fn sizes_read_like_the_web() {
        assert_eq!(size_text(29 * G), "29 GB");
        assert_eq!(size_text(3_100_000_000), "3.1 GB");
        assert_eq!(size_text(8 * G), "8 GB");
        assert_eq!(size_text(9_960_000_000), "10 GB");
        assert_eq!(size_text(14_400_000_000), "14 GB");
        assert_eq!(size_text(999_400_000_000), "999 GB");
        assert_eq!(size_text(1000 * G), "1 TB");
        assert_eq!(size_text(1_500 * G), "1.5 TB");
        assert_eq!(size_text(0), "0 GB");
    }

    #[test]
    fn names_list_two_then_others() {
        let n = |v: &[&str]| names(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(n(&["Sam"]), "Sam");
        assert_eq!(n(&["Sam", "Priya"]), "Sam and Priya");
        assert_eq!(n(&["Sam", "Priya", "Lee"]), "Sam, Priya and Lee");
        assert_eq!(
            n(&["Sam", "Priya", "Lee", "Ana", "Bo"]),
            "Sam, Priya and 3 others"
        );
    }

    #[test]
    fn three_of_yours_one_running_and_shared() {
        // Storyboard 6, Bulk delete.
        let mut builder = mine("cua-builder", 14, true);
        builder.people = vec![person("sam@example.com", Some("Sam"))];
        let items = [
            builder,
            mine("agent-sandbox", 6, false),
            mine("onb-new", 9, false),
        ];
        let v = confirm(&items).unwrap();
        assert_eq!(v.title, "Delete 3 Spaces?");
        assert_eq!(
            v.body,
            "cua-builder is running and will be turned off first. Sam loses access. Their files and apps are gone for good."
        );
        assert_eq!(
            rows(&v),
            [
                "cua-builder (Running) 14 GB",
                "agent-sandbox 6 GB",
                "onb-new 9 GB"
            ]
        );
        assert_eq!(v.groups[0].rows[0].people, ["Sam"]);
        assert_eq!(v.frees_label, "Frees");
        assert_eq!(v.frees.as_deref(), Some("29 GB"));
        assert_eq!(v.button, "Delete 3 Spaces");
        assert!(v.destructive);
        assert_eq!(v.cancel, "Cancel");
        assert_eq!(v.delete_ids.len(), 3);
        assert!(v.leave_ids.is_empty());
    }

    #[test]
    fn one_running_space() {
        // Storyboard 8: ci-runner from its menu.
        let v = confirm(&[mine("ci-runner", 22, true)]).unwrap();
        assert_eq!(v.title, "Delete ci-runner?");
        assert_eq!(
            v.body,
            "ci-runner is running and will be turned off first. Its files and apps are gone for good."
        );
        assert_eq!(v.button, "Delete");
        assert_eq!(v.frees.as_deref(), Some("22 GB"));
    }

    #[test]
    fn a_shared_space_names_who_loses_access() {
        // Storyboard 10, Deleting a Space you shared: five people.
        let mut d = mine("design-review", 34, true);
        d.os = "macos".into();
        d.people = vec![
            person("sam@example.com", Some("Sam")),
            person("priya@example.com", Some("Priya")),
            person("lee@example.com", None),
            person("ana@example.com", Some("Ana")),
            person("bo@example.com", Some("Bo")),
        ];
        let v = confirm(&[d]).unwrap();
        assert_eq!(
            v.body,
            "design-review is running and will be turned off first. Sam, Priya and 3 others lose access. Its files and apps are gone for good."
        );
        assert_eq!(v.groups[0].rows[0].people[2], "lee");
    }

    #[test]
    fn people_are_counted_once_across_spaces() {
        let mut a = mine("a", 1, false);
        a.people = vec![person("sam@x.com", Some("Sam"))];
        let mut b = mine("b", 1, false);
        b.people = vec![
            person("sam@x.com", Some("Sam")),
            person("priya@x.com", Some("Priya")),
        ];
        let v = confirm(&[a, b]).unwrap();
        assert!(v.body.contains("Sam and Priya lose access."), "{}", v.body);
    }

    #[test]
    fn delete_and_leave() {
        // Storyboard 10, Delete and leave.
        let mut builder = mine("cua-builder", 14, true);
        builder.people = vec![person("sam@example.com", Some("Sam"))];
        let items = [
            builder,
            theirs("qa-shared", "Sam's Mac mini"),
            mine("onb-new", 9, false),
        ];
        let v = confirm(&items).unwrap();
        assert_eq!(v.title, "Delete 2 Spaces and leave 1?");
        assert_eq!(
            v.body,
            "cua-builder is running and will be turned off first. Sam loses access. Their files and apps are gone for good. qa-shared stays on Sam's Mac mini."
        );
        assert_eq!(
            rows(&v),
            [
                "# Delete",
                "cua-builder (Running) 14 GB",
                "onb-new 9 GB",
                "# Leave",
                "qa-shared Sam's Mac mini"
            ]
        );
        // Only yours count.
        assert_eq!(v.frees.as_deref(), Some("23 GB"));
        assert_eq!(v.button, "Delete and leave");
        assert!(v.destructive);
        assert_eq!(v.leave_ids, ["relay:m/qa-shared"]);
    }

    #[test]
    fn leave_only_is_plain() {
        // Storyboard 10, Leave.
        let v = confirm(&[
            theirs("label-studio", "Priya's PC"),
            theirs("qa-shared", "Sam's Mac mini"),
        ])
        .unwrap();
        assert_eq!(v.title, "Leave 2 Spaces?");
        assert_eq!(v.body, "The ones you leave stay with their owners.");
        assert_eq!(
            rows(&v),
            ["label-studio Priya's PC", "qa-shared Sam's Mac mini"]
        );
        assert_eq!(v.button, "Leave");
        assert!(!v.destructive);
        assert_eq!(v.frees, None);
        let one = confirm(&[theirs("qa-shared", "Sam's Mac mini")]).unwrap();
        assert_eq!(one.title, "Leave qa-shared?");
        assert_eq!(one.body, "qa-shared stays on Sam's Mac mini.");
        let mut team = theirs("nightly-e2e", "QA team");
        team.team = true;
        assert_eq!(
            confirm(&[team]).unwrap().body,
            "nightly-e2e stays with QA team."
        );
    }

    #[test]
    fn a_running_space_you_leave_is_not_turned_off() {
        let mut t = theirs("qa-shared", "Sam's Mac mini");
        t.running = true;
        let v = confirm(&[t]).unwrap();
        assert!(!v.body.contains("turned off"));
        assert_eq!(v.groups[0].rows[0].status.as_deref(), Some("Running"));
    }

    #[test]
    fn yours_on_two_computers_are_grouped_by_computer() {
        let mut far = mine("scrape-worker", 11, false);
        far.place = "Hetzner server".into();
        let v = confirm(&[mine("tahoe-old", 41, false), far, mine("onb-new", 9, false)]).unwrap();
        assert_eq!(
            rows(&v),
            [
                "# This Mac",
                "tahoe-old 41 GB",
                "onb-new 9 GB",
                "# Hetzner server",
                "scrape-worker 11 GB"
            ]
        );
        assert_eq!(v.frees.as_deref(), Some("61 GB"));
    }

    #[test]
    fn frees_counts_only_known_sizes() {
        let mut unknown = mine("far", 1, false);
        unknown.bytes = None;
        let v = confirm(&[mine("a", 5, false), unknown.clone()]).unwrap();
        assert_eq!(v.frees.as_deref(), Some("5 GB"));
        assert_eq!(v.groups[0].rows[1].detail, None);
        assert_eq!(confirm(&[unknown]).unwrap().frees, None);
        assert_eq!(confirm(&[]), None);
    }

    #[test]
    fn progress_counts_through_deletes_then_leaves() {
        let items = [
            mine("a", 1, false),
            theirs("t1", "P"),
            mine("b", 1, false),
            mine("c", 1, false),
            theirs("t2", "P"),
        ];
        let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            progress(&items, &[]).as_deref(),
            Some("Deleting 1 of 3\u{2026}")
        );
        assert_eq!(
            progress(&items, &ids(&["local:a", "local:b"])).as_deref(),
            Some("Deleting 3 of 3\u{2026}")
        );
        assert_eq!(
            progress(&items, &ids(&["local:a", "local:b", "local:c"])).as_deref(),
            Some("Leaving 1 of 2\u{2026}")
        );
        assert_eq!(
            progress(
                &items,
                &ids(&["local:a", "local:b", "local:c", "relay:m/t1", "relay:m/t2"])
            ),
            None
        );
    }

    fn ok(id: &str) -> BulkResult {
        BulkResult {
            id: id.into(),
            ok: true,
            error: None,
        }
    }

    fn failed(id: &str, error: Option<&str>) -> BulkResult {
        BulkResult {
            id: id.into(),
            ok: false,
            error: error.map(Into::into),
        }
    }

    #[test]
    fn done_says_what_was_freed() {
        let items = [
            mine("cua-builder", 14, true),
            mine("agent-sandbox", 6, false),
            mine("onb-new", 9, false),
        ];
        let all: Vec<_> = items.iter().map(|i| ok(&i.id)).collect();
        let d = done(&items, &all);
        assert_eq!(d.text, "Deleted 3 Spaces. 29 GB freed.");
        assert_eq!(d.tone, BulkTone::Ok);
        let one = [mine("ci-runner", 22, true)];
        assert_eq!(
            done(&one, &[ok("local:ci-runner")]).text,
            "Deleted ci-runner. 22 GB freed."
        );
        let mut unknown = mine("x", 1, false);
        unknown.bytes = None;
        assert_eq!(done(&[unknown], &[ok("local:x")]).text, "Deleted x.");
    }

    #[test]
    fn done_after_leaving() {
        let items = [theirs("label-studio", "P"), theirs("qa-shared", "S")];
        let d = done(
            &items,
            &[ok("relay:m/label-studio"), ok("relay:m/qa-shared")],
        );
        assert_eq!(d.text, "Left 2 Spaces.");
        assert_eq!(
            done(&items[..1], &[ok("relay:m/label-studio")]).text,
            "Left label-studio."
        );
        let mixed = [
            mine("cua-builder", 14, true),
            theirs("qa-shared", "S"),
            mine("onb-new", 9, false),
        ];
        let d = done(
            &mixed,
            &[
                ok("local:cua-builder"),
                ok("relay:m/qa-shared"),
                ok("local:onb-new"),
            ],
        );
        assert_eq!(d.text, "Deleted 2 Spaces and left 1. 23 GB freed.");
    }

    #[test]
    fn done_with_failures() {
        let items = [
            mine("a", 10, false),
            mine("b", 20, false),
            mine("c", 30, false),
        ];
        let d = done(
            &items,
            &[
                ok("local:a"),
                failed("local:b", Some("it is busy")),
                ok("local:c"),
            ],
        );
        assert_eq!(
            d.text,
            "Deleted 2 of 3 Spaces. 40 GB freed. Couldn't delete b: it is busy."
        );
        assert_eq!(d.tone, BulkTone::Partial);
        let d = done(
            &items,
            &[
                ok("local:a"),
                failed("local:b", None),
                failed("local:c", Some("x")),
            ],
        );
        assert_eq!(
            d.text,
            "Deleted 1 of 3 Spaces. 10 GB freed. Couldn't delete b and c."
        );
        // Nothing done; a missing result is a failure too.
        let d = done(&items, &[]);
        assert_eq!(d.text, "Couldn't delete a, b and c.");
        assert_eq!(d.tone, BulkTone::Failed);
    }

    #[test]
    fn a_failed_leave_keeps_the_clients_sentence() {
        let items = [theirs("qa-shared", "S"), theirs("label-studio", "P")];
        let d = done(
            &items,
            &[
                failed(
                    "relay:m/qa-shared",
                    Some("Couldn't leave qa-shared: the Cua relay needs an update."),
                ),
                ok("relay:m/label-studio"),
            ],
        );
        assert_eq!(
            d.text,
            "Left 1 of 2 Spaces. Couldn't leave qa-shared: the Cua relay needs an update."
        );
        let d = done(
            &[mine("a", 1, false), theirs("t", "P")],
            &[ok("local:a"), failed("relay:m/t", Some("refused"))],
        );
        assert_eq!(
            d.text,
            "Deleted 1 Space. 1 GB freed. Couldn't leave t: refused."
        );
    }
}
