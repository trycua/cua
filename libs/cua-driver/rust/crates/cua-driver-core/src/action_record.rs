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
    LinuxAtSpiAction,
    LinuxAtSpiValue,
    LinuxXSendEvent,
    LinuxXTest,
    LinuxLibei,
    LinuxWaylandVirtualPointer,
    LinuxCuaCompositorInject,
    BrowserCdpInputMouse,
    BrowserCdpInputKey,
    BrowserCdpRuntimeFunction,
}

impl ActionTransport {
    pub const ALL: &'static [Self] = &[
        Self::AgentCursorOverlay,
        Self::MacosAxAction,
        Self::MacosAxValue,
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
        Self::LinuxAtSpiAction,
        Self::LinuxAtSpiValue,
        Self::LinuxXSendEvent,
        Self::LinuxXTest,
        Self::LinuxLibei,
        Self::LinuxWaylandVirtualPointer,
        Self::LinuxCuaCompositorInject,
        Self::BrowserCdpInputMouse,
        Self::BrowserCdpInputKey,
        Self::BrowserCdpRuntimeFunction,
    ];

    pub const fn route(self) -> ActionRoute {
        match self {
            Self::AgentCursorOverlay => ActionRoute::SyntheticEvents,
            Self::MacosAxAction
            | Self::MacosAxValue
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
            Self::MacosCgEventPid | Self::WindowsPostMessage | Self::LinuxXSendEvent => {
                ActionRoute::SyntheticEvents
            }
            Self::MacosCgEventHid
            | Self::WindowsTargetedInjection
            | Self::WindowsSendInput
            | Self::WindowsSetCursorPos
            | Self::WindowsShellExecute
            | Self::LinuxXTest
            | Self::LinuxLibei
            | Self::LinuxWaylandVirtualPointer
            | Self::LinuxCuaCompositorInject => ActionRoute::GlobalInput,
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
    RequestPermission,
    ElevateAccess,
    ExpandCaptureScope,
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
            ActionEffect::Partial if self.delivered_count.is_none() => {
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

    /// Normalize the legacy hand-written payload at the canonical dispatch
    /// seam without changing the MCP bytes returned by the tool.
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
            actual_delivery_from_legacy(tool_name, args, raw_path, transport, effect);

        let mut record = Self::new(effect, transport, requested_delivery);
        record.actual_delivery = actual_delivery;
        record.delivered_count = structured
            .get("delivered_chars")
            .or_else(|| structured.get("characters"))
            .or_else(|| structured.get("chars"))
            .and_then(serde_json::Value::as_u64)
            .and_then(|count| u32::try_from(count).ok());

        if structured
            .get("verified")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
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
    matches!(
        tool_name,
        "click"
            | "double_click"
            | "right_click"
            | "scroll"
            | "drag"
            | "mouse_drag"
            | "parallel_mouse_drag"
            | "move_cursor"
            | "mouse_button_down"
            | "mouse_button_up"
            | "type_text"
            | "type_text_chars"
            | "press_key"
            | "hotkey"
            | "set_value"
            | "browser_click"
            | "browser_type"
    )
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
        _ if matches!(tool_name, "browser_click" | "browser_type") => {
            RequestedDelivery::NotApplicable
        }
        _ => RequestedDelivery::Background,
    }
}

fn legacy_effect(structured: &serde_json::Value) -> ActionEffect {
    match structured.get("effect").and_then(serde_json::Value::as_str) {
        Some("confirmed") => ActionEffect::Confirmed,
        Some("partial") => ActionEffect::Partial,
        Some("suspected_noop") => ActionEffect::SuspectedNoop,
        Some("refused") => ActionEffect::Refused,
        _ => ActionEffect::Unverifiable,
    }
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
        "uia" => ActionTransport::WindowsUiaInvoke,
        "uia_expand_collapse" => ActionTransport::WindowsUiaExpandCollapse,
        "msaa" => ActionTransport::WindowsMsaaAction,
        "post_message" | "PostMessage" => ActionTransport::WindowsPostMessage,
        "SendInput" => ActionTransport::WindowsSendInput,
        "SetCursorPos" => ActionTransport::WindowsSetCursorPos,
        "atspi" | "wayland_atspi" | "x11_atspi" => ActionTransport::LinuxAtSpiAction,
        "x11_pixel" | "x11_pixel_fg" | "x11_xtest_fg" | "xtest" | "xtest_desktop" => {
            ActionTransport::LinuxXTest
        }
        "wayland_activate" | "wayland_focused" => ActionTransport::LinuxLibei,
        "wayland_desktop" => ActionTransport::LinuxWaylandVirtualPointer,
        "cua_compositor_inject" | "wayland_cua_compositor" => {
            ActionTransport::LinuxCuaCompositorInject
        }
        "hid" | "cgevent_hid" | "cgevent_fg" => ActionTransport::MacosCgEventHid,
        "cgevent" => ActionTransport::MacosCgEventPid,
        "dom_event" => ActionTransport::BrowserCdpRuntimeFunction,
        "trusted" => ActionTransport::BrowserCdpInputMouse,
        "key_events" | "key_events_fg" => {
            if cfg!(target_os = "macos") {
                if path.ends_with("_fg") {
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
                    ActionTransport::WindowsPostMessage
                }
            } else {
                if path.ends_with("_fg") {
                    ActionTransport::LinuxXTest
                } else {
                    ActionTransport::LinuxXSendEvent
                }
            }
        }
        "pixel" => {
            if cfg!(target_os = "macos") {
                ActionTransport::MacosCgEventPid
            } else if cfg!(target_os = "windows") {
                ActionTransport::WindowsTargetedInjection
            } else {
                ActionTransport::LinuxXTest
            }
        }
        "" if tool_name == "browser_click" => {
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
                ActionTransport::MacosCgEventPid
            } else if cfg!(target_os = "windows") {
                ActionTransport::WindowsTargetedInjection
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
    raw_path: Option<&str>,
    transport: ActionTransport,
    effect: ActionEffect,
) -> Option<ActualDelivery> {
    if effect == ActionEffect::Refused {
        return None;
    }
    if matches!(tool_name, "browser_click" | "browser_type") {
        return Some(ActualDelivery::Background);
    }
    if transport == ActionTransport::AgentCursorOverlay
        || args.get("scope").and_then(serde_json::Value::as_str) == Some("desktop")
    {
        return Some(ActualDelivery::NotApplicable);
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

fn projected_evidence(evidence: &[ActionEvidence]) -> Option<Vec<ActionEvidenceProjection>> {
    let evidence: Vec<_> = evidence
        .iter()
        .filter_map(|evidence| {
            let kind = match evidence.kind {
                EvidenceKind::AccessibilityReadback => ProjectedEvidenceKind::AccessibilityReadback,
                EvidenceKind::BrowserReadback => ProjectedEvidenceKind::BrowserReadback,
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
        EscalationKind::RequestPermission => "request_permission",
        EscalationKind::ElevateAccess => "elevate_access",
        EscalationKind::ExpandCaptureScope => "expand_capture_scope",
        EscalationKind::RetryWithForegroundDelivery => "retry_with_foreground_delivery",
    }
}

fn transport_name(transport: ActionTransport) -> &'static str {
    match transport {
        ActionTransport::AgentCursorOverlay => "agent_cursor_overlay",
        ActionTransport::MacosAxAction => "macos_ax_action",
        ActionTransport::MacosAxValue => "macos_ax_value",
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
        ActionTransport::LinuxAtSpiAction => "linux_at_spi_action",
        ActionTransport::LinuxAtSpiValue => "linux_at_spi_value",
        ActionTransport::LinuxXSendEvent => "linux_x_send_event",
        ActionTransport::LinuxXTest => "linux_x_test",
        ActionTransport::LinuxLibei => "linux_libei",
        ActionTransport::LinuxWaylandVirtualPointer => "linux_wayland_virtual_pointer",
        ActionTransport::LinuxCuaCompositorInject => "linux_cua_compositor_inject",
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
    fn every_transport_maps_to_one_of_the_five_stable_routes() {
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
    fn legacy_normalization_preserves_wire_facts_without_echoing_target() {
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
        let record = ActionExecutionRecord::from_legacy("click", &args, &structured)
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
