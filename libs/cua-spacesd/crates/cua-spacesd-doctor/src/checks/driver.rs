// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `driver`: the cua-driver tool registry cua-spacesd links in-process.
//!
//! "cua-driver in the guest" means the linked `cua-driver-core` plus its
//! platform backend (all input goes through it), so these checks ask the
//! running service, not a separate binary. An image that also ships a
//! standalone `cua-driver` gets its own `doctor --json` checked too.

use std::collections::BTreeSet;
use std::time::Duration;

use cua_spacesd_client::diagnose::{Check, Status};
use cua_spacesd_client::pb;
use serde_json::Value;

use crate::{Ctx, Recorder};

/// The MCP contract fixture cua-driver's own CI pins
/// (`libs/cua-driver/compat-fixtures/mcp.json`).
const MCP_CONTRACT: &str = include_str!("../../../../../cua-driver/compat-fixtures/mcp.json");

/// Tools the contract requires.
pub fn required_tools() -> Vec<String> {
    serde_json::from_str::<Value>(MCP_CONTRACT)
        .ok()
        .and_then(|v| {
            v["tools_list"]["required_tools"].as_array().map(|a| {
                a.iter()
                    .filter_map(|t| t.as_str().map(str::to_owned))
                    .collect()
            })
        })
        .unwrap_or_default()
}

/// The linked cua-driver version (from `health_report`), empty when unknown.
pub async fn linked_version(ctx: &Ctx) -> String {
    ctx.driver_version.lock().unwrap().clone()
}

/// Calls a tool; returns its JSON result (structured, or the first JSON/text
/// part parsed as JSON).
pub async fn call_json(
    ctx: &Ctx,
    name: &str,
    args: Value,
    timeout: Duration,
) -> Result<Value, String> {
    let response = ctx
        .client
        .driver()
        .call_tool(pb::CallToolRequest {
            name: name.into(),
            arguments_json: args.to_string(),
            timeout: Some(pbjson_types::Duration {
                seconds: timeout.as_secs() as i64,
                nanos: 0,
            }),
        })
        .await
        .map_err(|s| format!("{} ({:?})", s.message(), s.code()))?
        .into_inner();
    let text = if !response.structured_json.is_empty() {
        response.structured_json.clone()
    } else {
        response
            .content
            .iter()
            .find_map(|c| match &c.content {
                Some(pb::tool_content::Content::Json(j)) => Some(j.clone()),
                Some(pb::tool_content::Content::Text(t)) => Some(t.clone()),
                _ => None,
            })
            .unwrap_or_default()
    };
    if response.is_error {
        return Err(format!(
            "{name} reported an error: {}",
            text.chars().take(300).collect::<String>()
        ));
    }
    serde_json::from_str(&text).map_err(|e| format!("{name} result is not JSON: {e}"))
}

pub async fn run(ctx: &Ctx, rec: &mut Recorder<'_>) {
    if !rec.wants_group("driver") {
        return;
    }
    let claims: &[&str] = &["feature:driver"];
    if !ctx.supports("driver") {
        rec.push(
            Check::new(
                "driver.registry",
                Status::Fail,
                format!("no cua-driver registry: {}", ctx.limitation("driver")),
            )
            .fix("cua-spacesd was started with --no-driver or the platform backend failed to load"),
            claims,
        )
        .await;
        return;
    }

    let listed = tokio::time::timeout(
        Duration::from_secs(20),
        ctx.client.driver().list_tools(pb::ListToolsRequest {}),
    )
    .await;
    let tools = match listed {
        Ok(Ok(r)) => r.into_inner(),
        Ok(Err(status)) => {
            rec.push(
                Check::new(
                    "driver.registry",
                    Status::Fail,
                    format!("ListTools failed: {}", status.message()),
                ),
                claims,
            )
            .await;
            return;
        }
        Err(_) => {
            rec.push(
                Check::new("driver.registry", Status::Fail, "ListTools timed out"),
                claims,
            )
            .await;
            return;
        }
    };
    let names: BTreeSet<&str> = tools.tools.iter().map(|t| t.name.as_str()).collect();
    let missing: Vec<String> = required_tools()
        .into_iter()
        .filter(|t| !names.contains(t.as_str()))
        .collect();
    rec.push(
        Check::new(
            "driver.registry",
            super::verdict(missing.is_empty() && !tools.tools.is_empty()),
            if missing.is_empty() {
                format!(
                    "{} tools, contract version {:?}, every contract-required tool present",
                    tools.tools.len(),
                    tools.contract_version
                )
            } else {
                format!("contract tools missing: {}", missing.join(", "))
            },
        )
        .fact("tools_count", tools.tools.len())
        .fact("contract_version", &tools.contract_version),
        claims,
    )
    .await;

    let hash = cua_spacesd_client::diagnose::tools_sha256(&tools.tools);
    if ctx.manifest.present() {
        let pin = &ctx.manifest.manifest.spacesd.tools_sha256;
        let check = if let Some(o) = super::build::daemon_overlay() {
            super::build::overlaid_pin("driver.tools_sha256", &o)
        } else if pin.is_empty() {
            Check::new(
                "driver.tools_sha256",
                Status::Warn,
                "the manifest pins no tool schema hash",
            )
            .fix("regenerate the manifest with a cua-spacesd that has `build-info`")
        } else if *pin == hash {
            Check::new(
                "driver.tools_sha256",
                Status::Pass,
                "tool schemas match the image's pin",
            )
        } else {
            Check::new(
                "driver.tools_sha256",
                Status::Fail,
                "tool schemas differ from the ones baked into the image",
            )
            .fix("the running registry is not the one the image was built with")
            .fact("manifest", pin)
        };
        rec.push(check.fact("sha256", &hash), &["manifest:spacesd"])
            .await;
    }

    rec.run(
        "driver.health_report",
        claims,
        Duration::from_secs(30),
        async {
            match call_json(
                ctx,
                "health_report",
                serde_json::json!({}),
                Duration::from_secs(25),
            )
            .await
            {
                Ok(report) => {
                    let overall = report["overall"].as_str().unwrap_or("?").to_owned();
                    let version = report["driver_version"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned();
                    *ctx.driver_version.lock().unwrap() = version.clone();
                    // The doctor reaches cua-driver's registry through
                    // cua-spacesd, which links it in-process: TCC grants
                    // attach to cua-spacesd's own bundle, never to
                    // CuaDriver.app, so `bundle_identity` (which expects
                    // com.trycua.driver) is informational here. The tcc_*
                    // and *_capability checks still test the real grants.
                    let failing: Vec<String> = report["checks"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter(|c| c["status"] == "fail" && c["name"] != "bundle_identity")
                                .map(|c| {
                                    format!(
                                        "{}: {}",
                                        c["name"].as_str().unwrap_or("?"),
                                        c["message"]
                                            .as_str()
                                            .or(c["detail"].as_str())
                                            .unwrap_or("")
                                    )
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let mut check = Check::new(
                        "driver.health_report",
                        match overall.as_str() {
                            "ok" => Status::Pass,
                            "degraded" if failing.is_empty() => Status::Pass,
                            "degraded" => Status::Warn,
                            _ => Status::Fail,
                        },
                        if failing.is_empty() {
                            format!("health_report {overall} (cua-driver {version})")
                        } else {
                            format!("health_report {overall}: {}", failing.join("; "))
                        },
                    )
                    .fact("driver_version", &version)
                    .fact("platform", report["platform"].as_str().unwrap_or_default());
                    if let Some(checks) = report["checks"].as_array() {
                        for c in checks.iter().take(32) {
                            check = check.fact(
                                format!("check.{}", c["name"].as_str().unwrap_or("?")),
                                c["status"].as_str().unwrap_or("?"),
                            );
                        }
                    }
                    check
                }
                Err(error) => Check::new("driver.health_report", Status::Fail, error),
            }
        },
    )
    .await;

    if ctx.manifest.present() {
        let pin = ctx.manifest.manifest.spacesd.cua_driver_version.clone();
        let linked = linked_version(ctx).await;
        if let Some(o) = super::build::daemon_overlay() {
            rec.push(
                super::build::overlaid_pin("driver.version", &o),
                &["manifest:spacesd"],
            )
            .await;
        } else if !pin.is_empty() && !linked.is_empty() {
            rec.push(
                Check::new(
                    "driver.version",
                    super::verdict(pin == linked),
                    format!("linked cua-driver {linked}, image pins {pin}"),
                ),
                &["manifest:spacesd"],
            )
            .await;
        }
    }

    rec.run("driver.standalone", &[], Duration::from_secs(30), async {
        if !crate::sys::on_path("cua-driver") {
            return Check::new(
                "driver.standalone",
                Status::Skip,
                "no standalone cua-driver binary (the registry is linked into cua-spacesd)",
            )
            .skip_reason("not_applicable");
        }
        match crate::sys::run_local(
            "cua-driver",
            &["doctor", "--json"],
            &[],
            Duration::from_secs(25),
        )
        .await
        {
            Ok(out) => match serde_json::from_str::<Value>(&out.stdout) {
                Ok(json) => {
                    let ok = json["ok"].as_bool().unwrap_or(false);
                    Check::new(
                        "driver.standalone",
                        super::verdict(ok),
                        format!(
                            "cua-driver doctor ok={ok} ({} probes)",
                            json["probes"].as_array().map(|a| a.len()).unwrap_or(0)
                        ),
                    )
                }
                Err(error) => Check::new(
                    "driver.standalone",
                    Status::Fail,
                    format!("cua-driver doctor --json printed no JSON: {error}"),
                ),
            },
            Err(error) => Check::new(
                "driver.standalone",
                Status::Fail,
                format!("cua-driver doctor: {error}"),
            ),
        }
    })
    .await;
}

#[cfg(test)]
mod tests {
    #[test]
    fn contract_fixture_lists_required_tools() {
        let tools = super::required_tools();
        assert!(tools.contains(&"click".to_owned()), "{tools:?}");
        assert!(tools.len() >= 5);
    }
}
