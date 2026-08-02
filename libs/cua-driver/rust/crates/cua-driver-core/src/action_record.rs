//! Internal, non-wire representation of what an action actually did.
//!
//! This deliberately has no serde derives: it is the driver's source of truth,
//! not a protocol contract. Callers that need to publish an outcome should use
//! [`ActionExecutionRecord::stable_projection`] after validation.

/// The strongest truthful statement the driver can make about an action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActionEffect {
    Confirmed,
    Partial,
    Unverifiable,
    SuspectedNoop,
    Refused,
}

/// Classify an idempotent native mutation using an independent value readback.
/// A missing readback never confirms success, while a readable unchanged value
/// is stronger evidence of a no-op than of an unknown outcome.
pub fn effect_from_value_readback(
    confirmed: bool,
    changed: bool,
    readback_available: bool,
) -> ActionEffect {
    if confirmed && readback_available {
        ActionEffect::Confirmed
    } else if changed || !readback_available {
        ActionEffect::Unverifiable
    } else {
        ActionEffect::SuspectedNoop
    }
}

/// The delivery mode requested by the caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestedDelivery {
    Background,
    Foreground,
    NotApplicable,
}

/// The delivery mode actually used by the actuator.
///
/// `Unknown` means an attempt was made but the actuator could not determine
/// whether it delivered in the requested mode. `None` means no delivery was
/// attempted (for example, a refusal).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActualDelivery {
    Background,
    Foreground,
    NotApplicable,
    Unknown,
}

/// A concrete action transport known to the driver.
///
/// Keep this exhaustive rather than accepting arbitrary strings so a new
/// actuator must choose both its internal identity and published route.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ActionTransport {
    AgentCursorOverlay,
    MacosAxAction,
    MacosAxValue,
    MacosAxWindowFrame,
    MacosCgEventPid,
    MacosCgEventHid,
    WindowsUiaInvoke,
    WindowsUiaToggle,
    WindowsUiaSelection,
    WindowsUiaExpandCollapse,
    WindowsUiaValue,
    WindowsUiaRangeValue,
    WindowsUiaScroll,
    WindowsMsaaAction,
    WindowsPostMessage,
    WindowsTargetedInjection,
    WindowsSendInput,
    WindowsSetCursorPos,
    WindowsShellExecute,
    WindowsSetWindowPos,
    LinuxAtSpiAction,
    LinuxAtSpiValue,
    LinuxPty,
    LinuxXSendEvent,
    LinuxXTest,
    LinuxLibei,
    LinuxWaylandVirtualPointer,
    LinuxCuaCompositorInject,
    LinuxX11ConfigureWindow,
    BrowserCdpInputMouse,
    BrowserCdpInputKey,
    BrowserCdpRuntimeFunction,
}

impl ActionTransport {
    pub const ALL: &'static [Self] = &[
        Self::AgentCursorOverlay,
        Self::MacosAxAction,
        Self::MacosAxValue,
        Self::MacosAxWindowFrame,
        Self::MacosCgEventPid,
        Self::MacosCgEventHid,
        Self::WindowsUiaInvoke,
        Self::WindowsUiaToggle,
        Self::WindowsUiaSelection,
        Self::WindowsUiaExpandCollapse,
        Self::WindowsUiaValue,
        Self::WindowsUiaRangeValue,
        Self::WindowsUiaScroll,
        Self::WindowsMsaaAction,
        Self::WindowsPostMessage,
        Self::WindowsTargetedInjection,
        Self::WindowsSendInput,
        Self::WindowsSetCursorPos,
        Self::WindowsShellExecute,
        Self::WindowsSetWindowPos,
        Self::LinuxAtSpiAction,
        Self::LinuxAtSpiValue,
        Self::LinuxPty,
        Self::LinuxXSendEvent,
        Self::LinuxXTest,
        Self::LinuxLibei,
        Self::LinuxWaylandVirtualPointer,
        Self::LinuxCuaCompositorInject,
        Self::LinuxX11ConfigureWindow,
        Self::BrowserCdpInputMouse,
        Self::BrowserCdpInputKey,
        Self::BrowserCdpRuntimeFunction,
    ];

    pub const fn route(self) -> ActionRoute {
        match self {
            Self::AgentCursorOverlay => ActionRoute::SyntheticEvents,
            Self::MacosAxAction
            | Self::MacosAxValue
            | Self::MacosAxWindowFrame
            | Self::WindowsUiaInvoke
            | Self::WindowsUiaToggle
            | Self::WindowsUiaSelection
            | Self::WindowsUiaExpandCollapse
            | Self::WindowsUiaValue
            | Self::WindowsUiaRangeValue
            | Self::WindowsUiaScroll
            | Self::WindowsMsaaAction
            | Self::LinuxAtSpiAction
            | Self::LinuxAtSpiValue => ActionRoute::Accessibility,
            Self::MacosCgEventPid
            | Self::WindowsPostMessage
            | Self::LinuxPty
            | Self::LinuxXSendEvent => ActionRoute::SyntheticEvents,
            Self::MacosCgEventHid
            | Self::WindowsTargetedInjection
            | Self::WindowsSendInput
            | Self::WindowsSetCursorPos
            | Self::WindowsShellExecute
            | Self::LinuxXTest
            | Self::LinuxLibei
            | Self::LinuxWaylandVirtualPointer
            | Self::LinuxCuaCompositorInject => ActionRoute::GlobalInput,
            Self::WindowsSetWindowPos | Self::LinuxX11ConfigureWindow => ActionRoute::SystemApi,
            Self::BrowserCdpRuntimeFunction => ActionRoute::Dom,
            Self::BrowserCdpInputMouse | Self::BrowserCdpInputKey => ActionRoute::TrustedInput,
        }
    }
}

/// A stable route label suitable for projecting internal action truth.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActionRoute {
    Accessibility,
    SyntheticEvents,
    GlobalInput,
    SystemApi,
    Dom,
    TrustedInput,
}

/// Evidence supporting an action effect, intentionally without request data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionEvidence {
    pub kind: EvidenceKind,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvidenceKind {
    AccessibilityReadback,
    BrowserReadback,
    ValueReadback,
    WindowChange,
    NativeApiResult,
    ScreenshotComparison,
    EventReceipt,
    OperatorObservation,
}

/// A failed or superseded transport attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionAttempt {
    pub transport: ActionTransport,
    pub delivery: ActualDelivery,
    pub detail: Option<String>,
}

/// A deliberate transition between transports.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionFallback {
    pub from: ActionTransport,
    pub to: ActionTransport,
    pub reason: String,
}

/// Escalation undertaken while trying to complete an action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionEscalation {
    pub kind: EscalationKind,
    pub detail: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EscalationKind {
    ActivateTarget,
    RetryWithPixelTarget,
    RetryWithPageAction,
    RefreshPageState,
    RequestPermission,
    ElevateAccess,
    ExpandCaptureScope,
    PrepareSession,
    RetryWithForegroundDelivery,
}

/// Complete internal accounting for one action execution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionExecutionRecord {
    pub effect: ActionEffect,
    pub transport: ActionTransport,
    pub requested_delivery: RequestedDelivery,
    pub actual_delivery: Option<ActualDelivery>,
    pub attempts: Vec<ActionAttempt>,
    pub fallbacks: Vec<ActionFallback>,
    pub evidence: Vec<ActionEvidence>,
    pub escalation: Option<ActionEscalation>,
    pub delivered_count: Option<u32>,
    pub detail: Option<String>,
}

impl ActionExecutionRecord {
    pub fn new(
        effect: ActionEffect,
        transport: ActionTransport,
        requested_delivery: RequestedDelivery,
    ) -> Self {
        Self {
            effect,
            transport,
            requested_delivery,
            actual_delivery: None,
            attempts: Vec::new(),
            fallbacks: Vec::new(),
            evidence: Vec::new(),
            escalation: None,
            delivered_count: None,
            detail: None,
        }
    }

    pub fn builder(
        effect: ActionEffect,
        transport: ActionTransport,
        requested_delivery: RequestedDelivery,
    ) -> ActionExecutionRecordBuilder {
        ActionExecutionRecordBuilder::new(effect, transport, requested_delivery)
    }

    pub fn validate(&self) -> Result<(), ActionRecordValidationError> {
        match self.effect {
            ActionEffect::Confirmed if projected_evidence(&self.evidence).is_none() => {
                Err(ActionRecordValidationError::ConfirmedRequiresEvidence)
            }
            ActionEffect::Partial
                if self.actual_delivery.is_none() || self.delivered_count.is_none() =>
            {
                Err(ActionRecordValidationError::PartialRequiresDeliveredCount)
            }
            ActionEffect::Refused if self.actual_delivery.is_some() => {
                Err(ActionRecordValidationError::RefusedCannotHaveDelivery)
            }
            ActionEffect::Refused if !self.evidence.is_empty() => {
                Err(ActionRecordValidationError::RefusedCannotHaveEvidence)
            }
            _ => Ok(()),
        }
    }

    pub fn stable_projection(
        &self,
    ) -> Result<ActionOutcomeProjection, ActionRecordValidationError> {
        self.validate()?;
        Ok(ActionOutcomeProjection {
            effect: self.effect,
            route: self.transport.route(),
            delivery: self.actual_delivery.map(|actual| ActionDeliveryProjection {
                actual,
                delivered_count: self.delivered_count,
            }),
            evidence: projected_evidence(&self.evidence),
            escalation: self.escalation.clone(),
        })
    }

    /// Build the closed public action contract from the validated internal
    /// record. This is intentionally constructive: request data, raw
    /// transports, diagnostic detail, and non-publishable evidence never
    /// enter the value that is serialized for MCP or SDK clients.
    pub fn public_result(
        &self,
    ) -> Result<cua_driver_contract::ActionResult, ActionRecordValidationError> {
        let projection = self.stable_projection()?;
        Ok(cua_driver_contract::ActionResult {
            effect: match projection.effect {
                ActionEffect::Confirmed => cua_driver_contract::ActionEffect::Confirmed,
                ActionEffect::Partial => cua_driver_contract::ActionEffect::Partial,
                ActionEffect::Unverifiable => cua_driver_contract::ActionEffect::Unverifiable,
                ActionEffect::SuspectedNoop => cua_driver_contract::ActionEffect::SuspectedNoop,
                ActionEffect::Refused => cua_driver_contract::ActionEffect::Refused,
            },
            route: match projection.route {
                ActionRoute::Accessibility => cua_driver_contract::ActionRoute::Accessibility,
                ActionRoute::SyntheticEvents => cua_driver_contract::ActionRoute::SyntheticEvents,
                ActionRoute::GlobalInput => cua_driver_contract::ActionRoute::GlobalInput,
                ActionRoute::SystemApi => cua_driver_contract::ActionRoute::SystemApi,
                ActionRoute::Dom => cua_driver_contract::ActionRoute::Dom,
                ActionRoute::TrustedInput => cua_driver_contract::ActionRoute::TrustedInput,
            },
            delivery: projection
                .delivery
                .map(|delivery| cua_driver_contract::ActionDelivery {
                    mode: match delivery.actual {
                        ActualDelivery::Background => {
                            cua_driver_contract::ActionDeliveryMode::Background
                        }
                        ActualDelivery::Foreground => {
                            cua_driver_contract::ActionDeliveryMode::Foreground
                        }
                        ActualDelivery::NotApplicable => {
                            cua_driver_contract::ActionDeliveryMode::NotApplicable
                        }
                        ActualDelivery::Unknown => cua_driver_contract::ActionDeliveryMode::Unknown,
                    },
                    delivered_count: delivery.delivered_count,
                }),
            evidence: projection.evidence.map(|evidence| {
                evidence
                    .into_iter()
                    .map(|evidence| cua_driver_contract::ActionEvidence {
                        kind: match evidence.kind {
                            ProjectedEvidenceKind::AccessibilityReadback
                            | ProjectedEvidenceKind::BrowserReadback
                            | ProjectedEvidenceKind::ValueReadback => {
                                cua_driver_contract::ActionEvidenceKind::ValueReadback
                            }
                            ProjectedEvidenceKind::WindowChange => {
                                cua_driver_contract::ActionEvidenceKind::WindowChange
                            }
                        },
                    })
                    .collect()
            }),
            escalation: projection.escalation.map(|escalation| {
                use cua_driver_contract::{
                    ActionEscalation, ActionEscalationReason, ActionEscalationTarget,
                };

                let (target, reason) = match escalation.kind {
                    EscalationKind::ActivateTarget
                    | EscalationKind::RetryWithForegroundDelivery => (
                        ActionEscalationTarget::Foreground,
                        ActionEscalationReason::DeliveryFailed,
                    ),
                    EscalationKind::RetryWithPixelTarget => (
                        ActionEscalationTarget::Pixel,
                        ActionEscalationReason::EffectUnconfirmed,
                    ),
                    EscalationKind::RetryWithPageAction => (
                        ActionEscalationTarget::Page,
                        ActionEscalationReason::EffectUnconfirmed,
                    ),
                    EscalationKind::RefreshPageState => (
                        ActionEscalationTarget::Page,
                        ActionEscalationReason::RouteUnavailable,
                    ),
                    EscalationKind::RequestPermission | EscalationKind::ElevateAccess => (
                        ActionEscalationTarget::Session,
                        ActionEscalationReason::PermissionRequired,
                    ),
                    EscalationKind::ExpandCaptureScope => (
                        ActionEscalationTarget::Session,
                        ActionEscalationReason::RouteUnavailable,
                    ),
                    EscalationKind::PrepareSession => (
                        ActionEscalationTarget::Session,
                        ActionEscalationReason::RouteUnavailable,
                    ),
                };
                ActionEscalation {
                    target,
                    reason: if projection.effect == ActionEffect::SuspectedNoop
                        && reason != ActionEscalationReason::PermissionRequired
                    {
                        ActionEscalationReason::SuspectedNoop
                    } else {
                        reason
                    },
                }
            }),
        })
    }

    /// Normalize the legacy hand-written payload at the canonical dispatch
    /// seam before replacing it with the closed public action contract.
    ///
    /// This compatibility adapter is intentionally conservative: it reports
    /// `Unknown` rather than copying a requested delivery mode when the old
    /// payload did not prove what actually happened. Platform producers can
    /// attach a richer record directly as they are migrated.
    pub fn from_legacy(
        tool_name: &str,
        args: &serde_json::Value,
        structured: &serde_json::Value,
    ) -> Option<Self> {
        if !is_action_tool(tool_name) {
            return None;
        }
        let requested_delivery = requested_delivery(tool_name, args);
        let raw_path = structured
            .get("path")
            .or_else(|| structured.get("route"))
            .and_then(serde_json::Value::as_str);
        let transport = transport_from_legacy(tool_name, args, raw_path)?;
        let effect = legacy_effect(structured);
        let actual_delivery =
            actual_delivery_from_legacy(tool_name, args, structured, raw_path, transport, effect);

        let mut record = Self::new(effect, transport, requested_delivery);
        record.actual_delivery = actual_delivery;
        record.delivered_count = structured
            .get("delivered_chars")
            .or_else(|| structured.pointer("/refusal/detail/delivered_chars"))
            .and_then(serde_json::Value::as_u64)
            .and_then(|count| u32::try_from(count).ok());

        if legacy_has_publishable_readback(tool_name, structured) {
            record.evidence.push(ActionEvidence {
                kind: if matches!(
                    transport,
                    ActionTransport::BrowserCdpInputMouse
                        | ActionTransport::BrowserCdpInputKey
                        | ActionTransport::BrowserCdpRuntimeFunction
                ) {
                    EvidenceKind::BrowserReadback
                } else {
                    EvidenceKind::AccessibilityReadback
                },
                detail: structured
                    .get("verify")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("confirmed")
                    .to_owned(),
            });
        }
        if let Some(escalation) = structured.get("escalation") {
            let recommendation = escalation
                .get("recommended")
                .and_then(serde_json::Value::as_str);
            let kind = match recommendation {
                Some("foreground") => Some(EscalationKind::RetryWithForegroundDelivery),
                Some("px" | "pixel") => Some(EscalationKind::RetryWithPixelTarget),
                Some("page") => Some(EscalationKind::RetryWithPageAction),
                Some("session") => Some(EscalationKind::ExpandCaptureScope),
                _ => None,
            };
            if let Some(kind) = kind {
                record.escalation = Some(ActionEscalation {
                    kind,
                    detail: escalation
                        .get("reason")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned),
                });
            }
        } else if let Some(code) = structured
            .pointer("/refusal/code")
            .and_then(serde_json::Value::as_str)
        {
            record.escalation = browser_refusal_escalation(code);
        }
        if effect == ActionEffect::Partial && record.delivered_count.is_none() {
            return None;
        }
        if effect == ActionEffect::Confirmed && projected_evidence(&record.evidence).is_none() {
            // A legacy string claiming "confirmed" without a trusted readback
            // is not enough to preserve that stronger statement.
            record.effect = ActionEffect::Unverifiable;
        }
        record.validate().ok()?;
        Some(record)
    }

    /// Explicit internal-only representation for recording and diagnostics.
    /// Keeping this constructor here prevents serde derives from accidentally
    /// turning the rich record into a protocol surface.
    pub fn debug_json(&self) -> serde_json::Value {
        serde_json::json!({
            "effect": effect_name(self.effect),
            "transport": transport_name(self.transport),
            "route": route_name(self.transport.route()),
            "requested_delivery": requested_delivery_name(self.requested_delivery),
            "actual_delivery": self.actual_delivery.map(actual_delivery_name),
            "delivered_count": self.delivered_count,
            "attempts": self.attempts.iter().map(|attempt| serde_json::json!({
                "transport": transport_name(attempt.transport),
                "delivery": actual_delivery_name(attempt.delivery),
                "detail": attempt.detail,
            })).collect::<Vec<_>>(),
            "fallbacks": self.fallbacks.iter().map(|fallback| serde_json::json!({
                "from": transport_name(fallback.from),
                "to": transport_name(fallback.to),
                "reason": fallback.reason,
            })).collect::<Vec<_>>(),
            "evidence": self.evidence.iter().map(|evidence| serde_json::json!({
                "kind": evidence_kind_name(evidence.kind),
                "detail": evidence.detail,
            })).collect::<Vec<_>>(),
            "escalation": self.escalation.as_ref().map(|escalation| serde_json::json!({
                "kind": escalation_kind_name(escalation.kind),
                "detail": escalation.detail,
            })),
            "detail": self.detail,
        })
    }
}

pub fn is_action_tool(tool_name: &str) -> bool {
    cua_driver_contract::is_action_result_tool(tool_name)
}

fn requested_delivery(tool_name: &str, args: &serde_json::Value) -> RequestedDelivery {
    match args
        .get("delivery_mode")
        .and_then(serde_json::Value::as_str)
    {
        Some("foreground") => RequestedDelivery::Foreground,
        Some("background") => RequestedDelivery::Background,
        _ if args.get("scope").and_then(serde_json::Value::as_str) == Some("desktop") => {
            RequestedDelivery::NotApplicable
        }
        _ if matches!(
            tool_name,
            "browser_click" | "browser_pointer" | "browser_type"
        ) =>
        {
            RequestedDelivery::NotApplicable
        }
        _ => RequestedDelivery::Background,
    }
}

fn legacy_effect(structured: &serde_json::Value) -> ActionEffect {
    if structured.get("status").and_then(serde_json::Value::as_str) == Some("refused") {
        let is_partial = structured
            .pointer("/refusal/code")
            .and_then(serde_json::Value::as_str)
            == Some("browser_input_incomplete")
            && structured
                .pointer("/refusal/detail/delivered_chars")
                .and_then(serde_json::Value::as_u64)
                .is_some_and(|count| count > 0);
        return if is_partial {
            ActionEffect::Partial
        } else {
            ActionEffect::Refused
        };
    }
    match structured.get("effect").and_then(serde_json::Value::as_str) {
        Some("confirmed") => ActionEffect::Confirmed,
        Some("partial") => ActionEffect::Partial,
        Some("suspected_noop") => ActionEffect::SuspectedNoop,
        Some("refused") => ActionEffect::Refused,
        _ => ActionEffect::Unverifiable,
    }
}

fn browser_refusal_escalation(code: &str) -> Option<ActionEscalation> {
    let kind = match code {
        "browser_consent_required" | "browser_consent_revoked" | "browser_origin_outside_scope" => {
            EscalationKind::RequestPermission
        }
        "browser_requires_setup"
        | "browser_route_unavailable"
        | "browser_endpoint_owner_mismatch"
        | "browser_reconnect_exhausted" => EscalationKind::PrepareSession,
        "browser_binding_ambiguous"
        | "browser_binding_stale"
        | "browser_wrong_target_refused"
        | "browser_tab_required"
        | "browser_tab_not_found"
        | "browser_ref_stale"
        | "browser_input_trust_unavailable"
        | "browser_action_unavailable" => EscalationKind::RefreshPageState,
        // A delivered prefix is already represented by `effect: partial` and
        // `delivery.delivered_count`; verification decides whether to stop.
        "browser_input_incomplete" => return None,
        _ => return None,
    };
    Some(ActionEscalation { kind, detail: None })
}

/// Legacy `verified` was not a sufficient proof by itself: some action
/// producers used it as a delivery acknowledgement. Value-changing tools may
/// promote that bit when their platform implementation compares a fresh value
/// against the request. A click may do so only when the producer also declares
/// explicit accessibility read-back evidence, as the macOS collection-item
/// selection fallback does after observing `AXSelected`.
fn legacy_has_publishable_readback(tool_name: &str, structured: &serde_json::Value) -> bool {
    let value_readback_tool = matches!(tool_name, "type_text" | "type_text_chars" | "set_value");
    let click_selection_readback = tool_name == "click"
        && structured
            .get("evidence")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|evidence| {
                evidence.iter().any(|item| {
                    item.get("kind").and_then(serde_json::Value::as_str)
                        == Some("accessibility_readback")
                })
            });

    (value_readback_tool || click_selection_readback)
        && structured
            .get("verified")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        && structured.get("effect").and_then(serde_json::Value::as_str) == Some("confirmed")
}

fn transport_from_legacy(
    tool_name: &str,
    args: &serde_json::Value,
    raw_path: Option<&str>,
) -> Option<ActionTransport> {
    let path = raw_path.unwrap_or("");
    let transport = match path {
        "ax" | "ax_fg" => {
            if cfg!(target_os = "macos") {
                if matches!(tool_name, "type_text" | "type_text_chars" | "set_value") {
                    ActionTransport::MacosAxValue
                } else {
                    ActionTransport::MacosAxAction
                }
            } else if cfg!(target_os = "windows") {
                if matches!(tool_name, "type_text" | "type_text_chars" | "set_value") {
                    ActionTransport::WindowsUiaValue
                } else {
                    ActionTransport::WindowsUiaInvoke
                }
            } else {
                if matches!(tool_name, "type_text" | "type_text_chars" | "set_value") {
                    ActionTransport::LinuxAtSpiValue
                } else {
                    ActionTransport::LinuxAtSpiAction
                }
            }
        }
        "uia" if tool_name == "scroll" => ActionTransport::WindowsUiaScroll,
        "uia" => ActionTransport::WindowsUiaInvoke,
        "uia_expand_collapse" => ActionTransport::WindowsUiaExpandCollapse,
        "msaa" => ActionTransport::WindowsMsaaAction,
        "post_message" | "PostMessage" => ActionTransport::WindowsPostMessage,
        "SendInput" => ActionTransport::WindowsSendInput,
        "SetCursorPos" => ActionTransport::WindowsSetCursorPos,
        "atspi" | "wayland_atspi" | "x11_atspi" => ActionTransport::LinuxAtSpiAction,
        "pty" => ActionTransport::LinuxPty,
        "x11_pixel" | "x11_pixel_fg" | "x11_xtest_fg" | "xtest" | "xtest_desktop" => {
            ActionTransport::LinuxXTest
        }
        "wayland_activate" | "wayland_focused" => ActionTransport::LinuxLibei,
        "wayland_desktop" => ActionTransport::LinuxWaylandVirtualPointer,
        "cua_compositor_inject" | "wayland_cua_compositor" => {
            ActionTransport::LinuxCuaCompositorInject
        }
        "hid" | "cgevent_hid" | "cgevent_fg" => ActionTransport::MacosCgEventHid,
        "cgevent" => {
            if args
                .get("delivery_mode")
                .and_then(serde_json::Value::as_str)
                == Some("foreground")
            {
                ActionTransport::MacosCgEventHid
            } else {
                ActionTransport::MacosCgEventPid
            }
        }
        "dom_event" => ActionTransport::BrowserCdpRuntimeFunction,
        "trusted" => ActionTransport::BrowserCdpInputMouse,
        "key_events" | "key_events_fg" => {
            let foreground = path.ends_with("_fg")
                || args
                    .get("delivery_mode")
                    .and_then(serde_json::Value::as_str)
                    == Some("foreground");
            if cfg!(target_os = "macos") {
                if foreground {
                    ActionTransport::MacosCgEventHid
                } else {
                    ActionTransport::MacosCgEventPid
                }
            } else if cfg!(target_os = "windows") {
                if foreground {
                    ActionTransport::WindowsSendInput
                } else {
                    ActionTransport::WindowsPostMessage
                }
            } else {
                if foreground {
                    ActionTransport::LinuxXTest
                } else {
                    ActionTransport::LinuxXSendEvent
                }
            }
        }
        "pixel" => {
            if cfg!(target_os = "macos") {
                if args
                    .get("delivery_mode")
                    .and_then(serde_json::Value::as_str)
                    == Some("foreground")
                {
                    ActionTransport::MacosCgEventHid
                } else {
                    ActionTransport::MacosCgEventPid
                }
            } else if cfg!(target_os = "windows") {
                if args
                    .get("delivery_mode")
                    .and_then(serde_json::Value::as_str)
                    == Some("foreground")
                {
                    ActionTransport::WindowsSendInput
                } else {
                    ActionTransport::WindowsTargetedInjection
                }
            } else {
                ActionTransport::LinuxXTest
            }
        }
        "" if matches!(tool_name, "browser_click" | "browser_pointer") => {
            if args.get("input_route").and_then(serde_json::Value::as_str) == Some("dom_event") {
                ActionTransport::BrowserCdpRuntimeFunction
            } else {
                ActionTransport::BrowserCdpInputMouse
            }
        }
        "" if tool_name == "browser_type" => ActionTransport::BrowserCdpInputKey,
        "" if tool_name == "move_cursor"
            && args.get("scope").and_then(serde_json::Value::as_str) != Some("desktop") =>
        {
            ActionTransport::AgentCursorOverlay
        }
        "" if args.get("scope").and_then(serde_json::Value::as_str) == Some("desktop") => {
            if cfg!(target_os = "macos") {
                ActionTransport::MacosCgEventHid
            } else if cfg!(target_os = "windows") {
                ActionTransport::WindowsSendInput
            } else {
                ActionTransport::LinuxXTest
            }
        }
        "" if matches!(
            tool_name,
            "type_text" | "type_text_chars" | "press_key" | "hotkey"
        ) =>
        {
            if cfg!(target_os = "macos") {
                ActionTransport::MacosCgEventPid
            } else if cfg!(target_os = "windows") {
                if args
                    .get("delivery_mode")
                    .and_then(serde_json::Value::as_str)
                    == Some("foreground")
                {
                    ActionTransport::WindowsSendInput
                } else {
                    ActionTransport::WindowsPostMessage
                }
            } else {
                ActionTransport::LinuxXSendEvent
            }
        }
        "" if tool_name == "set_value" => {
            if cfg!(target_os = "macos") {
                ActionTransport::MacosAxValue
            } else if cfg!(target_os = "windows") {
                ActionTransport::WindowsUiaValue
            } else {
                ActionTransport::LinuxAtSpiValue
            }
        }
        "" if matches!(
            tool_name,
            "click"
                | "double_click"
                | "right_click"
                | "scroll"
                | "drag"
                | "mouse_drag"
                | "parallel_mouse_drag"
                | "mouse_button_down"
                | "mouse_button_up"
        ) =>
        {
            if cfg!(target_os = "macos") {
                if args
                    .get("delivery_mode")
                    .and_then(serde_json::Value::as_str)
                    == Some("foreground")
                {
                    ActionTransport::MacosCgEventHid
                } else {
                    ActionTransport::MacosCgEventPid
                }
            } else if cfg!(target_os = "windows") {
                if args
                    .get("delivery_mode")
                    .and_then(serde_json::Value::as_str)
                    == Some("foreground")
                {
                    ActionTransport::WindowsSendInput
                } else {
                    ActionTransport::WindowsTargetedInjection
                }
            } else {
                ActionTransport::LinuxXTest
            }
        }
        _ => return None,
    };
    Some(transport)
}

fn actual_delivery_from_legacy(
    tool_name: &str,
    args: &serde_json::Value,
    structured: &serde_json::Value,
    raw_path: Option<&str>,
    transport: ActionTransport,
    effect: ActionEffect,
) -> Option<ActualDelivery> {
    if effect == ActionEffect::Refused {
        return None;
    }
    if matches!(
        tool_name,
        "browser_click" | "browser_pointer" | "browser_type"
    ) {
        return Some(ActualDelivery::Background);
    }
    if transport == ActionTransport::AgentCursorOverlay
        || args.get("scope").and_then(serde_json::Value::as_str) == Some("desktop")
    {
        return Some(ActualDelivery::NotApplicable);
    }
    match structured_delivery_mode(args, structured) {
        Some(delivery) => return Some(delivery),
        None => {}
    }
    match raw_path {
        Some(path) if path.ends_with("_fg") => Some(ActualDelivery::Foreground),
        Some("hid" | "cgevent_hid" | "SendInput" | "wayland_activate" | "wayland_focused") => {
            Some(ActualDelivery::Foreground)
        }
        Some(_) => Some(ActualDelivery::Background),
        None => Some(ActualDelivery::Unknown),
    }
}

/// Successful legacy foreground branches either complete their activation and
/// dispatch or return an error. The request therefore identifies the executed
/// branch even when the old payload omitted a path or reused a background path
/// label such as Windows `pixel`. Prefer an explicit producer-emitted mode when
/// one exists.
fn structured_delivery_mode(
    args: &serde_json::Value,
    structured: &serde_json::Value,
) -> Option<ActualDelivery> {
    let mode = structured
        .get("delivery_mode")
        .or_else(|| args.get("delivery_mode"))
        .and_then(serde_json::Value::as_str);
    match mode {
        Some("foreground") => Some(ActualDelivery::Foreground),
        Some("background") if structured.get("delivery_mode").is_some() => {
            Some(ActualDelivery::Background)
        }
        // A requested background mode alone is not proof when the legacy
        // producer returned no route. Keep it unknown until the path branch
        // below establishes background delivery.
        _ => None,
    }
}

fn projected_evidence(evidence: &[ActionEvidence]) -> Option<Vec<ActionEvidenceProjection>> {
    let evidence: Vec<_> = evidence
        .iter()
        .filter_map(|evidence| {
            let kind = match evidence.kind {
                EvidenceKind::AccessibilityReadback => ProjectedEvidenceKind::AccessibilityReadback,
                EvidenceKind::BrowserReadback => ProjectedEvidenceKind::BrowserReadback,
                EvidenceKind::ValueReadback => ProjectedEvidenceKind::ValueReadback,
                EvidenceKind::WindowChange => ProjectedEvidenceKind::WindowChange,
                EvidenceKind::NativeApiResult
                | EvidenceKind::ScreenshotComparison
                | EvidenceKind::EventReceipt
                | EvidenceKind::OperatorObservation => return None,
            };
            Some(ActionEvidenceProjection {
                kind,
                detail: evidence.detail.clone(),
            })
        })
        .collect();
    (!evidence.is_empty()).then_some(evidence)
}

fn effect_name(effect: ActionEffect) -> &'static str {
    match effect {
        ActionEffect::Confirmed => "confirmed",
        ActionEffect::Partial => "partial",
        ActionEffect::Unverifiable => "unverifiable",
        ActionEffect::SuspectedNoop => "suspected_noop",
        ActionEffect::Refused => "refused",
    }
}

fn route_name(route: ActionRoute) -> &'static str {
    match route {
        ActionRoute::Accessibility => "accessibility",
        ActionRoute::SyntheticEvents => "synthetic_events",
        ActionRoute::GlobalInput => "global_input",
        ActionRoute::SystemApi => "system_api",
        ActionRoute::Dom => "dom",
        ActionRoute::TrustedInput => "trusted_input",
    }
}

fn requested_delivery_name(delivery: RequestedDelivery) -> &'static str {
    match delivery {
        RequestedDelivery::Background => "background",
        RequestedDelivery::Foreground => "foreground",
        RequestedDelivery::NotApplicable => "not_applicable",
    }
}

fn actual_delivery_name(delivery: ActualDelivery) -> &'static str {
    match delivery {
        ActualDelivery::Background => "background",
        ActualDelivery::Foreground => "foreground",
        ActualDelivery::NotApplicable => "not_applicable",
        ActualDelivery::Unknown => "unknown",
    }
}

fn evidence_kind_name(kind: EvidenceKind) -> &'static str {
    match kind {
        EvidenceKind::AccessibilityReadback => "accessibility_readback",
        EvidenceKind::BrowserReadback => "browser_readback",
        EvidenceKind::ValueReadback => "value_readback",
        EvidenceKind::WindowChange => "window_change",
        EvidenceKind::NativeApiResult => "native_api_result",
        EvidenceKind::ScreenshotComparison => "screenshot_comparison",
        EvidenceKind::EventReceipt => "event_receipt",
        EvidenceKind::OperatorObservation => "operator_observation",
    }
}

fn escalation_kind_name(kind: EscalationKind) -> &'static str {
    match kind {
        EscalationKind::ActivateTarget => "activate_target",
        EscalationKind::RetryWithPixelTarget => "retry_with_pixel_target",
        EscalationKind::RetryWithPageAction => "retry_with_page_action",
        EscalationKind::RefreshPageState => "refresh_page_state",
        EscalationKind::RequestPermission => "request_permission",
        EscalationKind::ElevateAccess => "elevate_access",
        EscalationKind::ExpandCaptureScope => "expand_capture_scope",
        EscalationKind::PrepareSession => "prepare_session",
        EscalationKind::RetryWithForegroundDelivery => "retry_with_foreground_delivery",
    }
}

fn transport_name(transport: ActionTransport) -> &'static str {
    match transport {
        ActionTransport::AgentCursorOverlay => "agent_cursor_overlay",
        ActionTransport::MacosAxAction => "macos_ax_action",
        ActionTransport::MacosAxValue => "macos_ax_value",
        ActionTransport::MacosAxWindowFrame => "macos_ax_window_frame",
        ActionTransport::MacosCgEventPid => "macos_cg_event_pid",
        ActionTransport::MacosCgEventHid => "macos_cg_event_hid",
        ActionTransport::WindowsUiaInvoke => "windows_uia_invoke",
        ActionTransport::WindowsUiaToggle => "windows_uia_toggle",
        ActionTransport::WindowsUiaSelection => "windows_uia_selection",
        ActionTransport::WindowsUiaExpandCollapse => "windows_uia_expand_collapse",
        ActionTransport::WindowsUiaValue => "windows_uia_value",
        ActionTransport::WindowsUiaRangeValue => "windows_uia_range_value",
        ActionTransport::WindowsUiaScroll => "windows_uia_scroll",
        ActionTransport::WindowsMsaaAction => "windows_msaa_action",
        ActionTransport::WindowsPostMessage => "windows_post_message",
        ActionTransport::WindowsTargetedInjection => "windows_targeted_injection",
        ActionTransport::WindowsSendInput => "windows_send_input",
        ActionTransport::WindowsSetCursorPos => "windows_set_cursor_pos",
        ActionTransport::WindowsShellExecute => "windows_shell_execute",
        ActionTransport::WindowsSetWindowPos => "windows_set_window_pos",
        ActionTransport::LinuxAtSpiAction => "linux_at_spi_action",
        ActionTransport::LinuxAtSpiValue => "linux_at_spi_value",
        ActionTransport::LinuxPty => "linux_pty",
        ActionTransport::LinuxXSendEvent => "linux_x_send_event",
        ActionTransport::LinuxXTest => "linux_x_test",
        ActionTransport::LinuxLibei => "linux_libei",
        ActionTransport::LinuxWaylandVirtualPointer => "linux_wayland_virtual_pointer",
        ActionTransport::LinuxCuaCompositorInject => "linux_cua_compositor_inject",
        ActionTransport::LinuxX11ConfigureWindow => "linux_x11_configure_window",
        ActionTransport::BrowserCdpInputMouse => "browser_cdp_input_mouse",
        ActionTransport::BrowserCdpInputKey => "browser_cdp_input_key",
        ActionTransport::BrowserCdpRuntimeFunction => "browser_cdp_runtime_function",
    }
}

/// Builder that requires callers to state both effect and selected transport.
#[derive(Clone, Debug)]
pub struct ActionExecutionRecordBuilder(ActionExecutionRecord);

impl ActionExecutionRecordBuilder {
    pub fn new(
        effect: ActionEffect,
        transport: ActionTransport,
        requested_delivery: RequestedDelivery,
    ) -> Self {
        Self(ActionExecutionRecord::new(
            effect,
            transport,
            requested_delivery,
        ))
    }

    pub fn actual_delivery(mut self, delivery: ActualDelivery) -> Self {
        self.0.actual_delivery = Some(delivery);
        self
    }

    pub fn attempt(mut self, attempt: ActionAttempt) -> Self {
        self.0.attempts.push(attempt);
        self
    }

    pub fn fallback(mut self, fallback: ActionFallback) -> Self {
        self.0.fallbacks.push(fallback);
        self
    }

    pub fn evidence(mut self, evidence: ActionEvidence) -> Self {
        self.0.evidence.push(evidence);
        self
    }

    pub fn escalation(mut self, escalation: ActionEscalation) -> Self {
        self.0.escalation = Some(escalation);
        self
    }

    pub fn delivered_count(mut self, delivered_count: u32) -> Self {
        self.0.delivered_count = Some(delivered_count);
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.0.detail = Some(detail.into());
        self
    }

    pub fn build(self) -> Result<ActionExecutionRecord, ActionRecordValidationError> {
        self.0.validate()?;
        Ok(self.0)
    }
}

/// Projection deliberately excludes request target, coordinates, and scope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionOutcomeProjection {
    pub effect: ActionEffect,
    pub route: ActionRoute,
    pub delivery: Option<ActionDeliveryProjection>,
    pub evidence: Option<Vec<ActionEvidenceProjection>>,
    pub escalation: Option<ActionEscalation>,
}

/// Published delivery accounting; the original request is intentionally absent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionDeliveryProjection {
    pub actual: ActualDelivery,
    pub delivered_count: Option<u32>,
}

/// Evidence families that may appear in the stable projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionEvidenceProjection {
    pub kind: ProjectedEvidenceKind,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectedEvidenceKind {
    AccessibilityReadback,
    BrowserReadback,
    ValueReadback,
    WindowChange,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActionRecordValidationError {
    ConfirmedRequiresEvidence,
    PartialRequiresDeliveredCount,
    RefusedCannotHaveDelivery,
    RefusedCannotHaveEvidence,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence() -> ActionEvidence {
        ActionEvidence {
            kind: EvidenceKind::AccessibilityReadback,
            detail: "value changed".to_owned(),
        }
    }

    #[test]
    fn value_readback_never_claims_unobserved_success() {
        assert_eq!(
            effect_from_value_readback(true, false, true),
            ActionEffect::Confirmed
        );
        assert_eq!(
            effect_from_value_readback(false, true, true),
            ActionEffect::Unverifiable
        );
        assert_eq!(
            effect_from_value_readback(false, false, false),
            ActionEffect::Unverifiable
        );
        assert_eq!(
            effect_from_value_readback(true, false, false),
            ActionEffect::Unverifiable
        );
        assert_eq!(
            effect_from_value_readback(false, false, true),
            ActionEffect::SuspectedNoop
        );
    }

    #[test]
    fn every_transport_maps_to_one_of_the_six_stable_routes() {
        let mut routes = Vec::new();
        for transport in ActionTransport::ALL {
            let record = ActionExecutionRecord::builder(
                ActionEffect::Unverifiable,
                *transport,
                RequestedDelivery::Foreground,
            )
            .actual_delivery(ActualDelivery::Unknown)
            .build()
            .unwrap();
            let route = record.stable_projection().unwrap().route;
            assert_eq!(route, transport.route());
            routes.push(route);
        }
        assert!(routes.contains(&ActionRoute::Accessibility));
        assert!(routes.contains(&ActionRoute::SyntheticEvents));
        assert!(routes.contains(&ActionRoute::GlobalInput));
        assert!(routes.contains(&ActionRoute::SystemApi));
        assert!(routes.contains(&ActionRoute::Dom));
        assert!(routes.contains(&ActionRoute::TrustedInput));
    }

    #[test]
    fn confirmed_requires_evidence() {
        assert_eq!(
            ActionExecutionRecord::new(
                ActionEffect::Confirmed,
                ActionTransport::MacosAxAction,
                RequestedDelivery::Background,
            )
            .validate(),
            Err(ActionRecordValidationError::ConfirmedRequiresEvidence)
        );

        assert!(ActionExecutionRecord::builder(
            ActionEffect::Confirmed,
            ActionTransport::MacosAxAction,
            RequestedDelivery::Background,
        )
        .evidence(evidence())
        .build()
        .is_ok());
    }

    #[test]
    fn partial_requires_delivered_count() {
        assert_eq!(
            ActionExecutionRecord::new(
                ActionEffect::Partial,
                ActionTransport::WindowsSendInput,
                RequestedDelivery::Foreground,
            )
            .validate(),
            Err(ActionRecordValidationError::PartialRequiresDeliveredCount)
        );

        let mut record = ActionExecutionRecord::new(
            ActionEffect::Partial,
            ActionTransport::WindowsSendInput,
            RequestedDelivery::Foreground,
        );
        record.delivered_count = Some(1);
        assert_eq!(
            record.validate(),
            Err(ActionRecordValidationError::PartialRequiresDeliveredCount),
            "a count without an actual delivery cannot form a valid public partial result"
        );
    }

    #[test]
    fn refused_cannot_claim_delivery_or_evidence() {
        let delivery = ActionExecutionRecord::builder(
            ActionEffect::Refused,
            ActionTransport::BrowserCdpInputMouse,
            RequestedDelivery::NotApplicable,
        )
        .actual_delivery(ActualDelivery::Unknown)
        .build();
        assert_eq!(
            delivery,
            Err(ActionRecordValidationError::RefusedCannotHaveDelivery)
        );

        let evidence = ActionExecutionRecord::builder(
            ActionEffect::Refused,
            ActionTransport::BrowserCdpInputMouse,
            RequestedDelivery::NotApplicable,
        )
        .evidence(evidence())
        .build();
        assert_eq!(
            evidence,
            Err(ActionRecordValidationError::RefusedCannotHaveEvidence)
        );
    }

    #[test]
    fn projection_contains_only_outcome_information() {
        let projection = ActionExecutionRecord::builder(
            ActionEffect::Confirmed,
            ActionTransport::LinuxLibei,
            RequestedDelivery::Foreground,
        )
        .actual_delivery(ActualDelivery::Foreground)
        .evidence(evidence())
        .delivered_count(1)
        .detail("portal session accepted input")
        .build()
        .unwrap()
        .stable_projection()
        .unwrap();

        assert_eq!(projection.effect, ActionEffect::Confirmed);
        assert_eq!(projection.route, ActionRoute::GlobalInput);
        assert_eq!(
            projection.delivery,
            Some(ActionDeliveryProjection {
                actual: ActualDelivery::Foreground,
                delivered_count: Some(1),
            })
        );
        assert!(projection.evidence.is_some());
    }

    #[test]
    fn projection_filters_non_publishable_evidence() {
        let record = ActionExecutionRecord::builder(
            ActionEffect::Unverifiable,
            ActionTransport::MacosCgEventHid,
            RequestedDelivery::Foreground,
        )
        .evidence(ActionEvidence {
            kind: EvidenceKind::ScreenshotComparison,
            detail: "pixels changed".to_owned(),
        })
        .build()
        .unwrap();

        assert_eq!(record.stable_projection().unwrap().evidence, None);
    }

    #[test]
    fn escalation_projection_preserves_decision_critical_reasons() {
        use cua_driver_contract::{ActionEscalationReason, ActionEscalationTarget};

        let cases = [
            (
                EscalationKind::RetryWithPixelTarget,
                ActionEscalationTarget::Pixel,
                ActionEscalationReason::EffectUnconfirmed,
            ),
            (
                EscalationKind::RetryWithPageAction,
                ActionEscalationTarget::Page,
                ActionEscalationReason::EffectUnconfirmed,
            ),
            (
                EscalationKind::RefreshPageState,
                ActionEscalationTarget::Page,
                ActionEscalationReason::RouteUnavailable,
            ),
            (
                EscalationKind::RetryWithForegroundDelivery,
                ActionEscalationTarget::Foreground,
                ActionEscalationReason::DeliveryFailed,
            ),
            (
                EscalationKind::RequestPermission,
                ActionEscalationTarget::Session,
                ActionEscalationReason::PermissionRequired,
            ),
            (
                EscalationKind::ExpandCaptureScope,
                ActionEscalationTarget::Session,
                ActionEscalationReason::RouteUnavailable,
            ),
            (
                EscalationKind::PrepareSession,
                ActionEscalationTarget::Session,
                ActionEscalationReason::RouteUnavailable,
            ),
        ];

        for (kind, target, reason) in cases {
            let result = ActionExecutionRecord::builder(
                ActionEffect::Unverifiable,
                ActionTransport::MacosCgEventPid,
                RequestedDelivery::Background,
            )
            .escalation(ActionEscalation { kind, detail: None })
            .build()
            .unwrap()
            .public_result()
            .unwrap();
            assert_eq!(
                result.escalation,
                Some(cua_driver_contract::ActionEscalation { target, reason })
            );
        }

        let permission = ActionExecutionRecord::builder(
            ActionEffect::SuspectedNoop,
            ActionTransport::MacosCgEventPid,
            RequestedDelivery::Background,
        )
        .escalation(ActionEscalation {
            kind: EscalationKind::RequestPermission,
            detail: None,
        })
        .build()
        .unwrap()
        .public_result()
        .unwrap();
        assert_eq!(
            permission.escalation.unwrap().reason,
            ActionEscalationReason::PermissionRequired,
            "suspected-noop classification must not hide a permission blocker"
        );
    }

    #[test]
    fn public_result_is_the_closed_contract_not_a_serialized_debug_record() {
        let result = ActionExecutionRecord::builder(
            ActionEffect::Confirmed,
            ActionTransport::MacosAxValue,
            RequestedDelivery::Background,
        )
        .actual_delivery(ActualDelivery::Background)
        .evidence(ActionEvidence {
            kind: EvidenceKind::AccessibilityReadback,
            detail: "secret request-adjacent diagnostic".to_owned(),
        })
        .detail("pid=42 window_id=7 x=10 y=20")
        .build()
        .unwrap()
        .public_result()
        .unwrap();

        let value = serde_json::to_value(result).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "effect": "confirmed",
                "route": "accessibility",
                "delivery": {"mode": "background"},
                "evidence": [{"kind": "value_readback"}]
            })
        );
        let rendered = value.to_string();
        for forbidden in [
            "secret",
            "pid",
            "window_id",
            "\"x\"",
            "\"y\"",
            "macos_ax_value",
            "requested_delivery",
        ] {
            assert!(
                !rendered.contains(forbidden),
                "public ActionResult leaked {forbidden}: {rendered}"
            );
        }
    }

    #[test]
    fn system_api_value_readback_projects_without_transport_details() {
        let result = ActionExecutionRecord::builder(
            ActionEffect::Confirmed,
            ActionTransport::WindowsSetWindowPos,
            RequestedDelivery::NotApplicable,
        )
        .actual_delivery(ActualDelivery::NotApplicable)
        .evidence(ActionEvidence {
            kind: EvidenceKind::ValueReadback,
            detail: "GetWindowRect matched exact HWND geometry".into(),
        })
        .build()
        .unwrap()
        .public_result()
        .unwrap();

        assert_eq!(
            serde_json::to_value(result).unwrap(),
            serde_json::json!({
                "effect": "confirmed",
                "route": "system_api",
                "delivery": {"mode": "not_applicable"},
                "evidence": [{"kind": "value_readback"}]
            })
        );
    }

    #[test]
    fn confirmed_rejects_debug_only_evidence() {
        let record = ActionExecutionRecord::builder(
            ActionEffect::Confirmed,
            ActionTransport::MacosCgEventHid,
            RequestedDelivery::Foreground,
        )
        .evidence(ActionEvidence {
            kind: EvidenceKind::ScreenshotComparison,
            detail: "pixels changed".to_owned(),
        })
        .build();
        assert_eq!(
            record,
            Err(ActionRecordValidationError::ConfirmedRequiresEvidence)
        );
    }

    #[test]
    fn legacy_value_readback_preserves_truth_without_echoing_target() {
        let args = serde_json::json!({
            "pid": 42,
            "window_id": 77,
            "x": 12,
            "y": 18,
            "delivery_mode": "background",
        });
        let structured = serde_json::json!({
            "path": "ax",
            "verified": true,
            "verify": "confirmed",
            "effect": "confirmed",
        });
        let record = ActionExecutionRecord::from_legacy("type_text", &args, &structured)
            .expect("legacy action should normalize");
        assert_eq!(record.effect, ActionEffect::Confirmed);
        assert_eq!(record.requested_delivery, RequestedDelivery::Background);
        assert_eq!(record.actual_delivery, Some(ActualDelivery::Background));
        let debug = record.debug_json();
        let rendered = debug.to_string();
        assert!(!rendered.contains("\"pid\""));
        assert!(!rendered.contains("\"window_id\""));
        assert!(!rendered.contains("\"x\""));
        assert!(!rendered.contains("\"y\""));
    }

    #[test]
    fn legacy_pointer_and_key_acknowledgements_cannot_become_confirmed() {
        for tool in [
            "click",
            "double_click",
            "right_click",
            "scroll",
            "drag",
            "mouse_drag",
            "parallel_mouse_drag",
            "mouse_button_down",
            "mouse_button_up",
            "press_key",
            "hotkey",
        ] {
            let record = ActionExecutionRecord::from_legacy(
                tool,
                &serde_json::json!({"delivery_mode": "background"}),
                &serde_json::json!({
                    "path": "ax",
                    "verified": true,
                    "verify": "confirmed",
                    "effect": "confirmed",
                }),
            )
            .unwrap_or_else(|| panic!("{tool} legacy action should normalize"));
            assert_eq!(
                record.effect,
                ActionEffect::Unverifiable,
                "{tool} cannot confirm from a delivery acknowledgement"
            );
            assert!(record.evidence.is_empty());
        }
    }

    #[test]
    fn click_selection_readback_preserves_confirmed_effect() {
        let record = ActionExecutionRecord::from_legacy(
            "click",
            &serde_json::json!({"delivery_mode": "background"}),
            &serde_json::json!({
                "path": "cgevent",
                "verified": true,
                "effect": "confirmed",
                "evidence": [{"kind": "accessibility_readback"}],
            }),
        )
        .expect("verified selection click should normalize");

        assert_eq!(record.effect, ActionEffect::Confirmed);
        assert_eq!(record.evidence.len(), 1);
        assert_eq!(record.evidence[0].kind, EvidenceKind::AccessibilityReadback);
        assert_eq!(
            serde_json::to_value(record.public_result().expect("public result")).unwrap(),
            serde_json::json!({
                "effect": "confirmed",
                "route": "synthetic_events",
                "delivery": {"mode": "background"},
                "evidence": [{"kind": "value_readback"}],
            })
        );
    }

    #[test]
    fn browser_pointer_routes_share_the_closed_action_contract() {
        for (input_route, expected_route) in [
            ("trusted", cua_driver_contract::ActionRoute::TrustedInput),
            ("dom_event", cua_driver_contract::ActionRoute::Dom),
        ] {
            let args = serde_json::json!({
                "session": "browser-test",
                "target_id": "secret-target",
                "tab_id": "secret-tab",
                "action": "drag",
                "ref": "secret-ref",
                "destination_ref": "secret-destination",
                "input_route": input_route,
            });
            let structured = serde_json::json!({
                "status": "ok",
                "route": input_route,
                "target_id": "secret-target",
                "tab_id": "secret-tab",
                "ref": "secret-ref",
                "destination_ref": "secret-destination",
                "x": 10,
                "y": 20,
            });
            let record = ActionExecutionRecord::from_legacy("browser_pointer", &args, &structured)
                .expect("browser pointer action should normalize");
            let public = record
                .public_result()
                .expect("browser pointer action should publish");
            assert_eq!(
                public.effect,
                cua_driver_contract::ActionEffect::Unverifiable
            );
            assert_eq!(public.route, expected_route);
            assert_eq!(
                public.delivery.as_ref().map(|delivery| delivery.mode),
                Some(cua_driver_contract::ActionDeliveryMode::Background)
            );
            let rendered = serde_json::to_string(&public).expect("serialize ActionResult");
            for forbidden in [
                "secret-target",
                "secret-tab",
                "secret-ref",
                "secret-destination",
                "\"x\"",
                "\"y\"",
            ] {
                assert!(
                    !rendered.contains(forbidden),
                    "browser ActionResult leaked {forbidden}: {rendered}"
                );
            }
        }
    }

    #[test]
    fn browser_refusals_and_partial_delivery_survive_projection() {
        let refused = ActionExecutionRecord::from_legacy(
            "browser_click",
            &serde_json::json!({"input_route": "trusted"}),
            &serde_json::json!({
                "status": "refused",
                "refusal": {
                    "code": "browser_ref_stale",
                    "message": "refresh browser state"
                }
            }),
        )
        .expect("browser refusal should normalize");
        let refused = refused.public_result().expect("refusal should project");
        assert_eq!(refused.effect, cua_driver_contract::ActionEffect::Refused);
        assert!(refused.delivery.is_none());
        assert_eq!(
            refused.escalation,
            Some(cua_driver_contract::ActionEscalation {
                target: cua_driver_contract::ActionEscalationTarget::Page,
                reason: cua_driver_contract::ActionEscalationReason::RouteUnavailable,
            })
        );

        let partial = ActionExecutionRecord::from_legacy(
            "browser_type",
            &serde_json::json!({}),
            &serde_json::json!({
                "status": "refused",
                "refusal": {
                    "code": "browser_input_incomplete",
                    "message": "typing stopped",
                    "detail": {
                        "requested_chars": 4,
                        "delivered_chars": 2
                    }
                }
            }),
        )
        .expect("partial browser input should normalize");
        let partial = partial.public_result().expect("partial should project");
        assert_eq!(partial.effect, cua_driver_contract::ActionEffect::Partial);
        assert_eq!(
            partial
                .delivery
                .as_ref()
                .and_then(|delivery| delivery.delivered_count),
            Some(2)
        );
        assert!(partial.escalation.is_none());
    }

    #[test]
    fn successful_foreground_pixel_branch_reports_actual_global_delivery() {
        let record = ActionExecutionRecord::from_legacy(
            "click",
            &serde_json::json!({"delivery_mode": "foreground"}),
            &serde_json::json!({
                "path": "pixel",
                "verified": false,
                "effect": "unverifiable",
            }),
        )
        .expect("foreground pixel action should normalize");
        assert_eq!(record.actual_delivery, Some(ActualDelivery::Foreground));
        assert_eq!(record.transport.route(), ActionRoute::GlobalInput);
        let public = record.public_result().expect("public ActionResult");
        assert_eq!(
            public.delivery.map(|delivery| delivery.mode),
            Some(cua_driver_contract::ActionDeliveryMode::Foreground)
        );
        assert_eq!(public.route, cua_driver_contract::ActionRoute::GlobalInput);
    }

    #[test]
    fn producer_emitted_delivery_wins_over_requested_delivery() {
        let record = ActionExecutionRecord::from_legacy(
            "scroll",
            &serde_json::json!({"delivery_mode": "foreground"}),
            &serde_json::json!({
                "path": "uia",
                "delivery_mode": "background",
                "verified": false,
                "effect": "unverifiable",
            }),
        )
        .expect("producer delivery fact should normalize");
        assert_eq!(record.actual_delivery, Some(ActualDelivery::Background));
        assert_eq!(record.transport.route(), ActionRoute::Accessibility);
    }

    #[test]
    fn legacy_confirmation_without_readback_is_downgraded() {
        let record = ActionExecutionRecord::from_legacy(
            "type_text",
            &serde_json::json!({"delivery_mode": "background"}),
            &serde_json::json!({
                "path": "key_events",
                "effect": "confirmed",
                "characters": 3,
            }),
        )
        .expect("legacy action should normalize");
        assert_eq!(record.effect, ActionEffect::Unverifiable);
        assert!(record.evidence.is_empty());
        assert_eq!(
            record.delivered_count, None,
            "legacy request-count echoes are not delivery evidence"
        );
    }

    #[test]
    fn legacy_partial_requires_an_explicit_delivered_count() {
        assert!(
            ActionExecutionRecord::from_legacy(
                "browser_type",
                &serde_json::json!({}),
                &serde_json::json!({
                    "route": "trusted",
                    "effect": "partial",
                    "chars": 3,
                    "requested_chars": 3,
                }),
            )
            .is_none(),
            "requested character counts must not be published as delivered counts"
        );

        let record = ActionExecutionRecord::from_legacy(
            "browser_type",
            &serde_json::json!({}),
            &serde_json::json!({
                "route": "trusted",
                "effect": "partial",
                "requested_chars": 3,
                "delivered_chars": 2,
            }),
        )
        .expect("an explicit delivered count should normalize");
        assert_eq!(record.delivered_count, Some(2));
    }

    #[test]
    fn every_known_legacy_action_path_normalizes() {
        let paths = [
            "ax",
            "ax_fg",
            "uia",
            "uia_expand_collapse",
            "msaa",
            "post_message",
            "PostMessage",
            "SendInput",
            "SetCursorPos",
            "atspi",
            "wayland_atspi",
            "x11_atspi",
            "pty",
            "x11_pixel",
            "x11_pixel_fg",
            "x11_xtest_fg",
            "xtest",
            "xtest_desktop",
            "wayland_activate",
            "wayland_focused",
            "wayland_desktop",
            "cua_compositor_inject",
            "wayland_cua_compositor",
            "hid",
            "cgevent_hid",
            "cgevent",
            "cgevent_fg",
            "dom_event",
            "trusted",
            "key_events",
            "key_events_fg",
            "pixel",
        ];
        for path in paths {
            assert!(
                ActionExecutionRecord::from_legacy(
                    "click",
                    &serde_json::json!({"delivery_mode": "background"}),
                    &serde_json::json!({
                        "path": path,
                        "effect": "unverifiable",
                    }),
                )
                .is_some(),
                "legacy path {path} must normalize before the breaking cutover"
            );
        }
    }
}
