//! `cua.control.session/1` — the cross-language conformance surface for the
//! control plane.
//!
//! The transcript slice proved the emulator by making three languages turn the
//! same `.cast` bytes into the same bytes of JSON. This does the same thing
//! for the control plane: [`run_session`] drives the whole typed surface
//! against the [`ScriptedTransport`](crate::client::transport::ScriptedTransport) —
//! a fake backend, so it runs in CI and never touches a live Space — and
//! writes one document describing everything it observed.
//!
//! Three rules, inherited from `framedoc.rs` and kept for the same reasons:
//!
//! 1. **No floats on the wire.** Bytes and durations are integers. A float
//!    formatter difference would make a conformance failure mean nothing.
//! 2. **Key order is written, not sorted by a serializer.** The small ordered
//!    writer below is used rather than `serde_json`, `JSONSerialization` or
//!    `JSON.stringify`, which disagree about ordering, escaping and spacing.
//! 3. **Escaping is spelled out.** Only `"`, `\` and C0 are escaped.
//!
//! The document deliberately includes every honesty flag and both transport
//! seams, so a binding that quietly drops one fails the build rather than a
//! code review.

use std::sync::Arc;

use crate::client::control::Connection;
use crate::client::coverage::coverage;
use crate::client::error::{Result, SpacesError};
use crate::client::model::*;
use crate::client::teleport::TeleportScope;
use crate::client::transport::ScriptedTransport;

pub const SESSION_SCHEMA: &str = "cua.control.session/1";

// ---------------------------------------------------------------------------
// The ordered writer
// ---------------------------------------------------------------------------

/// An ordered JSON value. Deliberately not `serde_json::Value`: the ordering
/// and formatting guarantees are the product here.
#[derive(Clone, Debug)]
pub enum Node {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Array(Vec<Node>),
    Object(Vec<(String, Node)>),
}

impl Node {
    pub fn object(entries: Vec<(&str, Node)>) -> Node {
        Node::Object(
            entries
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect(),
        )
    }

    pub fn string(value: impl Into<String>) -> Node {
        Node::Str(value.into())
    }

    pub fn optional_string(value: &Option<String>) -> Node {
        match value {
            Some(value) => Node::Str(value.clone()),
            None => Node::Null,
        }
    }

    pub fn optional_int(value: Option<i64>) -> Node {
        match value {
            Some(value) => Node::Int(value),
            None => Node::Null,
        }
    }

    pub fn optional_bool(value: Option<bool>) -> Node {
        match value {
            Some(value) => Node::Bool(value),
            None => Node::Null,
        }
    }

    pub fn strings(values: &[String]) -> Node {
        Node::Array(values.iter().map(Node::string).collect())
    }

    /// Two-space indented, newline-terminated. One writer, spelled out, so
    /// that "the bytes are equal" is a statement about the SDK rather than
    /// about a serializer.
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out.push('\n');
        out
    }

    fn write(&self, out: &mut String, depth: usize) {
        let pad = "  ".repeat(depth);
        let inner = "  ".repeat(depth + 1);
        match self {
            Node::Null => out.push_str("null"),
            Node::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Node::Int(value) => out.push_str(&value.to_string()),
            Node::Str(value) => write_string(out, value),
            Node::Array(items) => {
                if items.is_empty() {
                    out.push_str("[]");
                    return;
                }
                out.push_str("[\n");
                for (index, item) in items.iter().enumerate() {
                    out.push_str(&inner);
                    item.write(out, depth + 1);
                    out.push_str(if index + 1 == items.len() {
                        "\n"
                    } else {
                        ",\n"
                    });
                }
                out.push_str(&pad);
                out.push(']');
            }
            Node::Object(entries) => {
                if entries.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push_str("{\n");
                for (index, (key, value)) in entries.iter().enumerate() {
                    out.push_str(&inner);
                    write_string(out, key);
                    out.push_str(": ");
                    value.write(out, depth + 1);
                    out.push_str(if index + 1 == entries.len() {
                        "\n"
                    } else {
                        ",\n"
                    });
                }
                out.push_str(&pad);
                out.push('}');
            }
        }
    }
}

fn write_string(out: &mut String, value: &str) {
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

// ---------------------------------------------------------------------------
// The fixture script
// ---------------------------------------------------------------------------

/// The scripted backend the session runs against, checked in beside the
/// expected document so all three languages drive the same bytes.
pub fn session_script() -> String {
    // Written as a value and serialized by the ordered writer, so the script
    // file itself is deterministic too.
    include_str!("../../../../spaces-contract/fixtures/control/session-script.json").to_string()
}

// ---------------------------------------------------------------------------
// The session
// ---------------------------------------------------------------------------

/// Drive the whole typed control-plane surface against a scripted backend and
/// describe what was observed.
///
/// Every step is a typed call, not a raw tool call, so the document is a
/// statement about the SDK rather than about the script.
pub fn run_session(script_json: &str) -> Result<String> {
    let transport = Arc::new(ScriptedTransport::from_script(script_json)?);
    let connection = Arc::new(Connection::new(transport.clone()));

    let mut sections: Vec<(&str, Node)> = Vec::new();
    sections.push(("schema", Node::string(SESSION_SCHEMA)));

    // -- the tool surface -------------------------------------------------
    sections.push((
        "coverage",
        Node::Array(
            coverage()
                .into_iter()
                .map(|row| {
                    Node::object(vec![
                        ("tool", Node::string(row.tool)),
                        ("sdk_symbol", Node::string(row.sdk_symbol)),
                        ("providers", Node::strings(&row.providers)),
                        ("metering", Node::string(row.metering)),
                    ])
                })
                .collect(),
        ),
    ));

    // -- the honesty apparatus, every flag, in one place ------------------
    let scheduler = scheduler_facts();
    let limits = TransferLimits::conservative_default();
    sections.push((
        "honesty",
        Node::object(vec![
            (
                "scheduler_is_server_backed",
                Node::Bool(scheduler.is_server_backed),
            ),
            (
                "scheduler_missed_slot_policy",
                Node::string(scheduler.missed_slot_policy),
            ),
            (
                "transfer_limits_is_server_published",
                Node::Bool(limits.is_server_published),
            ),
            (
                "transfer_limits_max_file_count",
                Node::Int(i64::from(limits.max_file_count)),
            ),
            (
                "transfer_limits_max_bytes_per_file",
                Node::Int(limits.max_bytes_per_file as i64),
            ),
            (
                "transfer_limits_max_bytes_per_batch",
                Node::Int(limits.max_bytes_per_batch as i64),
            ),
            ("approvals_are_enforced", Node::Bool(APPROVALS_ARE_ENFORCED)),
            (
                "provider_server_backstop",
                Node::Array(
                    [
                        SpaceProvider::Local,
                        SpaceProvider::Fleet,
                        SpaceProvider::Demo,
                        SpaceProvider::Unknown,
                    ]
                    .iter()
                    .map(|provider| {
                        Node::object(vec![
                            ("provider", Node::string(provider.as_str())),
                            (
                                "server_backstop",
                                Node::Bool(provider.capabilities().server_backstop),
                            ),
                        ])
                    })
                    .collect(),
                ),
            ),
            (
                "agent_kinds",
                Node::Array(
                    ["claude-code", "codex", "cursor", "aider"]
                        .iter()
                        .map(|id| {
                            let kind = agent_kind(id);
                            Node::object(vec![
                                ("id", Node::string(kind.id)),
                                ("is_production_ready", Node::Bool(kind.is_production_ready)),
                            ])
                        })
                        .collect(),
                ),
            ),
        ]),
    ));

    // -- spaces -----------------------------------------------------------
    let spaces = connection.spaces()?;
    sections.push((
        "spaces",
        Node::Array(spaces.iter().map(space_node).collect()),
    ));

    let space = connection.attach("local:cua-space-fixture01", true)?;
    let capabilities = space.capabilities();
    sections.push((
        "attached",
        Node::object(vec![
            ("id", Node::string(space.id())),
            ("home", Node::string(space.home())),
            (
                "default_upload_directory",
                Node::string(space.provider().default_upload_directory()),
            ),
            ("capabilities", capabilities_node(&capabilities)),
        ]),
    ));

    // -- attaching to what is not there -----------------------------------
    sections.push((
        "attach_to_absent_space",
        error_node(connection.attach("fleet:not-here", true).err()),
    ));

    // -- windows ----------------------------------------------------------
    let windows = space.windows()?;
    sections.push((
        "windows",
        Node::Array(
            windows
                .iter()
                .map(|window| {
                    Node::object(vec![
                        ("id", Node::string(window.id.clone())),
                        ("app", Node::string(window.app.clone())),
                        ("title", Node::string(window.title.clone())),
                        ("width_px", Node::Int(i64::from(window.width_px))),
                        ("height_px", Node::Int(i64::from(window.height_px))),
                        (
                            "scale_factor_hundredths",
                            Node::Int(i64::from(window.scale_factor_hundredths)),
                        ),
                        ("visible", Node::Bool(window.visible)),
                        (
                            "process_id",
                            Node::optional_int(window.process_id.map(i64::from)),
                        ),
                        (
                            "looks_like_rcdp_target",
                            Node::Bool(window.looks_like_rcdp_target()),
                        ),
                    ])
                })
                .collect(),
        ),
    ));

    // -- streaming endpoint ------------------------------------------------
    let endpoint = space.stream_endpoint(false)?;
    sections.push((
        "stream_endpoint",
        Node::object(vec![
            ("host", Node::string(endpoint.host.clone())),
            ("port", Node::Int(i64::from(endpoint.port))),
            ("driver_port", Node::Int(i64::from(endpoint.driver_port))),
            ("token", Node::string(endpoint.token.clone())),
        ]),
    ));

    // -- in-space services -------------------------------------------------
    let catalog = space.service_catalog(None, None)?;
    sections.push((
        "service_catalog",
        Node::object(vec![
            ("service", Node::string(catalog.service.clone())),
            (
                "tools",
                Node::Array(
                    catalog
                        .tools
                        .iter()
                        .map(|tool| {
                            Node::object(vec![
                                ("name", Node::string(tool.name.clone())),
                                ("summary", Node::string(tool.summary.clone())),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("other_services", Node::strings(&catalog.other_services)),
            (
                "not_ready_warning",
                Node::optional_string(&catalog.not_ready_warning),
            ),
            (
                "is_empty_but_reachable",
                Node::Bool(catalog.is_empty_but_reachable()),
            ),
        ]),
    ));

    let parts = space.call_service_tool("screenshot", Some("cua-driver".into()), "{}")?;
    sections.push((
        "call_service_tool",
        Node::Array(
            parts
                .iter()
                .map(|part| {
                    Node::object(vec![
                        ("kind", Node::string(part.kind.clone())),
                        ("text", Node::string(part.text.clone())),
                        ("mime_type", Node::string(part.mime_type.clone())),
                        ("data_base64_len", Node::Int(part.data_base64.len() as i64)),
                    ])
                })
                .collect(),
        ),
    ));

    // -- agent threads -----------------------------------------------------
    let handle = space.start_agent(&AgentStartRequest {
        agent: "claude-code".into(),
        prompt: "summarise the repository".into(),
        metadata: vec![
            MetadataEntry {
                key: "cua.schedule".into(),
                value: "nightly".into(),
            },
            MetadataEntry {
                key: "bot".into(),
                value: "openkoala".into(),
            },
        ],
        shows_window: false,
        timeout_seconds: Some(900),
    })?;
    sections.push((
        "started_run",
        Node::object(vec![
            ("id", Node::string(handle.id.clone())),
            ("agent", Node::string(handle.agent.clone())),
            (
                "turn_model_kind",
                Node::string(handle.turn_model.kind.clone()),
            ),
            (
                "turn_model_accepts_follow_ups",
                Node::Bool(handle.turn_model.accepts_follow_ups),
            ),
            ("directory", Node::string(handle.directory())),
            (
                "approvals_are_enforced",
                Node::Bool(handle.approvals_are_enforced),
            ),
            ("notes", Node::strings(&handle.notes)),
        ]),
    ));

    // The prompt actually sent, so the metadata carrier is part of the proof.
    let sent_prompt = transport
        .calls()
        .into_iter()
        .find(|(tool, _)| tool == "agent_start")
        .and_then(|(_, arguments)| {
            arguments
                .get("prompt")
                .and_then(|value| value.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default();
    sections.push(("agent_start_prompt_on_the_wire", Node::string(sent_prompt)));

    let status = space.run_status(&handle.id, 200)?;
    sections.push(("run_status", snapshot_node(&status)));

    let events = events_from_snapshot(&status);
    sections.push((
        "run_events",
        Node::Array(
            events
                .iter()
                .map(|event| {
                    Node::object(vec![
                        ("line_index", Node::Int(i64::from(event.line_index))),
                        ("kind", Node::string(event.kind.as_str())),
                        ("text", Node::string(event.text.clone())),
                        (
                            "state",
                            match event.state {
                                Some(state) => Node::string(state.as_str()),
                                None => Node::Null,
                            },
                        ),
                        (
                            "exit_code",
                            Node::optional_int(event.exit_code.map(i64::from)),
                        ),
                        ("is_inferred", Node::Bool(event.is_inferred)),
                    ])
                })
                .collect(),
        ),
    ));

    // The cheap call carries no tail, and never loses the explanation (§22).
    let roster = space.runs()?;
    sections.push((
        "roster",
        Node::Array(roster.iter().map(snapshot_node).collect()),
    ));
    sections.push((
        "roster_round_trips",
        Node::Int(1), // asserted by the Rust test; recorded so a binding sees it
    ));

    let refused = space.send_message(&handle.id, "stop", DeliveryMode::RefuseIfBusy, None)?;
    sections.push(("delivery_refused", delivery_node(&refused)));
    let accepted = space.send_message(&handle.id, "carry on", DeliveryMode::RefuseIfBusy, None)?;
    sections.push(("delivery_accepted", delivery_node(&accepted)));

    let stopped = space.stop_run(&handle.id)?;
    sections.push((
        "stop_outcome",
        Node::object(vec![
            ("stopped", Node::Bool(stopped.stopped)),
            ("alive", Node::optional_bool(stopped.alive)),
            ("reason", Node::string(stopped.reason.clone())),
        ]),
    ));

    let harness = space.harness_capabilities()?;
    sections.push((
        "harness_capabilities",
        Node::object(vec![
            ("statuses", Node::strings(&harness.statuses)),
            (
                "harnesses",
                Node::Array(
                    harness
                        .harnesses
                        .iter()
                        .map(|kind| {
                            Node::object(vec![
                                ("id", Node::string(kind.id.clone())),
                                ("is_production_ready", Node::Bool(kind.is_production_ready)),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "status_classifier",
                Node::string(harness.status_classifier.clone()),
            ),
        ]),
    ));

    sections.push((
        "approve_run",
        error_node(space.approve_run(&handle.id, "allow_once").err()),
    ));

    // -- files -------------------------------------------------------------
    let uploaded = space.upload(
        "/Users/operator/notes.txt",
        &UploadPlacement::collision_safe_default(),
        Some(2_048),
    )?;
    sections.push((
        "upload",
        Node::object(vec![
            ("path", Node::string(uploaded.path.clone())),
            ("name", Node::string(uploaded.name.clone())),
            (
                "byte_count",
                Node::optional_int(uploaded.byte_count.map(|count| count as i64)),
            ),
        ]),
    ));
    sections.push((
        "download",
        Node::string(space.download("~/Downloads/report.pdf", None)?),
    ));
    sections.push((
        "space_write",
        Node::string(space.write_text("hello", "~/greeting.txt")?.path),
    ));
    sections.push(("space_bash", Node::string(space.bash("uname -s")?)));

    // The limit check, before any I/O (§14).
    let limits = TransferLimits {
        max_file_count: 6,
        max_bytes_per_file: 25 * 1024 * 1024,
        max_bytes_per_batch: 30 * 1024 * 1024,
        is_server_published: false,
    };
    let admission = limits.admit(&[
        TransferCandidate {
            name: "small.txt".into(),
            byte_count: 1_024,
        },
        TransferCandidate {
            name: "huge.mov".into(),
            byte_count: 40 * 1024 * 1024,
        },
        TransferCandidate {
            name: "medium.zip".into(),
            byte_count: 29 * 1024 * 1024,
        },
    ]);
    sections.push((
        "transfer_admission",
        Node::object(vec![
            (
                "accepted",
                Node::strings(
                    &admission
                        .accepted
                        .iter()
                        .map(|candidate| candidate.name.clone())
                        .collect::<Vec<_>>(),
                ),
            ),
            (
                "rejected",
                Node::Array(
                    admission
                        .rejected
                        .iter()
                        .map(|rejection| {
                            Node::object(vec![
                                ("name", Node::string(rejection.candidate.name.clone())),
                                ("violation", Node::string(rejection.violation.clone())),
                            ])
                        })
                        .collect(),
                ),
            ),
        ]),
    ));

    // -- operator display --------------------------------------------------
    space.pin_picture_in_picture_on_operator_desktop()?;
    space.unpin_picture_in_picture_from_operator_desktop()?;
    space.open_viewer_on_operator_desktop()?;
    space.stream_window_to_operator_desktop("target-172aad9a")?;
    sections.push((
        "operator_display_calls",
        Node::strings(&[
            "show_space_pip".to_string(),
            "hide_space_pip".to_string(),
            "open_space_viewer".to_string(),
            "stream_space_window".to_string(),
        ]),
    ));

    // -- teleport ----------------------------------------------------------
    let manifest = space.teleport_manifest("claude-code", TeleportScope::Full)?;
    sections.push((
        "teleport_manifest",
        Node::object(vec![
            ("app", Node::string(manifest.app.clone())),
            ("display_name", Node::string(manifest.display_name.clone())),
            ("server_scope", Node::string(manifest.server_scope.clone())),
            (
                "total_estimated_bytes",
                Node::Int(manifest.total_estimated_bytes as i64),
            ),
            (
                "items",
                Node::Array(
                    manifest
                        .items
                        .iter()
                        .map(|item| {
                            Node::object(vec![
                                ("relative_path", Node::string(item.relative_path.clone())),
                                ("label", Node::string(item.label.clone())),
                                ("estimated_bytes", Node::Int(item.estimated_bytes as i64)),
                                ("is_sensitive", Node::Bool(item.is_sensitive)),
                                (
                                    "is_checked_by_default",
                                    Node::Bool(item.is_checked_by_default),
                                ),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("notes", Node::strings(&manifest.notes)),
            (
                "default_selection",
                Node::strings(
                    &manifest
                        .default_selection()
                        .iter()
                        .map(|item| item.relative_path.clone())
                        .collect::<Vec<_>>(),
                ),
            ),
            (
                "login_only_selection",
                Node::strings(
                    &manifest
                        .login_only_selection()
                        .iter()
                        .map(|item| item.relative_path.clone())
                        .collect::<Vec<_>>(),
                ),
            ),
        ]),
    ));

    // The type-gate, in all four of its refusals.
    sections.push((
        "teleport_gate",
        Node::object(vec![
            (
                "unacknowledged_sensitive",
                error_node(
                    manifest
                        .approving(space.id(), &["claude/.credentials.json".to_string()], false)
                        .err(),
                ),
            ),
            (
                "path_not_in_manifest",
                error_node(
                    manifest
                        .approving(space.id(), &["../../etc/passwd".to_string()], true)
                        .err(),
                ),
            ),
            (
                "empty_selection",
                error_node(manifest.approving(space.id(), &[], true).err()),
            ),
            (
                "wrong_space",
                error_node({
                    let elsewhere = manifest
                        .approving(
                            "fleet:elsewhere",
                            &["claude/.credentials.json".to_string()],
                            true,
                        )
                        .expect("minting for another Space is allowed; sending it is not");
                    space.teleport_send(&elsewhere).err()
                }),
            ),
        ]),
    ));

    // The `nil`-selection rule, on the wire.
    let approval = manifest.approving_server_default(space.id(), true)?;
    let receipt = space.teleport_send(&approval)?;
    let include_on_the_wire = transport
        .calls()
        .into_iter()
        .find(|(tool, _)| tool == "teleport_app")
        .map(|(_, arguments)| arguments.get("include").is_some())
        .unwrap_or(true);
    sections.push((
        "teleport_server_default",
        Node::object(vec![
            (
                "uses_server_default",
                Node::Bool(approval.uses_server_default()),
            ),
            (
                "approved_bytes",
                Node::Int(approval.approved_bytes() as i64),
            ),
            ("approved_paths", Node::strings(&approval.approved_paths())),
            (
                "include_key_present_on_the_wire",
                Node::Bool(include_on_the_wire),
            ),
            ("receipt_method", Node::string(receipt.method.clone())),
            (
                "receipt_transferred_paths",
                Node::strings(&receipt.transferred_paths),
            ),
        ]),
    ));

    // -- hotspot -----------------------------------------------------------
    let started = connection.hotspot_start(space.id())?;
    let status = connection.hotspot_status()?;
    let stopped = connection.hotspot_stop()?;
    sections.push((
        "hotspot",
        Node::Array(
            [("start", started), ("status", status), ("stop", stopped)]
                .iter()
                .map(|(label, value)| {
                    Node::object(vec![
                        ("call", Node::string(*label)),
                        ("is_sharing", Node::Bool(value.is_sharing)),
                        ("space", Node::optional_string(&value.space)),
                    ])
                })
                .collect(),
        ),
    ));

    // -- creation: where it runs is an explicit argument ---------------------
    let local = connection.create_space(CreateSpaceOptions {
        on: Some("local".into()),
        name: Some("scratch".into()),
        ..Default::default()
    })?;
    sections.push((
        "create_space_local",
        Node::object(vec![
            ("id", Node::string(local.id())),
            ("metered", Node::Bool(false)),
        ]),
    ));

    // -- the §3 seam, at a typed call site ---------------------------------
    sections.push((
        "tool_failure_is_an_error",
        error_node(space.bash("false").err()),
    ));

    // -- the escape hatch --------------------------------------------------
    sections.push((
        "raw_tool_names",
        Node::strings(&connection.available_tools()?),
    ));

    Ok(Node::Object(
        sections
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect(),
    )
    .to_json())
}

fn space_node(info: &SpaceInfo) -> Node {
    Node::object(vec![
        ("id", Node::string(info.id.clone())),
        ("provider", Node::string(info.provider.as_str())),
        (
            "operating_system",
            Node::string(info.operating_system.clone()),
        ),
        ("state", Node::string(info.state.as_str())),
        ("raw_phase", Node::string(info.raw_phase.clone())),
        ("ip_address", Node::optional_string(&info.ip_address)),
        ("is_ready", Node::Bool(info.is_ready())),
    ])
}

fn capabilities_node(capabilities: &ProviderCapabilities) -> Node {
    Node::object(vec![
        ("provider", Node::string(capabilities.provider.clone())),
        ("agents", Node::Bool(capabilities.agents)),
        ("window_list", Node::Bool(capabilities.window_list)),
        ("upload", Node::Bool(capabilities.upload)),
        ("download", Node::Bool(capabilities.download)),
        ("rcdp_streaming", Node::Bool(capabilities.rcdp_streaming)),
        ("teleport", Node::Bool(capabilities.teleport)),
        ("provisioning", Node::Bool(capabilities.provisioning)),
        ("server_backstop", Node::Bool(capabilities.server_backstop)),
        ("notes", Node::strings(&capabilities.notes)),
    ])
}

fn snapshot_node(snapshot: &RunSnapshot) -> Node {
    Node::object(vec![
        ("id", Node::string(snapshot.id.clone())),
        ("space", Node::string(snapshot.space.clone())),
        ("agent", Node::string(snapshot.agent.clone())),
        ("state", Node::string(snapshot.state.as_str())),
        ("reason", Node::string(snapshot.reason.clone())),
        ("accepts_message", Node::Bool(snapshot.accepts_message)),
        (
            "exit_code",
            Node::optional_int(snapshot.exit_code.map(i64::from)),
        ),
        ("summary", Node::string(snapshot.summary.clone())),
        ("output_tail", Node::optional_string(&snapshot.output_tail)),
        ("output_truncated", Node::Bool(snapshot.output_truncated)),
        ("created_at_ms", Node::optional_int(snapshot.created_at_ms)),
        (
            "reason_is_carried_forward",
            Node::Bool(snapshot.reason_is_carried_forward),
        ),
        ("raw_prompt", Node::optional_string(&snapshot.raw_prompt)),
    ])
}

fn delivery_node(delivery: &Delivery) -> Node {
    Node::object(vec![
        ("run_id", Node::string(delivery.run_id.clone())),
        ("accepted", Node::Bool(delivery.accepted)),
        ("reason", Node::string(delivery.reason.clone())),
        (
            "state_at_refusal",
            match delivery.state_at_refusal {
                Some(state) => Node::string(state.as_str()),
                None => Node::Null,
            },
        ),
    ])
}

/// An error, as a tag and a message. Recorded rather than asserted in prose,
/// so a binding that maps an error to the wrong case fails the comparison.
fn error_node(error: Option<SpacesError>) -> Node {
    match error {
        Some(error) => Node::object(vec![
            ("tag", Node::string(error.tag())),
            ("message", Node::string(error.to_string())),
        ]),
        None => Node::object(vec![
            ("tag", Node::string("NoError")),
            ("message", Node::string("the call succeeded")),
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_is_deterministic() {
        let script = session_script();
        assert_eq!(run_session(&script).unwrap(), run_session(&script).unwrap());
    }

    #[test]
    fn the_session_document_carries_every_honesty_flag() {
        let document = run_session(&session_script()).unwrap();
        for flag in [
            "\"scheduler_is_server_backed\": false",
            "\"transfer_limits_is_server_published\": false",
            "\"approvals_are_enforced\": false",
            "\"server_backstop\": false",
            "\"is_inferred\": false",
            "\"is_production_ready\": true",
        ] {
            assert!(document.contains(flag), "missing {flag} from the session");
        }
        assert!(
            !document.contains("\"is_inferred\": true"),
            "nothing the core produces may be inferred"
        );
    }

    /// The `nil`-selection rule, asserted on the document rather than only in
    /// a unit test, so every language's copy proves it too.
    #[test]
    fn the_session_proves_a_server_default_teleport_sends_no_include() {
        let document = run_session(&session_script()).unwrap();
        assert!(document.contains("\"include_key_present_on_the_wire\": false"));
        assert!(document.contains("\"uses_server_default\": true"));
    }

    #[test]
    fn the_writer_escapes_only_what_it_says_it_does() {
        let node = Node::object(vec![
            ("quote", Node::string("a\"b")),
            ("backslash", Node::string("a\\b")),
            ("control", Node::string("a\u{1}b")),
            ("unicode", Node::string("héllo — 世界")),
            ("slash", Node::string("a/b")),
        ]);
        let json = node.to_json();
        assert!(json.contains("\"a\\\"b\""));
        assert!(json.contains("\"a\\\\b\""));
        assert!(json.contains("\"a\\u0001b\""));
        assert!(json.contains("héllo — 世界"));
        assert!(json.contains("\"a/b\""), "no \\/ escaping");
    }
}
