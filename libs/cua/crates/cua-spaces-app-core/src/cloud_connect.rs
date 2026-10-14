// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! "Connect a cloud": the sheet that connects the user's own AWS, Google
//! Cloud or Modal account for Spaces (`cloud_test`, `cloud_connect`).
//!
//! Plain data in (the providers `cloud_status` lists) and plain data out
//! (one line per provider with a found mark when this machine has its CLI
//! sign-in, the region, project or environment field, what that provider
//! touches, the checks a Test ran, and the request to run). Test creates
//! nothing; Connect runs the same checks first. Credentials are never
//! typed here: each cloud uses its own CLI sign-in.
//!
//! "No cloud" is always offered alongside the real providers. Picking it
//! never issues a `cloud_connect` or `cloud_disconnect` call: it just ends
//! the flow, the same as cancelling, so a cloud that is already connected
//! is left exactly as it is (no silent disconnect).

use serde::{Deserialize, Serialize};

/// The pseudo-provider id for "No cloud" (a row `cloud_status` never
/// lists; picking it never calls `cloud_connect` or `cloud_disconnect`).
pub const NONE_PROVIDER: &str = "none";

/// One provider as `cloud_status` lists it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudProviderInput {
    /// `aws`, `gcp`, `modal`.
    pub name: String,
    /// `AWS`, `Google Cloud`, `Modal`.
    pub title: String,
    /// Connected already.
    #[serde(default)]
    pub connected: bool,
    /// This machine has its CLI sign-in (or config).
    #[serde(default)]
    pub found: bool,
    /// Where ("~/.aws profile default"), never a secret.
    #[serde(default)]
    pub source: String,
    /// AWS or Modal profile (the connected or detected one).
    #[serde(default)]
    pub profile: String,
    /// Region.
    #[serde(default)]
    pub region: String,
    /// GCP project.
    #[serde(default)]
    pub project: String,
    /// Modal environment.
    #[serde(default)]
    pub environment: String,
    /// "AWS \u{b7} us-west-2".
    #[serde(default)]
    pub label: String,
}

/// Everything the sheet reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudConnectInput {
    /// The providers this build supports.
    #[serde(default)]
    pub providers: Vec<CloudProviderInput>,
}

/// One check of a test (`cloud_test` or the checks `cloud_connect` ran).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudCheckInput {
    /// `credentials`, `permissions`, `quota`, ...
    pub name: String,
    /// Passed.
    pub ok: bool,
    /// What was found, or what to fix.
    #[serde(default)]
    pub detail: String,
}

/// Where the sheet is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "kebab-case")]
pub enum CloudConnectPhase {
    /// Nothing running.
    #[default]
    Idle,
    /// `cloud_test` is running.
    Testing,
    /// A test finished.
    Tested {
        /// Every check passed.
        ok: bool,
        /// The account, project or workspace reached.
        account: String,
        /// The checks.
        checks: Vec<CloudCheckInput>,
    },
    /// `cloud_connect` is running.
    Connecting,
    /// Connected (the shell closes the sheet and refreshes).
    Connected {
        /// "AWS \u{b7} us-west-2".
        label: String,
    },
    /// A call failed.
    Failed {
        /// The error, as the SDK words it.
        error: String,
    },
}

/// The sheet's state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudConnectState {
    /// The chosen provider (`aws`); `None`: the first with credentials.
    pub selected: Option<String>,
    /// The region, project or environment as typed (empty: the default).
    pub value: String,
    /// The profile as typed (empty: the default).
    pub profile: String,
    /// Also make it the default location.
    pub make_default: bool,
    /// Where it is.
    pub phase: CloudConnectPhase,
}

/// An input to the sheet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum CloudConnectAction {
    /// A provider row.
    Select {
        /// `aws`, `gcp`, `modal`.
        name: String,
    },
    /// The region, project or environment field.
    SetValue {
        /// Text.
        text: String,
    },
    /// The profile field.
    SetProfile {
        /// Text.
        text: String,
    },
    /// "Make default".
    SetMakeDefault {
        /// On.
        on: bool,
    },
    /// Test pressed (the shell runs the view's request).
    Test,
    /// The test finished.
    Tested {
        /// Every check passed.
        ok: bool,
        /// The account reached.
        #[serde(default)]
        account: String,
        /// The checks.
        #[serde(default)]
        checks: Vec<CloudCheckInput>,
    },
    /// Connect pressed (the shell runs the view's request).
    Connect,
    /// Connected.
    Connected {
        /// The connected cloud's label.
        label: String,
    },
    /// A call failed.
    Failed {
        /// The SDK's error.
        error: String,
    },
}

/// Where Spaces go in the account (`cloud_test` / `cloud_connect`
/// arguments; only names, never a credential).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudTargetArgs {
    /// `aws`, `gcp`, `modal`.
    pub provider: String,
    /// AWS or Modal profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// AWS region.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// GCP project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Modal environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
}

/// The call the shell runs now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CloudConnectRequest {
    /// `cloud_test`.
    Test {
        /// Where.
        target: CloudTargetArgs,
    },
    /// `cloud_connect`.
    Connect {
        /// Where.
        target: CloudTargetArgs,
        /// Also `default.on`.
        make_default: bool,
    },
}

/// One provider row: one line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudProviderRow {
    /// `aws`.
    pub id: String,
    /// `AWS`.
    pub title: String,
    /// The one line after the title ("Connected, AWS \u{b7} us-west-2",
    /// "~/.aws profile default", "No sign-in found").
    pub detail: String,
    /// This machine has its sign-in (the found mark).
    pub found: bool,
    /// Chosen.
    pub selected: bool,
}

/// A text field of the sheet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudField {
    /// `region`, `project`, `environment`, `profile`.
    pub id: String,
    /// "Region".
    pub label: String,
    /// The default the provider uses when it is empty.
    pub placeholder: String,
    /// As typed.
    pub value: String,
}

/// One line of the checks list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudCheckRow {
    /// Passed.
    pub ok: bool,
    /// "credentials: account 418638388952".
    pub text: String,
}

/// The sheet as drawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudConnectView {
    /// "Connect a cloud".
    pub title: String,
    /// The providers.
    pub rows: Vec<CloudProviderRow>,
    /// The region, project or environment field.
    pub field: Option<CloudField>,
    /// The profile field (AWS, Modal).
    pub profile_field: Option<CloudField>,
    /// The checks the last test ran.
    pub checks: Vec<CloudCheckRow>,
    /// One line under the checks ("Ready. Nothing was created.").
    pub result: Option<String>,
    /// "What Cua will touch": what the selected provider creates, that
    /// only Cua's own tagged resources are ever touched, how cost and
    /// auto-stop work, and the command to remove everything. Empty when
    /// no real provider is chosen (including "No cloud").
    pub touches: Vec<String>,
    /// "Make default".
    pub make_default_label: String,
    /// Its value.
    pub make_default: bool,
    /// "Test" / "Testing\u{2026}".
    pub test_label: String,
    /// Test can be pressed.
    pub can_test: bool,
    /// Test's tooltip.
    pub test_help: String,
    /// "Connect" / "Connecting\u{2026}".
    pub connect_label: String,
    /// Connect can be pressed.
    pub can_connect: bool,
    /// "Cancel".
    pub cancel_label: String,
    /// The last error.
    pub error: Option<String>,
    /// Connected: close the sheet and refresh.
    pub done: bool,
    /// The call to run now (while testing or connecting).
    pub request: Option<CloudConnectRequest>,
}

/// `cloud_status` as the SDK returns it (its JSON), read leniently.
#[derive(Debug, Clone, Default, Deserialize)]
struct StatusWire {
    #[serde(default)]
    default_on: String,
    #[serde(default)]
    providers: Vec<ProviderWire>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct CredentialsWire {
    #[serde(default)]
    found: bool,
    #[serde(default)]
    source: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct KindWire {
    #[serde(default)]
    image: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    supported: bool,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    machine_type: String,
    #[serde(default)]
    usd_per_hour: f64,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ProviderWire {
    #[serde(default)]
    name: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    connected: bool,
    #[serde(default)]
    credentials: CredentialsWire,
    #[serde(default)]
    profile: String,
    #[serde(default)]
    region: String,
    #[serde(default)]
    project: String,
    #[serde(default)]
    environment: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    ttl_hours: u32,
    #[serde(default)]
    kinds: Vec<KindWire>,
}

fn status_of(status: &serde_json::Value) -> StatusWire {
    serde_json::from_value(status.clone()).unwrap_or_default()
}

/// The sheet's input from a `cloud_status` result (its JSON).
pub fn connect_input_from_status(status: &serde_json::Value) -> CloudConnectInput {
    CloudConnectInput {
        providers: status_of(status)
            .providers
            .into_iter()
            .map(|p| CloudProviderInput {
                name: p.name,
                title: p.title,
                connected: p.connected,
                found: p.credentials.found,
                source: p.credentials.source,
                profile: p.profile,
                region: p.region,
                project: p.project,
                environment: p.environment,
                label: p.label,
            })
            .collect(),
    }
}

/// The New Space wizard's connected clouds from a `cloud_status` result.
pub fn connected_clouds_from_status(
    status: &serde_json::Value,
) -> Vec<crate::wizard::ConnectedCloud> {
    let s = status_of(status);
    s.providers
        .into_iter()
        .filter(|p| p.connected)
        .map(|p| crate::wizard::ConnectedCloud {
            is_default: s.default_on == p.name,
            label: if p.label.is_empty() {
                p.title.clone()
            } else {
                p.label
            },
            name: p.name,
            title: p.title,
            ttl_hours: p.ttl_hours,
            offers: p
                .kinds
                .into_iter()
                .map(|k| crate::wizard::CloudOffer {
                    image: k.image,
                    kind: k.kind,
                    supported: k.supported,
                    reason: k.reason,
                    machine_type: k.machine_type,
                    usd_per_hour: k.usd_per_hour,
                })
                .collect(),
        })
        .collect()
}

/// Whether `on` (a `default.on` value) names one of the user's clouds.
pub fn is_cloud_word(on: &str) -> bool {
    matches!(on, "aws" | "gcp" | "modal")
}

/// The sheet's first state.
pub fn cloud_connect_initial() -> CloudConnectState {
    CloudConnectState::default()
}

/// "No cloud" is explicitly selected (never falls back to auto-select).
fn is_none_selected(state: &CloudConnectState) -> bool {
    state.selected.as_deref() == Some(NONE_PROVIDER)
}

fn selected<'a>(
    input: &'a CloudConnectInput,
    state: &CloudConnectState,
) -> Option<&'a CloudProviderInput> {
    let auto = || {
        input
            .providers
            .iter()
            .find(|p| p.found && !p.connected)
            .or_else(|| input.providers.iter().find(|p| p.found))
            .or_else(|| input.providers.first())
    };
    match state.selected.as_deref() {
        Some(NONE_PROVIDER) => None,
        Some(n) => input.providers.iter().find(|p| p.name == n).or_else(auto),
        None => auto(),
    }
}

/// "What Cua will touch" for `name`, from what `cua-byoc` actually
/// creates (see `libs/cua/crates/cua-byoc/src/{aws,gcp,modal}.rs`).
/// Empty for a name this build does not know.
fn touches_of(name: &str) -> Vec<String> {
    let common = "Only resources Cua creates are touched; they are tagged and recorded, \
                  and nothing you already had is changed or deleted.";
    match name {
        "aws" => vec![
            "Creates one EC2 instance per sandbox, plus one shared security group with no \
             inbound rules. No key pairs, no IAM roles."
                .into(),
            common.into(),
            "Costs are billed to your AWS account. Each sandbox ends itself after its time \
             limit (8 hours by default)."
                .into(),
            "Remove everything Cua created with `cua cloud sweep aws --all --delete`.".into(),
        ],
        "gcp" => vec![
            "Creates one Compute Engine VM per sandbox, plus one dedicated network with no \
             firewall rules. No service account is attached."
                .into(),
            common.into(),
            "Costs are billed to your Google Cloud project. Each VM deletes itself after its \
             run-time limit."
                .into(),
            "Remove everything Cua created with `cua cloud sweep gcp --all --delete`.".into(),
        ],
        "modal" => vec![
            "Runs sandboxes in the `cua-sandboxes` app, in the environment you choose.".into(),
            common.into(),
            "Costs are billed to your Modal workspace. Each sandbox is deleted automatically \
             after its time limit (up to 24 hours)."
                .into(),
            "Remove everything Cua created with `cua cloud sweep modal --all --delete`.".into(),
        ],
        _ => vec![],
    }
}

/// The field a provider asks for: `(id, label, the provider's current)`.
fn field_of(p: &CloudProviderInput) -> (&'static str, &'static str, &str) {
    match p.name.as_str() {
        "gcp" => ("project", "Project", &p.project),
        "modal" => ("environment", "Environment", &p.environment),
        _ => ("region", "Region", &p.region),
    }
}

fn has_profile(p: &CloudProviderInput) -> bool {
    matches!(p.name.as_str(), "aws" | "modal")
}

fn target(p: &CloudProviderInput, state: &CloudConnectState) -> CloudTargetArgs {
    let value = Some(state.value.trim().to_string()).filter(|v| !v.is_empty());
    let (id, _, _) = field_of(p);
    let profile =
        Some(state.profile.trim().to_string()).filter(|v| !v.is_empty() && has_profile(p));
    CloudTargetArgs {
        provider: p.name.clone(),
        profile,
        region: value.clone().filter(|_| id == "region"),
        project: value.clone().filter(|_| id == "project"),
        environment: value.filter(|_| id == "environment"),
    }
}

fn busy(state: &CloudConnectState) -> bool {
    matches!(
        state.phase,
        CloudConnectPhase::Testing | CloudConnectPhase::Connecting
    )
}

/// Applies `action`.
pub fn cloud_connect_reduce(
    input: &CloudConnectInput,
    state: &CloudConnectState,
    action: &CloudConnectAction,
) -> CloudConnectState {
    let mut s = state.clone();
    let has_provider = selected(input, &s).is_some();
    let has_choice = has_provider || is_none_selected(&s);
    match action {
        CloudConnectAction::Select { name } if !busy(&s) => {
            if name == NONE_PROVIDER || input.providers.iter().any(|p| &p.name == name) {
                s.selected = Some(name.clone());
                s.value.clear();
                s.profile.clear();
                s.phase = CloudConnectPhase::Idle;
            }
        }
        CloudConnectAction::SetValue { text } if !busy(&s) => {
            s.value = text.clone();
            s.phase = CloudConnectPhase::Idle;
        }
        CloudConnectAction::SetProfile { text } if !busy(&s) => {
            s.profile = text.clone();
            s.phase = CloudConnectPhase::Idle;
        }
        CloudConnectAction::SetMakeDefault { on } => s.make_default = *on,
        CloudConnectAction::Test if has_provider && !busy(&s) => {
            s.phase = CloudConnectPhase::Testing
        }
        CloudConnectAction::Tested {
            ok,
            account,
            checks,
        } if s.phase == CloudConnectPhase::Testing => {
            s.phase = CloudConnectPhase::Tested {
                ok: *ok,
                account: account.clone(),
                checks: checks.clone(),
            }
        }
        // "No cloud" ends the flow on the spot: no request ever goes out,
        // so nothing a cloud already has (or an already-connected cloud)
        // is ever touched or disconnected.
        CloudConnectAction::Connect if is_none_selected(&s) && !busy(&s) => {
            s.phase = CloudConnectPhase::Connected {
                label: "No cloud".into(),
            }
        }
        CloudConnectAction::Connect if has_choice && !busy(&s) => {
            s.phase = CloudConnectPhase::Connecting
        }
        CloudConnectAction::Connected { label } if s.phase == CloudConnectPhase::Connecting => {
            s.phase = CloudConnectPhase::Connected {
                label: label.clone(),
            }
        }
        CloudConnectAction::Failed { error } if busy(&s) => {
            s.phase = CloudConnectPhase::Failed {
                error: error.clone(),
            }
        }
        _ => {}
    }
    s
}

/// The sheet as drawn.
pub fn cloud_connect_view(
    input: &CloudConnectInput,
    state: &CloudConnectState,
) -> CloudConnectView {
    let chosen = selected(input, state);
    let none_selected = is_none_selected(state);
    let mut rows: Vec<CloudProviderRow> = input
        .providers
        .iter()
        .map(|p| CloudProviderRow {
            id: p.name.clone(),
            title: p.title.clone(),
            detail: if p.connected && !p.label.is_empty() {
                format!("Connected, {}", p.label)
            } else if p.found && !p.source.is_empty() {
                p.source.clone()
            } else if p.found {
                "Sign-in found".into()
            } else {
                "No sign-in found".into()
            },
            found: p.found,
            selected: chosen.is_some_and(|c| c.name == p.name),
        })
        .collect();
    rows.push(CloudProviderRow {
        id: NONE_PROVIDER.into(),
        title: "No cloud".into(),
        detail: if input.providers.iter().any(|p| p.connected) {
            "Leaves your connected cloud as is.".into()
        } else {
            "Don't connect a cloud.".into()
        },
        found: false,
        selected: none_selected,
    });
    let field = chosen.map(|p| {
        let (id, label, current) = field_of(p);
        CloudField {
            id: id.into(),
            label: label.into(),
            placeholder: current.to_string(),
            value: state.value.clone(),
        }
    });
    let profile_field = chosen.filter(|p| has_profile(p)).map(|p| CloudField {
        id: "profile".into(),
        label: "Profile".into(),
        placeholder: if p.profile.is_empty() {
            "default".into()
        } else {
            p.profile.clone()
        },
        value: state.profile.clone(),
    });
    let (checks, result) = match &state.phase {
        CloudConnectPhase::Tested {
            ok,
            account,
            checks,
        } => (
            checks
                .iter()
                .map(|c| CloudCheckRow {
                    ok: c.ok,
                    text: if c.detail.is_empty() {
                        c.name.clone()
                    } else {
                        format!("{}: {}", c.name, c.detail)
                    },
                })
                .collect(),
            Some(if *ok {
                if account.is_empty() {
                    "Ready. Nothing was created.".to_string()
                } else {
                    format!("Ready: {account}. Nothing was created.")
                }
            } else {
                "Not ready: fix what failed. Nothing was created.".to_string()
            }),
        ),
        _ => (vec![], None),
    };
    let working = busy(state);
    let request = chosen.and_then(|p| match state.phase {
        CloudConnectPhase::Testing => Some(CloudConnectRequest::Test {
            target: target(p, state),
        }),
        CloudConnectPhase::Connecting => Some(CloudConnectRequest::Connect {
            target: target(p, state),
            make_default: state.make_default,
        }),
        _ => None,
    });
    CloudConnectView {
        title: "Connect a cloud".into(),
        rows,
        field,
        profile_field,
        checks,
        result,
        touches: chosen.map(|p| touches_of(&p.name)).unwrap_or_default(),
        make_default_label: "Make default".into(),
        make_default: state.make_default,
        test_label: if state.phase == CloudConnectPhase::Testing {
            "Testing\u{2026}"
        } else {
            "Test"
        }
        .into(),
        can_test: chosen.is_some() && !working,
        test_help: "Checks the account without creating anything.".into(),
        connect_label: if state.phase == CloudConnectPhase::Connecting {
            "Connecting\u{2026}"
        } else if none_selected {
            "Done"
        } else {
            "Connect"
        }
        .into(),
        can_connect: (chosen.is_some() || none_selected) && !working,
        cancel_label: "Cancel".into(),
        error: match &state.phase {
            CloudConnectPhase::Failed { error } => Some(error.clone()),
            _ => None,
        },
        done: matches!(state.phase, CloudConnectPhase::Connected { .. }),
        request,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> CloudConnectInput {
        CloudConnectInput {
            providers: vec![
                CloudProviderInput {
                    name: "aws".into(),
                    title: "AWS".into(),
                    found: true,
                    source: "~/.aws profile default".into(),
                    profile: "default".into(),
                    region: "us-west-2".into(),
                    label: "AWS \u{b7} us-west-2".into(),
                    ..Default::default()
                },
                CloudProviderInput {
                    name: "gcp".into(),
                    title: "Google Cloud".into(),
                    ..Default::default()
                },
            ],
        }
    }

    fn run(actions: &[CloudConnectAction]) -> CloudConnectState {
        let i = input();
        actions.iter().fold(cloud_connect_initial(), |s, a| {
            cloud_connect_reduce(&i, &s, a)
        })
    }

    #[test]
    fn the_found_provider_is_chosen_and_test_creates_nothing() {
        let v = cloud_connect_view(&input(), &cloud_connect_initial());
        assert!(v.rows[0].selected && v.rows[0].found);
        assert_eq!(v.rows[1].detail, "No sign-in found");
        assert_eq!(v.field.as_ref().unwrap().placeholder, "us-west-2");
        assert!(v.request.is_none());
        let s = run(&[
            CloudConnectAction::SetValue {
                text: " eu-west-1 ".into(),
            },
            CloudConnectAction::SetProfile {
                text: "sandbox".into(),
            },
            CloudConnectAction::Test,
        ]);
        let v = cloud_connect_view(&input(), &s);
        assert_eq!(v.test_label, "Testing\u{2026}");
        assert!(!v.can_connect);
        assert_eq!(
            v.request,
            Some(CloudConnectRequest::Test {
                target: CloudTargetArgs {
                    provider: "aws".into(),
                    profile: Some("sandbox".into()),
                    region: Some("eu-west-1".into()),
                    ..Default::default()
                }
            })
        );
    }

    #[test]
    fn a_passed_test_then_connect_is_done() {
        let s = run(&[
            CloudConnectAction::Test,
            CloudConnectAction::Tested {
                ok: true,
                account: "418638388952".into(),
                checks: vec![CloudCheckInput {
                    name: "credentials".into(),
                    ok: true,
                    detail: "account 418638388952".into(),
                }],
            },
        ]);
        let v = cloud_connect_view(&input(), &s);
        assert_eq!(v.checks[0].text, "credentials: account 418638388952");
        assert_eq!(
            v.result.as_deref(),
            Some("Ready: 418638388952. Nothing was created.")
        );
        let s = cloud_connect_reduce(
            &input(),
            &s,
            &CloudConnectAction::SetMakeDefault { on: true },
        );
        let s = cloud_connect_reduce(&input(), &s, &CloudConnectAction::Connect);
        let v = cloud_connect_view(&input(), &s);
        assert!(matches!(
            v.request,
            Some(CloudConnectRequest::Connect {
                make_default: true,
                ..
            })
        ));
        let s = cloud_connect_reduce(
            &input(),
            &s,
            &CloudConnectAction::Connected {
                label: "AWS \u{b7} us-west-2".into(),
            },
        );
        assert!(cloud_connect_view(&input(), &s).done);
    }

    #[test]
    fn the_sdk_status_maps_to_the_sheet_and_the_wizard() {
        let status = serde_json::json!({
            "default_on": "aws",
            "providers": [
                {"name": "aws", "title": "AWS", "tier": "vm", "connected": true,
                 "credentials": {"found": true, "source": "~/.aws profile default"},
                 "region": "us-west-2", "label": "AWS \u{b7} us-west-2", "ttl_hours": 8,
                 "kinds": [{"image": "linux", "kind": "container", "supported": true,
                            "machine_type": "t4g.medium", "usd_per_hour": 0.0368}]},
                {"name": "gcp", "title": "Google Cloud", "connected": false,
                 "credentials": {"found": false}}
            ]
        });
        let input = connect_input_from_status(&status);
        assert_eq!(input.providers.len(), 2);
        assert!(input.providers[0].found && input.providers[0].connected);
        assert_eq!(input.providers[0].source, "~/.aws profile default");
        let clouds = connected_clouds_from_status(&status);
        assert_eq!(clouds.len(), 1);
        assert!(clouds[0].is_default);
        assert_eq!(clouds[0].offers[0].machine_type, "t4g.medium");
        assert!(connected_clouds_from_status(&serde_json::json!("nope")).is_empty());
        assert!(is_cloud_word("modal") && !is_cloud_word("cloud"));
    }

    #[test]
    fn a_failure_says_why_and_selecting_another_resets_the_fields() {
        let s = run(&[
            CloudConnectAction::Connect,
            CloudConnectAction::Failed {
                error: "your cloud: AWS is not ready".into(),
            },
        ]);
        let v = cloud_connect_view(&input(), &s);
        assert_eq!(v.error.as_deref(), Some("your cloud: AWS is not ready"));
        assert!(v.can_test && v.can_connect);
        let s = cloud_connect_reduce(
            &input(),
            &s,
            &CloudConnectAction::Select { name: "gcp".into() },
        );
        let v = cloud_connect_view(&input(), &s);
        assert!(v.rows[1].selected);
        assert_eq!(v.field.as_ref().unwrap().label, "Project");
        assert!(v.profile_field.is_none());
        assert!(v.error.is_none());
    }

    #[test]
    fn no_cloud_shows_no_fields_and_ends_the_flow_without_a_request() {
        let i = input();
        let s = cloud_connect_reduce(
            &i,
            &cloud_connect_initial(),
            &CloudConnectAction::Select {
                name: NONE_PROVIDER.into(),
            },
        );
        let v = cloud_connect_view(&i, &s);
        let none_row = v.rows.last().unwrap();
        assert_eq!(none_row.id, NONE_PROVIDER);
        assert!(none_row.selected && !none_row.found);
        assert!(v.field.is_none() && v.profile_field.is_none());
        assert!(v.touches.is_empty());
        assert!(!v.can_test, "nothing to test for No cloud");
        assert!(v.can_connect);
        assert_eq!(v.connect_label, "Done");
        assert!(v.request.is_none());

        let s = cloud_connect_reduce(&i, &s, &CloudConnectAction::Connect);
        let v = cloud_connect_view(&i, &s);
        assert!(v.done, "Connect (Done) ends the flow on the spot");
        assert!(
            v.request.is_none(),
            "No cloud never asks the shell to run anything"
        );
    }

    #[test]
    fn no_cloud_leaves_an_already_connected_cloud_as_is() {
        let mut i = input();
        i.providers[0].connected = true; // AWS is already connected.
        let s = cloud_connect_reduce(
            &i,
            &cloud_connect_initial(),
            &CloudConnectAction::Select {
                name: NONE_PROVIDER.into(),
            },
        );
        let v = cloud_connect_view(&i, &s);
        assert_eq!(
            v.rows.last().unwrap().detail,
            "Leaves your connected cloud as is."
        );
        let s = cloud_connect_reduce(&i, &s, &CloudConnectAction::Connect);
        let v = cloud_connect_view(&i, &s);
        assert!(v.done);
        // Picking No cloud never issues a connect or a disconnect request,
        // so an already-connected cloud is never silently dropped.
        assert!(v.request.is_none());
    }

    #[test]
    fn what_each_provider_touches_names_its_resources_and_cleanup() {
        let i = input();
        let v = cloud_connect_view(&i, &cloud_connect_initial());
        assert!(v.touches.iter().any(|l| l.contains("EC2 instance")));
        assert!(v.touches.iter().any(|l| l.contains("security group")));
        assert!(
            v.touches
                .iter()
                .any(|l| l.contains("cua cloud sweep aws --all --delete"))
        );
        assert!(v.touches.iter().any(|l| l.contains("tagged and recorded")));

        let s = cloud_connect_reduce(
            &i,
            &cloud_connect_initial(),
            &CloudConnectAction::Select { name: "gcp".into() },
        );
        let v = cloud_connect_view(&i, &s);
        assert!(v.touches.iter().any(|l| l.contains("Compute Engine VM")));
        assert!(
            v.touches
                .iter()
                .any(|l| l.contains("cua cloud sweep gcp --all --delete"))
        );

        let modal = touches_of("modal");
        assert!(modal.iter().any(|l| l.contains("cua-sandboxes")));
        assert!(
            modal
                .iter()
                .any(|l| l.contains("cua cloud sweep modal --all --delete"))
        );
        assert!(touches_of("unknown-provider").is_empty());
    }
}
