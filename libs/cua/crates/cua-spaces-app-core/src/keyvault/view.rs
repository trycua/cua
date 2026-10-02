// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Pure shaping for the Keyvault page (ported from the webview's
//! `model/keyvault.ts`): items grouped per site and per account with their
//! consent state, recent decisions from the append-only audit log, live
//! access, and the page's unavailable and protection states.

use super::wire::*;
use serde::{Deserialize, Serialize};

/// A live grant: not revoked, not expired, uses left.
pub fn live_grant(g: &KvGrant, now: i64) -> bool {
    !g.revoked && now < g.not_after_ms as i64 && g.uses_left != Some(0)
}

/// A live rule.
pub fn live_rule(r: &KvRule, now: i64) -> bool {
    r.enabled && now < r.not_after_ms as i64
}

/// A delivery not yet wiped (one without expiry stays until wiped).
pub fn live_delivery(d: &KvDelivery, now: i64) -> bool {
    !d.wiped && (d.expires_ms == KV_NO_EXPIRY || now < d.expires_ms as i64)
}

/// How long a delivered copy stays: "wiped in 5 min", or "until you wipe
/// it" (auto-wipe off).
pub fn delivery_lifetime(expires_ms: u64, now: i64) -> String {
    if expires_ms == KV_NO_EXPIRY {
        "until you wipe it".into()
    } else {
        format!("wiped in {}", duration(expires_ms as i64 - now))
    }
}

/// The always-visible signal (notch indicator, menu bar line) while any
/// Keyvault sign-in is live in a Space, whoever sent it (an approved grant,
/// an unattended rule or the user): "Keyvault sign-ins live in dev-1", or
/// "... in 2 Spaces". None when nothing is live.
pub fn sharing_label(o: &KeyvaultOverview, now: i64) -> Option<String> {
    visible_sharing_label(o, now, &[])
}

/// [`sharing_label`] without the copies the user dismissed (their import
/// ids): Dismiss hides the notch indicator, it never revokes or wipes.
pub fn visible_sharing_label(
    o: &KeyvaultOverview,
    now: i64,
    dismissed: &[String],
) -> Option<String> {
    let targets = live_targets(o, now, dismissed);
    match targets.as_slice() {
        [] => None,
        [one] => Some(format!("Keyvault sign-ins live in {one}")),
        many => Some(format!("Keyvault sign-ins live in {} Spaces", many.len())),
    }
}

/// The targets with a live copy not in `dismissed`, sorted, once each.
fn live_targets<'a>(o: &'a KeyvaultOverview, now: i64, dismissed: &[String]) -> Vec<&'a str> {
    let mut targets: Vec<&str> = o
        .deliveries
        .iter()
        .filter(|d| live_delivery(d, now) && !dismissed.contains(&d.import_id))
        .map(|d| d.target.as_str())
        .collect();
    targets.sort_unstable();
    targets.dedup();
    targets
}

/// A delivery's target names this Space: its id (`local:dev-1`), the name
/// in it (`dev-1`, what an agent asks for), or its display name.
fn names_space(target: &str, space: &crate::model::Space) -> bool {
    target == space.id
        || target == space.name
        || space
            .id
            .split_once(':')
            .is_some_and(|(_, name)| name == target)
}

/// The ids of `spaces` signed in through the Keyvault: a live copy is in
/// them. The Spaces list marks every one ("Signed in"); the notch passes
/// the user's `dismissed` copies, which it no longer shows.
pub fn signed_in_spaces(
    o: &KeyvaultOverview,
    now: i64,
    dismissed: &[String],
    spaces: &[crate::model::Space],
) -> Vec<String> {
    let targets = live_targets(o, now, dismissed);
    spaces
        .iter()
        .filter(|s| targets.iter().any(|t| names_space(t, s)))
        .map(|s| s.id.clone())
        .collect()
}

/// The dismissed copies still live (a wiped or expired copy no longer
/// needs remembering).
pub fn prune_dismissed(o: &KeyvaultOverview, now: i64, dismissed: &[String]) -> Vec<String> {
    dismissed
        .iter()
        .filter(|id| {
            o.deliveries
                .iter()
                .any(|d| &d.import_id == *id && live_delivery(d, now))
        })
        .cloned()
        .collect()
}

/// The Access row of `space`'s copies (its [`AccessRow::key`]), to focus
/// when its "Signed in" badge is clicked.
pub fn space_access_key(
    o: &KeyvaultOverview,
    now: i64,
    space: &crate::model::Space,
) -> Option<String> {
    live_targets(o, now, &[])
        .into_iter()
        .find(|t| names_space(t, space))
        .map(|t| format!("d:{t}"))
}

fn js_round(x: f64) -> i64 {
    (x + 0.5).floor() as i64
}

/// "45s", "5 min", "3 h", "2 days".
pub fn duration(ms: i64) -> String {
    let s = js_round(ms as f64 / 1000.0).max(0);
    if s < 60 {
        return format!("{s}s");
    }
    let m = js_round(s as f64 / 60.0);
    if m < 60 {
        return format!("{m} min");
    }
    let h = js_round(m as f64 / 60.0);
    if h < 48 {
        return format!("{h} h");
    }
    format!("{} days", js_round(h as f64 / 24.0))
}

/// "now" or "5 min ago".
pub fn ago(ms: i64) -> String {
    if ms < 60_000 {
        "now".into()
    } else {
        format!("{} ago", duration(ms))
    }
}

/// A short name for a verified caller display: the signing identifier when
/// there is one, else the text before the parenthesised detail.
pub fn short_caller(display: &str) -> String {
    const KEY: &str = "signed id \"";
    if let Some(i) = display.find(KEY) {
        let rest = &display[i + KEY.len()..];
        if let Some(end) = rest.find('"')
            && end > 0
        {
            return rest[..end].to_string();
        }
    }
    let text = display.strip_prefix("Cua: ").unwrap_or(display);
    match text.find(" (") {
        Some(cut) if cut > 0 => text[..cut].to_string(),
        _ => text.to_string(),
    }
}

/// Trust tone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tone {
    /// Verified.
    Ok,
    /// Caution.
    Warn,
    /// Untrusted.
    Danger,
}

/// A caller's signing, in words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SigningBadge {
    /// "signed, team X".
    pub text: String,
    /// Tone.
    pub tone: Tone,
}

/// The badge for a caller.
pub fn signing_badge(c: &KvCaller) -> SigningBadge {
    match &c.signing {
        KvSigning::Signed { team_id, .. } if c.os_verified => SigningBadge {
            text: format!("signed, team {team_id}"),
            tone: Tone::Ok,
        },
        KvSigning::Signed { team_id, .. } => SigningBadge {
            text: format!("team {team_id}, not OS-verified"),
            tone: Tone::Warn,
        },
        KvSigning::AdHoc { .. } => SigningBadge {
            text: "ad hoc, untrusted".into(),
            tone: Tone::Warn,
        },
        _ => SigningBadge {
            text: "unsigned, untrusted".into(),
            tone: Tone::Danger,
        },
    }
}

/// Mixed on/off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tri {
    /// All on.
    On,
    /// All off.
    Off,
    /// Some.
    Mixed,
}

fn rule_targets(r: &KvRule) -> String {
    if r.targets.iter().any(|t| t == "*") {
        "any Space".into()
    } else {
        r.targets.join(", ")
    }
}

/// The audit kinds the Recent list shows, with their verbs.
pub const DECISION_KINDS: [(&str, &str); 16] = [
    ("consent.request", "Asked"),
    ("consent.allow", "Approved"),
    ("consent.deny", "Denied"),
    ("teleport.deliver", "Delivered"),
    ("teleport.deny", "Refused"),
    ("grant.revoke", "Revoked"),
    ("target.wipe", "Wiped"),
    ("rule.add", "Rule added"),
    ("rule.remove", "Rule removed"),
    ("item.policy", "Policy changed"),
    ("item.unlock", "Unlocked"),
    ("item.lock", "Locked"),
    ("item.delete", "Deleted"),
    ("vault.disable", "Keyvault turned off"),
    ("vault.enable", "Keyvault turned on"),
    ("caller.reject", "Caller refused"),
];

/// A decision's tone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DecisionTone {
    /// Allowed or delivered.
    Ok,
    /// Denied, refused, failed.
    Deny,
    /// Everything else.
    Info,
}

/// One row of Recent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    /// The entry.
    pub entry: KvAuditEntry,
    /// "Approved".
    pub verb: String,
    /// Tone.
    pub tone: DecisionTone,
    /// Item and target, or the actor.
    pub what: String,
}

/// Newest first, at most `limit`.
pub fn recent_decisions(o: &KeyvaultOverview, limit: usize) -> Vec<Decision> {
    let label = |id: &str| {
        o.items
            .iter()
            .find(|i| i.id == id)
            .filter(|i| !i.key.is_empty())
            .map(|i| match &i.domain {
                Some(d) => format!("{} / {}", super::vault::site_of(d), i.key),
                None => i.key.clone(),
            })
            .unwrap_or_else(|| "an item".to_string())
    };
    let mut entries: Vec<&KvAuditEntry> = o
        .audit
        .iter()
        .filter(|e| DECISION_KINDS.iter().any(|(k, _)| *k == e.kind))
        .collect();
    entries.sort_by_key(|e| std::cmp::Reverse(e.seq));
    entries
        .into_iter()
        .take(limit)
        .map(|e| {
            let parts: Vec<String> = [
                e.item.as_deref().filter(|s| !s.is_empty()).map(label),
                e.target
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .map(|t| format!("\u{2192} {t}")),
            ]
            .into_iter()
            .flatten()
            .collect();
            let tone = if e.decision == "deny"
                || e.decision == "error"
                || e.kind == "consent.deny"
                || e.kind == "teleport.deny"
            {
                DecisionTone::Deny
            } else if e.kind == "consent.allow" || e.kind == "teleport.deliver" {
                DecisionTone::Ok
            } else {
                DecisionTone::Info
            };
            Decision {
                verb: DECISION_KINDS
                    .iter()
                    .find(|(k, _)| *k == e.kind)
                    .map(|(_, v)| (*v).to_string())
                    .unwrap_or_else(|| e.kind.clone()),
                tone,
                what: if parts.is_empty() {
                    short_caller(&e.actor)
                } else {
                    parts.join(" ")
                },
                entry: e.clone(),
            }
        })
        .collect()
}

/// What a pending request would move, as one line: sites with their item
/// counts, files, and what the approval would import.
pub fn pending_summary(p: &KvPending) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for i in &p.items {
        let key = match i.domain.as_deref().filter(|d| !d.is_empty()) {
            Some(d) => super::vault::site_of(d),
            None if i.kind == "file" => format!("{} files", i.app_display),
            None => i.app_display.clone(),
        };
        match counts.iter_mut().find(|(k, _)| *k == key) {
            Some((_, n)) => *n += 1,
            None => counts.push((key, 1)),
        }
    }
    let mut what: Vec<String> = counts
        .into_iter()
        .map(|(k, n)| if n > 1 { format!("{k} ({n} items)") } else { k })
        .collect();
    what.extend(p.needs_import.iter().map(|s| match s {
        KvSelector::Site { site, .. } => format!("{site} (not saved)"),
        KvSelector::App { app } => format!("{app} (not saved)"),
        KvSelector::Item { id } => id.clone(),
        KvSelector::Login { site } => format!("the saved password for {site}"),
    }));
    let what = if what.is_empty() {
        "nothing".to_string()
    } else {
        what.join(", ")
    };
    if is_login(p) {
        return format!("Sign in to {what} in {}", p.request.targets.join(", "));
    }
    format!("{what} \u{2192} {}", p.request.targets.join(", "))
}

/// A request to sign in to a site with a saved password.
pub fn is_login(p: &KvPending) -> bool {
    p.request.actions.iter().any(|a| a == "login")
}

/// How long a request asks for ("15 min" by default).
pub fn wants(p: &KvPending) -> String {
    // A sign-in approval covers one use unless the user widens it.
    if is_login(p) && p.request.uses.is_none_or(|u| u == 1) {
        return "Once".into();
    }
    match p.request.duration_secs {
        Some(s) if s > 0 => duration(s as i64 * 1000),
        _ => "15 min".into(),
    }
}

/// The caller's own claims, marked unverified (the tooltip).
pub fn claims(p: &KvPending) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(a) = p.request.agent.as_deref().filter(|s| !s.is_empty()) {
        out.push(format!("For agent \"{a}\" (unverified)"));
    }
    if let Some(n) = p.request.claimed_name.as_deref().filter(|s| !s.is_empty()) {
        out.push(format!("Calls itself \"{n}\" (unverified)"));
    }
    if !p.request.reason.is_empty() {
        out.push(format!("Says: \"{}\" (unverified)", p.request.reason));
    }
    out
}

/// One row of Waiting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingRow {
    /// Request id.
    pub id: String,
    /// Short caller.
    pub caller: String,
    /// Signing badge.
    pub badge: SigningBadge,
    /// What and where.
    pub summary: String,
    /// For how long.
    pub wants: String,
    /// Unverified claims (tooltip).
    pub claims: Vec<String>,
}

/// The Waiting rows.
pub fn pending_rows(o: &KeyvaultOverview) -> Vec<PendingRow> {
    o.pending
        .iter()
        .map(|p| PendingRow {
            id: p.id.clone(),
            caller: short_caller(&p.caller_display),
            badge: signing_badge(&p.caller),
            summary: pending_summary(p),
            wants: wants(p),
            claims: claims(p),
        })
        .collect()
}

/// What live access a row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccessKind {
    /// A grant.
    Grant,
    /// An unattended rule.
    Rule,
    /// Copies in a Space.
    Delivery,
}

/// One row of Access.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessRow {
    /// Kind.
    pub kind: AccessKind,
    /// Stable key.
    pub key: String,
    /// Main text.
    pub text: String,
    /// Muted detail.
    pub detail: String,
    /// "Revoke", "Remove", "Wipe".
    pub action_label: String,
    /// What the button sends.
    pub command: KvCommand,
    /// A Space's copies: their import ids, which Dismiss hides from the
    /// notch (empty for grants and rules).
    #[serde(default)]
    pub imports: Vec<String>,
}

/// Live grants, rules and delivery targets, with their undo.
pub fn access_rows(o: &KeyvaultOverview, now: i64) -> Vec<AccessRow> {
    let names = |ids: &[String]| {
        let named = |id: &String| {
            o.items
                .iter()
                .find(|i| &i.id == id)
                .filter(|i| !i.key.is_empty())
                .map(|i| match &i.domain {
                    Some(d) => format!("{} / {}", super::vault::site_of(d), i.key),
                    None => i.key.clone(),
                })
        };
        let shown: Vec<String> = ids
            .iter()
            .take(3)
            .map(|id| named(id).unwrap_or_else(|| "an item".into()))
            .collect();
        let more = ids.len().saturating_sub(3);
        let mut text = shown.join(", ");
        if more > 0 {
            text.push_str(&format!(" and {more} more"));
        }
        text
    };
    let mut rows = Vec::new();
    for g in o.grants.iter().filter(|g| live_grant(g, now)) {
        rows.push(AccessRow {
            kind: AccessKind::Grant,
            key: format!("g:{}", g.id),
            text: format!(
                "{} \u{2192} {}",
                short_caller(&g.caller_display),
                g.targets.join(", ")
            ),
            detail: format!(
                "{} \u{b7} {}",
                names(&g.items),
                duration(g.not_after_ms as i64 - now)
            ),
            action_label: "Revoke".into(),
            command: KvCommand::RevokeGrant { id: g.id.clone() },
            imports: vec![],
        });
    }
    for r in o.rules.iter().filter(|r| live_rule(r, now)) {
        rows.push(AccessRow {
            kind: AccessKind::Rule,
            key: format!("r:{}", r.id),
            text: format!(
                "Rule: {} \u{2192} {}",
                r.callers
                    .iter()
                    .map(|c| short_caller(&c.display))
                    .collect::<Vec<_>>()
                    .join(", "),
                rule_targets(r)
            ),
            detail: names(&r.items),
            action_label: "Remove".into(),
            command: KvCommand::RemoveRule { id: r.id.clone() },
            imports: vec![],
        });
    }
    let mut targets: Vec<&str> = Vec::new();
    for d in o.deliveries.iter().filter(|d| live_delivery(d, now)) {
        if !targets.contains(&d.target.as_str()) {
            targets.push(&d.target);
        }
    }
    for t in targets {
        // The soonest expiry; none (until wiped) only when no copy expires.
        let first = o
            .deliveries
            .iter()
            .filter(|d| live_delivery(d, now) && d.target == t)
            .map(|d| d.expires_ms)
            .filter(|&e| e != KV_NO_EXPIRY)
            .min()
            .unwrap_or(KV_NO_EXPIRY);
        rows.push(AccessRow {
            kind: AccessKind::Delivery,
            key: format!("d:{t}"),
            text: format!("Copy in {t}"),
            detail: delivery_lifetime(first, now),
            action_label: "Wipe".into(),
            command: KvCommand::Release { target: t.into() },
            imports: o
                .deliveries
                .iter()
                .filter(|d| live_delivery(d, now) && d.target == t)
                .map(|d| d.import_id.clone())
                .collect(),
        });
    }
    rows
}

/// The page chrome: availability, the kill switch, banners, protection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyvaultPage {
    /// Items can be shown.
    pub ready: bool,
    /// The unavailable heading.
    pub unavailable_title: Option<String>,
    /// The broker's sentence.
    pub message: Option<String>,
    /// "Set up Keyvault" shows.
    pub can_setup: bool,
    /// "Unlock" shows.
    pub can_unlock: bool,
    /// The Disable switch shows (this app is first party, a vault exists).
    pub kill_switch_visible: bool,
    /// The switch can be flipped (unlocked).
    pub kill_switch_enabled: bool,
    /// The kill switch is on.
    pub disabled: bool,
    /// "Keyvault is off. Nothing can be teleported."
    pub disabled_banner: Option<String>,
    /// Sections that failed to load.
    pub partial_errors: Vec<String>,
    /// "Log verified" / "Log tampered at #n".
    pub log_status: Option<String>,
    /// The log check failed.
    pub log_tampered: bool,
    /// Protection facts.
    pub protection: Vec<crate::spaces::sidebar::Fact>,
    /// "Revoke all" shows (more than one live grant).
    pub revoke_all: bool,
    /// Items exist.
    pub has_items: bool,
    /// Search shows (any item exists).
    pub search_visible: bool,
    /// "Never ask again" on the unlock prompt is on (Settings turns it off).
    pub skip_unlock_prompt: bool,
    /// An earlier preview's vault was set aside: one line to show.
    pub reset_notice: Option<String>,
    /// Pending requests (the sidebar badge).
    pub pending_count: u32,
    /// The switch's tooltip (it reads "on" while the Keyvault works).
    pub kill_switch_help: String,
    /// The page's fixed words.
    pub labels: KvLabels,
    /// The setup or unlock form, when one applies.
    pub form: Option<super::credential::KvCredentialForm>,
}

/// The Keyvault's fixed words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KvLabels {
    /// "Deny".
    pub deny: String,
    /// "Review…" (opens the approval sheet).
    pub review: String,
    /// "Cancel".
    pub cancel: String,
    /// "Set up Keyvault".
    pub set_up: String,
    /// "Unlock".
    pub unlock: String,
    /// "Revoke all".
    pub revoke_all: String,
    /// Under the approval sheet.
    pub confirm_note: String,
    /// "Protection".
    pub protection_title: String,
}

/// The Keyvault's fixed words.
pub fn labels() -> KvLabels {
    KvLabels {
        deny: "Deny".into(),
        review: "Review\u{2026}".into(),
        cancel: "Cancel".into(),
        set_up: "Set up Keyvault".into(),
        unlock: "Unlock".into(),
        revoke_all: "Revoke all".into(),
        confirm_note: "Touch ID confirms in the Cua daemon.".into(),
        protection_title: "Protection".into(),
    }
}

/// The banner after setup: the recovery key, shown once.
pub fn recovery_key_text(key: &str) -> String {
    format!("Recovery key, shown once: {key}")
}

/// The page chrome for an overview.
pub fn page(o: &KeyvaultOverview, now: i64) -> KeyvaultPage {
    use crate::spaces::sidebar::Fact;
    let ready = o.availability == "ready";
    let s = o.status.as_ref();
    let disabled = s.is_some_and(|s| s.disabled);
    let unavailable_title = (!ready).then(|| {
        match o.availability.as_str() {
            "not_running" => "The Keyvault is not running",
            "impostor" => "The Cua daemon is not signed by Cua",
            "no_vault" => "No Keyvault yet",
            "locked" => "The Keyvault is locked",
            "not_first_party" => "This app is not signed by Cua",
            _ => "The Keyvault is unavailable",
        }
        .to_string()
    });
    let fact = |l: &str, v: String| Fact {
        label: l.into(),
        value: v,
        copy: None,
        help: None,
        warning: None,
    };
    let protection = s
        .map(|s| {
            vec![
                fact("Touch ID", "Asked by the Cua daemon to widen access".into()),
                fact(
                    "Unlock",
                    if super::credential::passphrase_only(s) {
                        "Passphrase"
                    } else if s.unlock_policy.as_deref() == Some("presence") {
                        "Touch ID"
                    } else {
                        "Mac login"
                    }
                    .into(),
                ),
                fact(
                    "Secure Enclave",
                    "Not used: needs a provisioning-signed build".into(),
                ),
                fact(
                    "This app",
                    if s.caller_first_party {
                        "Verified as Cua"
                    } else {
                        "Not verified"
                    }
                    .into(),
                ),
                fact(
                    "Cua daemon",
                    if o.server_verified {
                        "Signature verified"
                    } else {
                        "Not verified (development build)"
                    }
                    .into(),
                ),
            ]
        })
        .unwrap_or_default();
    let live_grants = o.grants.iter().filter(|g| live_grant(g, now)).count();
    KeyvaultPage {
        ready,
        unavailable_title,
        message: o.message.clone(),
        can_setup: o.availability == "no_vault",
        can_unlock: o.availability == "locked",
        kill_switch_visible: s.is_some_and(|s| s.initialized && s.caller_first_party),
        kill_switch_enabled: s.is_some_and(|s| s.unlocked),
        disabled,
        disabled_banner: disabled.then(|| "Keyvault is off. Nothing can be teleported.".into()),
        partial_errors: o.partial_errors.clone(),
        log_status: o.audit_verification.as_ref().map(|v| {
            if v.ok {
                "Log verified".to_string()
            } else {
                format!(
                    "Log tampered at #{}",
                    v.tampered_line
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "?".into())
                )
            }
        }),
        log_tampered: o.audit_verification.as_ref().is_some_and(|v| !v.ok),
        protection,
        revoke_all: live_grants > 1,
        has_items: !o.items.is_empty(),
        search_visible: !o.items.is_empty(),
        skip_unlock_prompt: s.and_then(|s| s.skip_unlock_prompt).unwrap_or(false),
        reset_notice: s.and_then(|s| s.reset_notice.clone()),
        pending_count: o.pending.len() as u32,
        kill_switch_help: if disabled {
            "Turning it back on asks for Touch ID"
        } else {
            "Stops every teleport, import and approval"
        }
        .into(),
        labels: labels(),
        form: super::credential::credential_form(o),
    }
}

#[cfg(test)]
mod sharing_tests {
    use super::*;

    fn delivery(target: &str, expires_ms: u64, wiped: bool) -> KvDelivery {
        KvDelivery {
            import_id: format!("imp-{target}"),
            target: target.into(),
            provider_id: "chrome".into(),
            items: vec!["i1".into()],
            caller_fp: "fp-agent".into(),
            delivered_ms: 1_000,
            expires_ms,
            wiped,
        }
    }

    #[test]
    fn live_deliveries_are_never_silent() {
        let mut o = KeyvaultOverview::default();
        assert_eq!(sharing_label(&o, 2_000), None);
        o.deliveries = vec![
            delivery("dev-1", 10_000, false),
            delivery("dev-1", 10_000, false),
        ];
        assert_eq!(
            sharing_label(&o, 2_000).as_deref(),
            Some("Keyvault sign-ins live in dev-1")
        );
        o.deliveries.push(delivery("ci-box", 10_000, false));
        assert_eq!(
            sharing_label(&o, 2_000).as_deref(),
            Some("Keyvault sign-ins live in 2 Spaces")
        );
        // Wiped and expired copies are not live.
        o.deliveries = vec![
            delivery("dev-1", 10_000, true),
            delivery("dev-2", 1_500, false),
        ];
        assert_eq!(sharing_label(&o, 2_000), None);
    }

    #[test]
    fn a_copy_without_expiry_stays_live_until_wiped() {
        let o = KeyvaultOverview {
            deliveries: vec![delivery("dev-1", KV_NO_EXPIRY, false)],
            ..KeyvaultOverview::default()
        };
        let far = i64::MAX / 2;
        assert!(live_delivery(&o.deliveries[0], far));
        assert_eq!(
            sharing_label(&o, far).as_deref(),
            Some("Keyvault sign-ins live in dev-1")
        );
        let rows = access_rows(&o, far);
        assert_eq!(rows[0].text, "Copy in dev-1");
        assert_eq!(rows[0].detail, "until you wipe it");
        assert_eq!(rows[0].imports, ["imp-dev-1"]);
        // A wiped one is gone.
        let wiped = KeyvaultOverview {
            deliveries: vec![delivery("dev-1", KV_NO_EXPIRY, true)],
            ..KeyvaultOverview::default()
        };
        assert_eq!(sharing_label(&wiped, 0), None);
        // With auto-wipe on, the soonest expiry is shown.
        let timed = KeyvaultOverview {
            deliveries: vec![
                delivery("dev-1", KV_NO_EXPIRY, false),
                delivery("dev-1", 2_000 + 5 * 60_000, false),
            ],
            ..KeyvaultOverview::default()
        };
        assert_eq!(access_rows(&timed, 2_000)[0].detail, "wiped in 5 min");
        assert_eq!(delivery_lifetime(KV_NO_EXPIRY, 0), "until you wipe it");
    }

    fn space(id: &str, name: &str) -> crate::model::Space {
        let row = crate::model::SpaceRow {
            id: id.into(),
            name: name.into(),
            provider: "local".into(),
            spacesd_version: "0.4.0".into(),
            features: vec![],
            added_at: None,
            os: None,
            os_name: None,
            reachable: true,
            error: None,
            os_pretty_name: None,
            image: None,
            image_digest: None,
            kind: None,
            arch: None,
            host: None,
            host_name: None,
            power: None,
            power_state: None,
            cloud: None,
            cloud_place: None,
            cloud_delete: None,
        };
        crate::spaces::row_to_space(&row, 0)
    }

    #[test]
    fn signed_in_spaces_follow_live_copies_and_dismissal_hides_only_the_notch() {
        let mut o = KeyvaultOverview::default();
        let mut by_id = delivery("local:ci", KV_NO_EXPIRY, false);
        by_id.import_id = "imp-a".into();
        let mut by_name = delivery("dev-1", 10_000, false);
        by_name.import_id = "imp-b".into();
        o.deliveries = vec![by_id, by_name, delivery("gone", 10_000, true)];
        let spaces = [
            space("local:ci", "ci"),
            space("local:dev-1", "dev-1"),
            space("local:gone", "gone"),
        ];
        // Matched by id or by name; a wiped copy signs nothing in.
        assert_eq!(
            signed_in_spaces(&o, 2_000, &[], &spaces),
            ["local:ci", "local:dev-1"]
        );
        // Dismissing one copy hides it from the notch only.
        let dismissed = vec!["imp-b".to_string()];
        assert_eq!(
            signed_in_spaces(&o, 2_000, &dismissed, &spaces),
            ["local:ci"]
        );
        assert_eq!(
            visible_sharing_label(&o, 2_000, &dismissed).as_deref(),
            Some("Keyvault sign-ins live in local:ci")
        );
        let all = vec!["imp-a".to_string(), "imp-b".to_string()];
        assert_eq!(visible_sharing_label(&o, 2_000, &all), None);
        // The Access page still lists (and wipes) both.
        assert_eq!(access_rows(&o, 2_000).len(), 2);
        // A new copy (a new import id) shows again; dismissed ids of copies
        // no longer live are forgotten.
        assert_eq!(prune_dismissed(&o, 2_000, &all), all);
        assert_eq!(prune_dismissed(&o, 20_000, &all), ["imp-a"]);
        // The badge focuses the Space's Access row.
        assert_eq!(
            space_access_key(&o, 2_000, &spaces[1]).as_deref(),
            Some("d:dev-1")
        );
        assert_eq!(space_access_key(&o, 2_000, &spaces[2]), None);
    }

    #[test]
    fn an_unsigned_daemon_is_named_as_such() {
        let o = KeyvaultOverview {
            availability: "impostor".into(),
            ..KeyvaultOverview::default()
        };
        assert_eq!(
            page(&o, 0).unavailable_title.as_deref(),
            Some("The Cua daemon is not signed by Cua")
        );
    }
}
