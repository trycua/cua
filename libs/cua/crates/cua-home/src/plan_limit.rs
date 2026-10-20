// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The relay's plan-limit refusal, in plain words, for every client.
//!
//! The relay answers a NEW live session over the plan's cap with HTTP 429
//! and `x-cua-relay-error: plan_limit_sessions` (or `plan_limit_hours`),
//! plus `x-cua-relay-plan`, `x-cua-relay-used` and `x-cua-relay-limit`.
//! Live sessions are never cut. The host client turns those headers into
//! one sentence ([`PlanLimit::sentence`]); the apps read the same sentence
//! back ([`PlanLimit::parse`]) to show it with an Upgrade button. No
//! dependencies, so the wasm build can use it.

/// The header that names the refusal.
pub const ERROR_HEADER: &str = "x-cua-relay-error";
/// The plan the account is on (`free`, `pro`, `team`).
pub const PLAN_HEADER: &str = "x-cua-relay-plan";
/// What is used now.
pub const USED_HEADER: &str = "x-cua-relay-used";
/// What the plan allows.
pub const LIMIT_HEADER: &str = "x-cua-relay-limit";

/// Which cap was hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Live sessions at once.
    Sessions,
    /// Remote hours this month.
    Hours,
}

/// A refused new session.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanLimit {
    /// The cap.
    pub kind: Kind,
    /// `free`, `pro` or `teams`; other plans keep their own name.
    pub plan: String,
    /// Used now.
    pub used: f64,
    /// Allowed.
    pub limit: f64,
}

fn number(n: f64) -> String {
    let tenths = (n * 10.0).round() / 10.0;
    if tenths.fract() == 0.0 {
        format!("{tenths:.0}")
    } else {
        format!("{tenths:.1}")
    }
}

fn kind_of_plan(plan: &str) -> String {
    match plan.trim().to_ascii_lowercase().as_str() {
        "team" => "teams".into(),
        p => p.to_owned(),
    }
}

/// "the free plan", "Pro", "Teams".
fn plan_name(plan: &str) -> String {
    match plan {
        "free" => "the free plan".into(),
        "" => "your plan".into(),
        p => {
            let mut c = p.chars();
            c.next()
                .map(|f| format!("{}{}", f.to_uppercase(), c.as_str()))
                .unwrap_or_default()
        }
    }
}

fn from_name(name: &str) -> String {
    match name {
        "the free plan" => "free".into(),
        "your plan" => String::new(),
        n => n.to_ascii_lowercase(),
    }
}

impl PlanLimit {
    /// From the relay's headers; none when `error` is not a plan limit or
    /// the numbers are missing.
    pub fn from_headers(
        error: Option<&str>,
        plan: Option<&str>,
        used: Option<&str>,
        limit: Option<&str>,
    ) -> Option<Self> {
        let kind = match error?.trim() {
            "plan_limit_sessions" => Kind::Sessions,
            "plan_limit_hours" => Kind::Hours,
            _ => return None,
        };
        Some(Self {
            kind,
            plan: kind_of_plan(plan.unwrap_or_default()),
            used: used?.trim().parse().ok()?,
            limit: limit?.trim().parse().ok()?,
        })
    }

    fn next_step(&self) -> &'static str {
        match (self.kind, self.plan.as_str()) {
            (Kind::Sessions, "free") => "Close one or upgrade to Pro.",
            (Kind::Sessions, "pro") => "Close one or upgrade to Teams.",
            (Kind::Sessions, "teams") => "Close one or add seats.",
            (Kind::Sessions, _) => "Close one or upgrade your plan.",
            (Kind::Hours, "free") => "Upgrade to Pro for more.",
            (Kind::Hours, "pro") => "Upgrade to Teams for more.",
            (Kind::Hours, "teams") => "Add seats for more.",
            (Kind::Hours, _) => "Upgrade your plan for more.",
        }
    }

    /// "You're using 5 of 5 live sessions on Pro. Close one or upgrade to Teams."
    pub fn sentence(&self) -> String {
        let plan = plan_name(&self.plan);
        let (used, limit) = (number(self.used), number(self.limit));
        match self.kind {
            Kind::Sessions => format!(
                "You're using {used} of {limit} live sessions on {plan}. {}",
                self.next_step()
            ),
            Kind::Hours => format!(
                "You've used {used} of {limit} remote hours this month on {plan}. {}",
                self.next_step()
            ),
        }
    }

    /// Reads [`PlanLimit::sentence`] back out of an error's text (which may
    /// wrap it). None for any other text.
    pub fn parse(text: &str) -> Option<Self> {
        for (marker, kind, tail) in [
            ("You're using ", Kind::Sessions, " live sessions on "),
            ("You've used ", Kind::Hours, " remote hours this month on "),
        ] {
            let Some(i) = text.find(marker) else { continue };
            let rest = &text[i + marker.len()..];
            let Some((counts, after)) = rest.split_once(tail) else {
                continue;
            };
            let Some((used, limit)) = counts.split_once(" of ") else {
                continue;
            };
            let Some((name, _)) = after.split_once(". ") else {
                continue;
            };
            let (Ok(used), Ok(limit)) = (used.parse(), limit.parse()) else {
                continue;
            };
            return Some(Self {
                kind,
                plan: from_name(name),
                used,
                limit,
            });
        }
        None
    }

    /// The account-app page that adds room: Free to Pro, Pro to Teams, a
    /// team to its billing page; `?from=plan_limit` for the purchase funnel.
    pub fn upgrade_url(&self, account_app: &str) -> String {
        let base = account_app.trim_end_matches('/');
        match self.plan.as_str() {
            "free" => format!("{base}/buy/pro?from=plan_limit"),
            "pro" => format!("{base}/buy/teams?from=plan_limit"),
            _ => format!("{base}/billing"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(e: &str, plan: &str, used: &str, limit: &str) -> Option<PlanLimit> {
        PlanLimit::from_headers(Some(e), Some(plan), Some(used), Some(limit))
    }

    #[test]
    fn pro_sessions_in_plain_words() {
        let l = h("plan_limit_sessions", "pro", "5", "5").unwrap();
        assert_eq!(
            l.sentence(),
            "You're using 5 of 5 live sessions on Pro. Close one or upgrade to Teams."
        );
        assert_eq!(
            l.upgrade_url("https://app.cua.ai/"),
            "https://app.cua.ai/buy/teams?from=plan_limit"
        );
    }

    #[test]
    fn free_and_hours() {
        let l = h("plan_limit_sessions", "free", "2", "2").unwrap();
        assert_eq!(
            l.sentence(),
            "You're using 2 of 2 live sessions on the free plan. Close one or upgrade to Pro."
        );
        let l = h("plan_limit_hours", "free", "20.04", "20").unwrap();
        assert_eq!(
            l.sentence(),
            "You've used 20 of 20 remote hours this month on the free plan. Upgrade to Pro for more."
        );
        let l = h("plan_limit_hours", "pro", "200", "200").unwrap();
        assert_eq!(
            l.sentence(),
            "You've used 200 of 200 remote hours this month on Pro. Upgrade to Teams for more."
        );
    }

    #[test]
    fn the_sentence_reads_back_even_when_wrapped() {
        for l in [
            h("plan_limit_sessions", "pro", "5", "5").unwrap(),
            h("plan_limit_sessions", "free", "2", "2").unwrap(),
            h("plan_limit_hours", "team", "400", "400").unwrap(),
            h("plan_limit_hours", "pro", "12.5", "200").unwrap(),
        ] {
            assert_eq!(PlanLimit::parse(&l.sentence()), Some(l.clone()));
            let wrapped = format!("relay: HTTP 429: {}", l.sentence());
            assert_eq!(PlanLimit::parse(&wrapped), Some(l));
        }
        assert_eq!(PlanLimit::parse("no machine image is configured"), None);
    }

    #[test]
    fn other_refusals_are_not_plan_limits() {
        assert!(h("rate_limited", "pro", "5", "5").is_none());
        assert!(
            PlanLimit::from_headers(Some("plan_limit_sessions"), Some("pro"), None, Some("5"))
                .is_none()
        );
        assert!(PlanLimit::from_headers(None, None, None, None).is_none());
    }
}
