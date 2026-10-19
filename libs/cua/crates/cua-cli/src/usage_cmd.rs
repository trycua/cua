//! `cua usage`: your remote use this month. A live session is a
//! connection to one of your remote machines through the hosted relay
//! (viewing, a terminal, an agent); relay hours, shown as "remote hours",
//! are the time with at least one. A computer used from itself never goes
//! through the relay, and nothing stops at the limit: the relay only says
//! it was reached.

use crate::util::line;
use cua_host::{AccountTokens, RelayClient, RelayUsage};
use cua_sdk::CuaError;
use std::io::Write;
use std::path::Path;

/// `cua usage` as the signed-in account, on `relay` (default: as `cua devices`).
pub async fn run(
    relay: Option<String>,
    home: &Path,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let client =
        RelayClient::new(&crate::host::relay_url(relay, home)).map_err(crate::host::host_err)?;
    run_with(&client, &crate::host::SessionTokens, json, out).await
}

/// Reads the allowance from `client` with `tokens` and prints it.
pub async fn run_with(
    client: &RelayClient,
    tokens: &dyn AccountTokens,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let err = crate::host::host_err;
    let token = tokens.access_token().await.map_err(err)?;
    let usage = client.usage(&token).await.map_err(err)?;
    if json {
        let value = match &usage {
            Some(u) => serde_json::to_value(u).unwrap_or_default(),
            None => serde_json::json!({ "metered": false }),
        };
        line(out, value.to_string());
        return Ok(0);
    }
    match usage {
        Some(u) if u.metered => print(&u, out),
        _ => line(out, "Your account has no limit on remote use."),
    }
    Ok(0)
}

fn print(u: &RelayUsage, out: &mut dyn Write) {
    let of = |used: String, limit: Option<String>| match limit {
        Some(l) => format!("{used} of {l}"),
        None => format!("{used} (no limit)"),
    };
    line(
        out,
        format!(
            "Plan: {}{}",
            if u.plan.is_empty() { "-" } else { &u.plan },
            if u.period.is_empty() {
                String::new()
            } else {
                format!(" ({})", u.period)
            }
        ),
    );
    line(
        out,
        format!(
            "Live sessions: {}",
            of(
                u.live_sessions.to_string(),
                u.live_sessions_limit.map(|l| l.to_string())
            )
        ),
    );
    line(
        out,
        format!(
            "Remote hours: {} this month",
            of(hours(u.hours_used), u.hours_limit.map(hours))
        ),
    );
    line(out, REMOTE_HOURS_MEANING);
    if let Some(team) = &u.team {
        line(
            out,
            format!(
                "Team pool: {}, {} seat{} (your hours: {})",
                team.id,
                team.seats,
                if team.seats == 1 { "" } else { "s" },
                hours(u.my_hours_used)
            ),
        );
    }
    if u.soft_limit_reached {
        let upgrade = u
            .upgrade_url
            .as_deref()
            .map(|url| format!(" Upgrade: {url}"))
            .unwrap_or_default();
        line(
            out,
            format!(
                "You've reached your plan's remote limit for this month. Your Spaces keep working.{upgrade}"
            ),
        );
    }
}

/// What remote hours are (the apps' words: `relay_usage::REMOTE_HOURS_MEANING`
/// in cua-spaces-app-core).
const REMOTE_HOURS_MEANING: &str = "Remote hours: time spent using your computers from somewhere else (your laptop, phone or an agent on another computer). Using a computer from itself is free.";

/// `20` or `3.4`.
fn hours(h: f64) -> String {
    let tenths = (h * 10.0).round() / 10.0;
    if tenths.fract() == 0.0 {
        format!("{tenths:.0}")
    } else {
        format!("{tenths:.1}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_host::StaticToken;
    use cua_host::testing::FakeRelay;

    async fn text(relay: &FakeRelay, json: bool) -> String {
        let client = RelayClient::new(&relay.url).unwrap();
        let mut out = Vec::new();
        run_with(&client, &StaticToken("acct".into()), json, &mut out)
            .await
            .unwrap();
        String::from_utf8(out).unwrap()
    }

    #[tokio::test]
    async fn prints_the_allowance_and_the_upgrade_past_it() {
        let relay = FakeRelay::start().await;
        relay.add_account("acct", "user-1", None);
        assert_eq!(
            text(&relay, false).await.trim(),
            "Your account has no limit on remote use."
        );
        assert_eq!(text(&relay, true).await.trim(), r#"{"metered":false}"#);

        let mut usage = serde_json::json!({
            "metered": true, "period": "2026-10", "plan": "free",
            "live_sessions": 1, "live_sessions_limit": 2, "hours_used": 6.5, "hours_limit": 20,
            "my_hours_used": 6.5, "soft_limit_reached": false, "reasons": [],
            "upgrade_url": "https://cua.ai/pricing"
        });
        relay.set_usage(Some(usage.clone()));
        let t = text(&relay, false).await;
        assert!(t.contains("Plan: free (2026-10)"), "{t}");
        assert!(t.contains("Live sessions: 1 of 2"), "{t}");
        assert!(t.contains("Remote hours: 6.5 of 20 this month"), "{t}");
        assert!(!t.contains("Team pool") && !t.contains("reached"), "{t}");

        usage["team"] = serde_json::json!({ "id": "acme", "seats": 3 });
        usage["hours_used"] = serde_json::json!(21);
        usage["soft_limit_reached"] = serde_json::json!(true);
        usage["reasons"] = serde_json::json!(["hours"]);
        relay.set_usage(Some(usage));
        let t = text(&relay, false).await;
        assert!(t.contains("Remote hours: 21 of 20 this month"), "{t}");
        assert!(
            t.contains("Team pool: acme, 3 seats (your hours: 6.5)"),
            "{t}"
        );
        assert!(
            t.contains("You've reached your plan's remote limit for this month. Your Spaces keep working. Upgrade: https://cua.ai/pricing"),
            "{t}"
        );
        let v: serde_json::Value = serde_json::from_str(&text(&relay, true).await).unwrap();
        assert_eq!(v["soft_limit_reached"], true);
        assert_eq!(v["team"]["seats"], 3);
    }
}
