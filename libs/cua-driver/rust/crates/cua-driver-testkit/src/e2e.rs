//! Typed contracts and result reporting for desktop E2E cells.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub const DECLARATION_SCHEMA: &str = "cua-e2e-case/v2";
pub const ENVIRONMENT_SCHEMA: &str = "cua-e2e-environment/v2";
pub const RESULT_SCHEMA: &str = "cua-e2e-result/v2";

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Windows,
    Macos,
    Linux,
}

impl Platform {
    pub fn current() -> Self {
        #[cfg(target_os = "windows")]
        {
            Self::Windows
        }
        #[cfg(target_os = "macos")]
        {
            Self::Macos
        }
        #[cfg(target_os = "linux")]
        {
            Self::Linux
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        {
            panic!("unsupported E2E platform")
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayServer {
    Win32,
    Quartz,
    X11,
    Wayland,
}

impl DisplayServer {
    pub fn current() -> Self {
        #[cfg(target_os = "windows")]
        {
            Self::Win32
        }
        #[cfg(target_os = "macos")]
        {
            Self::Quartz
        }
        #[cfg(target_os = "linux")]
        {
            match std::env::var("XDG_SESSION_TYPE")
                .unwrap_or_else(|_| "x11".to_owned())
                .to_ascii_lowercase()
                .as_str()
            {
                "wayland" => Self::Wayland,
                _ => Self::X11,
            }
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        {
            panic!("unsupported E2E display server")
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Targeting {
    Ax,
    Px,
    Page,
    NotApplicable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    Background,
    Foreground,
    NotApplicable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Window,
    Desktop,
    NotApplicable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DriverRoute {
    AxRead,
    WindowState,
    UiaInvoke,
    UiaToggle,
    UiaSelection,
    UiaExpandCollapse,
    UiaValue,
    UiaRangeValue,
    UiaScroll,
    PostMessage,
    WindowsTargetedInjection,
    WindowsSendInput,
    WindowsShellExecute,
    WindowsPrintWindow,
    WindowsOverlay,
    MacosAxAction,
    MacosAxValue,
    MacosCgEventPid,
    MacosCgEventHid,
    LinuxAtSpiAction,
    LinuxAtSpiValue,
    LinuxXSendEvent,
    LinuxXTest,
    LinuxLibei,
    LinuxWaylandVirtualPointer,
    Cdp,
    Composite,
}

pub fn shared_web_route(
    platform: Platform,
    display_server: DisplayServer,
    action: &str,
    targeting: Targeting,
    delivery: Delivery,
) -> Result<DriverRoute, String> {
    use DriverRoute as Route;

    let pointer_or_key_route = |background, foreground| match delivery {
        Delivery::Background => Ok(background),
        Delivery::Foreground => Ok(foreground),
        Delivery::NotApplicable => Err(format!("{action}: delivery mode is required")),
    };
    match (platform, display_server, targeting, action) {
        (Platform::Windows, DisplayServer::Win32, Targeting::Px, _) => {
            pointer_or_key_route(Route::WindowsTargetedInjection, Route::WindowsSendInput)
        }
        (Platform::Windows, DisplayServer::Win32, Targeting::Ax, "left_click")
        | (Platform::Windows, DisplayServer::Win32, Targeting::Ax, "child_window") => {
            Ok(Route::UiaInvoke)
        }
        (Platform::Windows, DisplayServer::Win32, Targeting::Ax, "type_text") => {
            Ok(Route::UiaValue)
        }
        (Platform::Windows, DisplayServer::Win32, Targeting::Ax, "scroll") => {
            Ok(Route::UiaScroll)
        }
        (
            Platform::Windows,
            DisplayServer::Win32,
            Targeting::Ax,
            "right_click" | "double_click" | "press_key" | "hotkey",
        ) => pointer_or_key_route(Route::PostMessage, Route::WindowsSendInput),
        (Platform::Windows, DisplayServer::Win32, Targeting::Ax, "editor_save") => {
            Ok(Route::Composite)
        }

        (Platform::Macos, DisplayServer::Quartz, Targeting::Px, _) => {
            pointer_or_key_route(Route::MacosCgEventPid, Route::MacosCgEventHid)
        }
        (Platform::Macos, DisplayServer::Quartz, Targeting::Ax, "left_click")
        | (Platform::Macos, DisplayServer::Quartz, Targeting::Ax, "child_window")
        | (Platform::Macos, DisplayServer::Quartz, Targeting::Ax, "scroll") => {
            Ok(Route::MacosAxAction)
        }
        (Platform::Macos, DisplayServer::Quartz, Targeting::Ax, "type_text") => {
            Ok(Route::MacosAxValue)
        }
        (
            Platform::Macos,
            DisplayServer::Quartz,
            Targeting::Ax,
            "right_click" | "double_click" | "press_key" | "hotkey",
        ) => pointer_or_key_route(Route::MacosCgEventPid, Route::MacosCgEventHid),
        (Platform::Macos, DisplayServer::Quartz, Targeting::Ax, "editor_save") => {
            Ok(Route::Composite)
        }

        (Platform::Linux, DisplayServer::Wayland, Targeting::Px, _) => {
            Ok(Route::LinuxWaylandVirtualPointer)
        }
        (Platform::Linux, DisplayServer::X11, Targeting::Px, _) => {
            pointer_or_key_route(Route::LinuxXSendEvent, Route::LinuxXTest)
        }
        (
            Platform::Linux,
            DisplayServer::X11 | DisplayServer::Wayland,
            Targeting::Ax,
            "left_click" | "child_window" | "scroll",
        ) => Ok(Route::LinuxAtSpiAction),
        (
            Platform::Linux,
            DisplayServer::X11 | DisplayServer::Wayland,
            Targeting::Ax,
            "type_text",
        ) => Ok(Route::LinuxAtSpiValue),
        (
            Platform::Linux,
            DisplayServer::X11,
            Targeting::Ax,
            "right_click" | "double_click" | "press_key" | "hotkey",
        ) => pointer_or_key_route(Route::LinuxXSendEvent, Route::LinuxXTest),
        (
            Platform::Linux,
            DisplayServer::Wayland,
            Targeting::Ax,
            "right_click" | "double_click" | "press_key" | "hotkey",
        ) => Ok(Route::LinuxWaylandVirtualPointer),
        (
            Platform::Linux,
            DisplayServer::X11 | DisplayServer::Wayland,
            Targeting::Ax,
            "editor_save",
        ) => Ok(Route::Composite),
        _ => Err(format!(
            "no shared route for {platform:?}/{display_server:?}/{action}/{targeting:?}/{delivery:?}"
        )),
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OracleKind {
    FixtureState,
    AxState,
    Pixels,
    Focus,
    ZOrder,
    Cursor,
    NoLeakedInput,
    Protocol,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusalCode {
    BackgroundUnavailable,
    BackgroundOccluded,
    BackgroundUipiBlocked,
}

impl RefusalCode {
    pub fn from_driver_code(code: &str) -> Option<Self> {
        match code {
            "background_unavailable" => Some(Self::BackgroundUnavailable),
            "background_occluded" => Some(Self::BackgroundOccluded),
            "background_uipi_blocked" => Some(Self::BackgroundUipiBlocked),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ContractExpectation {
    Deliver,
    Refuse { allowed_codes: Vec<RefusalCode> },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TestStatus {
    Pass,
    Fail,
    Skip,
    EnvironmentError,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentStatus {
    Ready,
    Error,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EnvironmentRecord {
    pub schema: String,
    pub platform: Platform,
    pub display_server: DisplayServer,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_sha: Option<String>,
    pub status: EnvironmentStatus,
    pub duration_ms: u128,
    pub message: String,
}

impl EnvironmentRecord {
    pub fn ready(duration: Duration) -> Self {
        Self {
            schema: ENVIRONMENT_SCHEMA.to_owned(),
            platform: Platform::current(),
            display_server: DisplayServer::current(),
            source_sha: source_sha_from_env(),
            status: EnvironmentStatus::Ready,
            duration_ms: duration.as_millis(),
            message: String::new(),
        }
    }

    pub fn error(duration: Duration, message: impl Into<String>) -> Self {
        Self {
            schema: ENVIRONMENT_SCHEMA.to_owned(),
            platform: Platform::current(),
            display_server: DisplayServer::current(),
            source_sha: source_sha_from_env(),
            status: EnvironmentStatus::Error,
            duration_ms: duration.as_millis(),
            message: message.into(),
        }
    }
}

fn source_sha_from_env() -> Option<String> {
    std::env::var("CUA_E2E_SOURCE_SHA")
        .ok()
        .filter(|sha| !sha.is_empty())
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedBehavior {
    Delivered,
    Refused,
    NoEffect,
    Error,
    NotRun,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Evidence {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trajectory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screenshot: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CaseSpec {
    pub cell_id: String,
    pub platform: Platform,
    pub display_server: DisplayServer,
    pub harness: String,
    pub toolkit: String,
    pub action: String,
    pub targeting: Targeting,
    pub delivery: Delivery,
    pub scope: Scope,
    pub driver_route: DriverRoute,
    pub expected_behavior: ContractExpectation,
    pub oracles: Vec<OracleKind>,
}

impl CaseSpec {
    #[allow(clippy::too_many_arguments)]
    pub fn delivered(
        cell_id: impl Into<String>,
        harness: impl Into<String>,
        toolkit: impl Into<String>,
        action: impl Into<String>,
        targeting: Targeting,
        delivery: Delivery,
        scope: Scope,
        driver_route: DriverRoute,
        oracles: Vec<OracleKind>,
    ) -> Self {
        Self {
            cell_id: cell_id.into(),
            platform: Platform::current(),
            display_server: DisplayServer::current(),
            harness: harness.into(),
            toolkit: toolkit.into(),
            action: action.into(),
            targeting,
            delivery,
            scope,
            driver_route,
            expected_behavior: ContractExpectation::Deliver,
            oracles,
        }
    }

    pub fn expecting_refusal(mut self, allowed_codes: Vec<RefusalCode>) -> Self {
        self.expected_behavior = ContractExpectation::Refuse { allowed_codes };
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.cell_id.is_empty()
            || !self
                .cell_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(format!("{}: cell id is not artifact-safe", self.cell_id));
        }
        if self.oracles.is_empty() {
            return Err(format!("{}: no external oracle declared", self.cell_id));
        }
        if let ContractExpectation::Refuse { allowed_codes } = &self.expected_behavior {
            if self.delivery != Delivery::Background {
                return Err(format!(
                    "{}: only background delivery may declare refusal",
                    self.cell_id
                ));
            }
            if allowed_codes.is_empty() {
                return Err(format!("{}: refusal has no allowed code", self.cell_id));
            }
            for required in [
                OracleKind::Focus,
                OracleKind::ZOrder,
                OracleKind::NoLeakedInput,
            ] {
                if !self.oracles.contains(&required) {
                    return Err(format!(
                        "{}: refusal is missing {:?} oracle",
                        self.cell_id, required
                    ));
                }
            }
        }
        Ok(())
    }
}

fn native_cell_id(toolkit: &str, action: &str, targeting: Targeting, delivery: Delivery) -> String {
    let targeting = match targeting {
        Targeting::Ax => "ax",
        Targeting::Px => "px",
        Targeting::Page => "page",
        Targeting::NotApplicable => "not-applicable",
    };
    let delivery = match delivery {
        Delivery::Background => "background",
        Delivery::Foreground => "foreground",
        Delivery::NotApplicable => "not-applicable",
    };
    format!(
        "{}-{toolkit}-{action}-{targeting}-{delivery}",
        std::env::consts::OS
    )
    .replace('_', "-")
}

pub fn native_background_case(
    toolkit: &str,
    action: &str,
    targeting: Targeting,
    route: DriverRoute,
) -> CaseSpec {
    let mut oracles = vec![
        OracleKind::FixtureState,
        OracleKind::Focus,
        OracleKind::ZOrder,
        OracleKind::NoLeakedInput,
    ];
    if DisplayServer::current() != DisplayServer::Wayland {
        oracles.push(OracleKind::Cursor);
    }
    CaseSpec::delivered(
        native_cell_id(toolkit, action, targeting, Delivery::Background),
        toolkit,
        toolkit,
        action,
        targeting,
        Delivery::Background,
        Scope::Window,
        route,
        oracles,
    )
}

pub fn native_foreground_case(
    toolkit: &str,
    action: &str,
    targeting: Targeting,
    route: DriverRoute,
) -> CaseSpec {
    CaseSpec::delivered(
        native_cell_id(toolkit, action, targeting, Delivery::Foreground),
        toolkit,
        toolkit,
        action,
        targeting,
        Delivery::Foreground,
        Scope::Window,
        route,
        vec![OracleKind::FixtureState],
    )
}

pub fn native_readonly_case(
    toolkit: &str,
    action: &str,
    targeting: Targeting,
    route: DriverRoute,
    oracles: Vec<OracleKind>,
) -> CaseSpec {
    CaseSpec::delivered(
        native_cell_id(toolkit, action, targeting, Delivery::NotApplicable),
        toolkit,
        toolkit,
        action,
        targeting,
        Delivery::NotApplicable,
        Scope::Window,
        route,
        oracles,
    )
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CaseDeclaration {
    pub schema: String,
    #[serde(flatten)]
    pub case: CaseSpec,
}

impl From<CaseSpec> for CaseDeclaration {
    fn from(case: CaseSpec) -> Self {
        Self {
            schema: DECLARATION_SCHEMA.to_owned(),
            case,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Observation {
    pub behavior: ObservedBehavior,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal_code: Option<RefusalCode>,
    pub passed_oracles: Vec<OracleKind>,
    pub message: String,
    pub evidence: Evidence,
}

impl Observation {
    pub fn delivered(passed_oracles: Vec<OracleKind>, evidence: Evidence) -> Self {
        Self {
            behavior: ObservedBehavior::Delivered,
            refusal_code: None,
            passed_oracles,
            message: String::new(),
            evidence,
        }
    }

    pub fn delivered_with_fixture_state(mut passed_oracles: Vec<OracleKind>) -> Self {
        passed_oracles.push(OracleKind::FixtureState);
        passed_oracles.sort();
        passed_oracles.dedup();
        Self::delivered(passed_oracles, Evidence::default())
    }

    pub fn refused(
        code: RefusalCode,
        passed_oracles: Vec<OracleKind>,
        message: impl Into<String>,
        evidence: Evidence,
    ) -> Self {
        Self {
            behavior: ObservedBehavior::Refused,
            refusal_code: Some(code),
            passed_oracles,
            message: message.into(),
            evidence,
        }
    }

    pub fn error(message: impl Into<String>, evidence: Evidence) -> Self {
        Self {
            behavior: ObservedBehavior::Error,
            refusal_code: None,
            passed_oracles: Vec::new(),
            message: message.into(),
            evidence,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CaseResult {
    pub schema: String,
    #[serde(flatten)]
    pub case: CaseSpec,
    pub test_status: TestStatus,
    pub observed_behavior: ObservedBehavior,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal_code: Option<RefusalCode>,
    pub passed_oracles: Vec<OracleKind>,
    pub duration_ms: u128,
    pub message: String,
    pub evidence: Evidence,
}

impl CaseResult {
    pub fn evaluate(case: CaseSpec, observation: Observation, duration: Duration) -> Self {
        let mut failures = Vec::new();
        if let Err(error) = case.validate() {
            failures.push(error);
        }
        for oracle in &case.oracles {
            if !observation.passed_oracles.contains(oracle) {
                failures.push(format!("missing {:?} oracle", oracle));
            }
        }
        match (&case.expected_behavior, observation.behavior) {
            (ContractExpectation::Deliver, ObservedBehavior::Delivered) => {}
            (ContractExpectation::Deliver, ObservedBehavior::Refused) => {
                failures.push("required delivery was refused".to_owned());
            }
            (ContractExpectation::Refuse { allowed_codes }, ObservedBehavior::Refused) => {
                match observation.refusal_code {
                    Some(code) if allowed_codes.contains(&code) => {}
                    Some(code) => failures.push(format!("unexpected refusal code: {code:?}")),
                    None => failures.push("refusal has no structured code".to_owned()),
                }
            }
            (ContractExpectation::Refuse { .. }, ObservedBehavior::Delivered) => {
                failures.push("unexpected delivery requires contract review".to_owned());
            }
            (_, other) => failures.push(format!("observed behavior was {other:?}")),
        }

        let status = if failures.is_empty() {
            TestStatus::Pass
        } else {
            TestStatus::Fail
        };
        let message = [observation.message, failures.join("; ")]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("; ");
        Self {
            schema: RESULT_SCHEMA.to_owned(),
            case,
            test_status: status,
            observed_behavior: observation.behavior,
            refusal_code: observation.refusal_code,
            passed_oracles: observation.passed_oracles,
            duration_ms: duration.as_millis(),
            message,
            evidence: observation.evidence,
        }
    }
}

#[derive(Debug, Default, Eq, PartialEq)]
pub struct ValidationSummary {
    pub delivered: usize,
    pub refused: usize,
    pub failed: usize,
    pub skipped: usize,
}

impl ValidationSummary {
    pub fn markdown(&self, results: &[CaseResult]) -> String {
        let declarations = results
            .iter()
            .map(|result| result.case.clone())
            .collect::<Vec<_>>();
        self.markdown_with_declarations(&declarations, results)
    }

    pub fn markdown_with_declarations(
        &self,
        declarations: &[CaseSpec],
        results: &[CaseResult],
    ) -> String {
        self.markdown_with_declarations_and_source(declarations, results, None)
    }

    pub fn markdown_with_declarations_and_source(
        &self,
        declarations: &[CaseSpec],
        results: &[CaseResult],
        source_sha: Option<&str>,
    ) -> String {
        let mut output = format!(
            "# CUA Driver E2E\n\n**Result:** {} delivered, {} refused, {} failed, {} skipped\n\n",
            self.delivered, self.refused, self.failed, self.skipped
        );
        if let Some(source_sha) = source_sha {
            output.push_str(&format!("**Source SHA:** `{source_sha}`\n\n"));
        }
        output.push_str("## Declared Coverage\n\n");
        output.push_str(&declared_coverage_markdown(declarations, results));
        output.push_str("\n## Detailed Results\n\n");
        output.push_str(
            "| Cell | Platform | Harness | Action | Targeting | Delivery | Scope | Route | Oracles | Expected | Observed | Status | Duration | Evidence | Details |\n",
        );
        output.push_str("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | ---: | --- | --- |\n");
        for result in results {
            let evidence = result
                .evidence
                .video
                .as_deref()
                .or(result.evidence.trajectory.as_deref())
                .unwrap_or("-");
            let oracles = result
                .case
                .oracles
                .iter()
                .map(|oracle| format!("{oracle:?}"))
                .collect::<Vec<_>>()
                .join(", ");
            let details = if result.message.is_empty() {
                "-".to_owned()
            } else {
                result.message.replace('|', "\\|").replace('\n', " ")
            };
            output.push_str(&format!(
                "| {} | {:?}/{:?} | {} | {} | {:?} | {:?} | {:?} | {:?} | {} | {:?} | {:?} | {:?} | {} ms | {} | {} |\n",
                result.case.cell_id,
                result.case.platform,
                result.case.display_server,
                result.case.harness,
                result.case.action,
                result.case.targeting,
                result.case.delivery,
                result.case.scope,
                result.case.driver_route,
                oracles,
                result.case.expected_behavior,
                result.observed_behavior,
                result.test_status,
                result.duration_ms,
                evidence,
                details,
            ));
        }
        output
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum CoverageColumn {
    AxBackground,
    AxForeground,
    PxBackground,
    PxForeground,
    Page,
    NotApplicable,
}

const COVERAGE_COLUMNS: [CoverageColumn; 6] = [
    CoverageColumn::AxBackground,
    CoverageColumn::AxForeground,
    CoverageColumn::PxBackground,
    CoverageColumn::PxForeground,
    CoverageColumn::Page,
    CoverageColumn::NotApplicable,
];

fn declared_coverage_markdown(declarations: &[CaseSpec], results: &[CaseResult]) -> String {
    let results_by_cell = results
        .iter()
        .map(|result| (result.case.cell_id.as_str(), result))
        .collect::<BTreeMap<_, _>>();
    let mut rows = BTreeMap::<(String, String), BTreeMap<CoverageColumn, Vec<String>>>::new();

    for declaration in declarations {
        let column = coverage_column(declaration);
        let status = results_by_cell
            .get(declaration.cell_id.as_str())
            .map(|result| coverage_status(result))
            .unwrap_or("MISSING");
        let label = match column {
            CoverageColumn::Page => format!("{:?}: {status}", declaration.delivery),
            CoverageColumn::NotApplicable => format!(
                "{:?}/{:?}: {status}",
                declaration.targeting, declaration.delivery
            ),
            _ => status.to_owned(),
        };
        rows.entry((declaration.harness.clone(), declaration.action.clone()))
            .or_default()
            .entry(column)
            .or_default()
            .push(label);
    }

    let mut output = String::from(
        "| Harness | Action | AX/BG | AX/FG | PX/BG | PX/FG | Page | NotApplicable |\n",
    );
    output.push_str("| --- | --- | --- | --- | --- | --- | --- | --- |\n");
    for ((harness, action), cells) in rows {
        output.push_str(&format!(
            "| {} | {} | {} |\n",
            markdown_table_text(&harness),
            markdown_table_text(&action),
            COVERAGE_COLUMNS
                .iter()
                .map(|column| render_coverage_cell(cells.get(column)))
                .collect::<Vec<_>>()
                .join(" | ")
        ));
    }
    output
}

fn coverage_column(case: &CaseSpec) -> CoverageColumn {
    match (case.targeting, case.delivery) {
        (Targeting::Page, _) => CoverageColumn::Page,
        (Targeting::Ax, Delivery::Background) => CoverageColumn::AxBackground,
        (Targeting::Ax, Delivery::Foreground) => CoverageColumn::AxForeground,
        (Targeting::Px, Delivery::Background) => CoverageColumn::PxBackground,
        (Targeting::Px, Delivery::Foreground) => CoverageColumn::PxForeground,
        _ => CoverageColumn::NotApplicable,
    }
}

fn coverage_status(result: &CaseResult) -> &'static str {
    match (result.test_status, result.observed_behavior) {
        (TestStatus::Pass, ObservedBehavior::Delivered) => "PASS",
        (TestStatus::Pass, ObservedBehavior::Refused) => "REFUSED",
        (TestStatus::Pass, _) => "INVALID",
        (TestStatus::Fail | TestStatus::EnvironmentError, _) => "FAIL",
        (TestStatus::Skip, _) => "SKIP",
    }
}

fn render_coverage_cell(labels: Option<&Vec<String>>) -> String {
    let Some(labels) = labels else {
        return "-".to_owned();
    };
    let mut counts = BTreeMap::new();
    for label in labels {
        *counts.entry(label).or_insert(0usize) += 1;
    }
    counts
        .into_iter()
        .map(|(label, count)| {
            if count == 1 {
                label.clone()
            } else {
                format!("{label} ({count})")
            }
        })
        .collect::<Vec<_>>()
        .join("<br>")
}

fn markdown_table_text(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', " ")
}

pub fn append_json_line<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")
}

pub fn write_declaration_from_env(case: &CaseSpec) -> io::Result<()> {
    let Some(path) = std::env::var_os("CUA_E2E_DECLARATIONS_FILE") else {
        return Ok(());
    };
    append_json_line(&PathBuf::from(path), &CaseDeclaration::from(case.clone()))
}

pub fn write_result_from_env(result: &CaseResult) -> io::Result<()> {
    let Some(path) = std::env::var_os("CUA_E2E_RESULTS_FILE") else {
        return Ok(());
    };
    append_json_line(&PathBuf::from(path), result)
}

/// Execute one declared E2E cell, persist its typed result even when the body
/// panics, and preserve Cargo's failing-test signal.
pub fn execute_case(case: CaseSpec, test: impl FnOnce(&mut Evidence) -> Observation) -> CaseResult {
    write_declaration_from_env(&case).expect("write E2E case declaration");
    let started = Instant::now();
    let mut evidence = Evidence::default();
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| test(&mut evidence)));
    let observation = match outcome {
        Ok(mut observation) => {
            if observation.evidence == Evidence::default() {
                observation.evidence = evidence;
            }
            observation
        }
        Err(payload) => Observation::error(panic_message(&payload), evidence),
    };
    let result = CaseResult::evaluate(case, observation, started.elapsed());
    write_result_from_env(&result).expect("write E2E case result");
    assert_eq!(
        result.test_status,
        TestStatus::Pass,
        "{}: {}",
        result.case.cell_id,
        result.message
    );
    result
}

pub fn recording_evidence(recording_dir: Option<&Path>) -> Evidence {
    let Some(recording_dir) = recording_dir else {
        return Evidence::default();
    };
    let relative_dir = std::env::var_os("CUA_E2E_RECORDINGS_ROOT")
        .map(PathBuf::from)
        .and_then(|root| recording_dir.strip_prefix(root).ok().map(PathBuf::from))
        .unwrap_or_else(|| {
            recording_dir
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_default()
        });
    let artifact_dir = PathBuf::from("recordings").join(relative_dir);
    let path = |name: &str| artifact_dir.join(name).to_string_lossy().replace('\\', "/");
    Evidence {
        video: Some(path("recording.mp4")),
        trajectory: Some(path("trajectory.json")),
        screenshot: None,
        log: None,
    }
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|message| (*message).to_owned())
        })
        .unwrap_or_else(|| "E2E cell panicked without a string payload".to_owned())
}

pub fn write_environment_from_env(record: &EnvironmentRecord) -> io::Result<()> {
    let Some(path) = std::env::var_os("CUA_E2E_ENVIRONMENT_FILE") else {
        return Ok(());
    };
    append_json_line(&PathBuf::from(path), record)
}

pub fn read_json_lines<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>, Vec<String>> {
    let file = File::open(path).map_err(|error| vec![format!("{}: {error}", path.display())])?;
    let mut values = Vec::new();
    let mut errors = Vec::new();
    for (line_index, line) in BufReader::new(file).lines().enumerate() {
        match line {
            Ok(line) if line.trim().is_empty() => {}
            Ok(line) => match serde_json::from_str(&line) {
                Ok(value) => values.push(value),
                Err(error) => {
                    errors.push(format!("{}:{}: {error}", path.display(), line_index + 1))
                }
            },
            Err(error) => errors.push(format!("{}:{}: {error}", path.display(), line_index + 1)),
        }
    }
    if errors.is_empty() {
        Ok(values)
    } else {
        Err(errors)
    }
}

pub fn validate_catalog(
    declarations: &[CaseSpec],
    results: &[CaseResult],
    artifact_root: Option<&Path>,
    require_video: bool,
) -> Result<ValidationSummary, Vec<String>> {
    let mut errors = Vec::new();
    if declarations.is_empty() {
        errors.push("E2E catalog has no declarations".to_owned());
    }
    let mut declared = BTreeMap::new();
    for case in declarations {
        if let Err(error) = case.validate() {
            errors.push(error);
        }
        if declared.insert(case.cell_id.clone(), case).is_some() {
            errors.push(format!("duplicate declaration: {}", case.cell_id));
        }
    }

    let mut observed = BTreeSet::new();
    let mut summary = ValidationSummary::default();
    for result in results {
        let cell_id = &result.case.cell_id;
        if result.schema != RESULT_SCHEMA {
            errors.push(format!(
                "unsupported result schema for {cell_id}: {}",
                result.schema
            ));
        }
        if !observed.insert(cell_id.clone()) {
            errors.push(format!("duplicate result: {cell_id}"));
            continue;
        }
        match declared.get(cell_id) {
            Some(case) if **case == result.case => {}
            Some(_) => errors.push(format!("result contract changed: {cell_id}")),
            None => errors.push(format!("undeclared result: {cell_id}")),
        }

        let rebuilt = CaseResult::evaluate(
            result.case.clone(),
            Observation {
                behavior: result.observed_behavior,
                refusal_code: result.refusal_code,
                passed_oracles: result.passed_oracles.clone(),
                message: String::new(),
                evidence: result.evidence.clone(),
            },
            Duration::from_millis(result.duration_ms.min(u64::MAX as u128) as u64),
        );
        if rebuilt.test_status != result.test_status {
            errors.push(format!("invalid status for {cell_id}"));
        }
        match result.test_status {
            TestStatus::Pass => match result.observed_behavior {
                ObservedBehavior::Delivered => summary.delivered += 1,
                ObservedBehavior::Refused => summary.refused += 1,
                _ => errors.push(format!("passing cell has no valid behavior: {cell_id}")),
            },
            TestStatus::Fail | TestStatus::EnvironmentError => summary.failed += 1,
            TestStatus::Skip => summary.skipped += 1,
        }

        if require_video {
            let Some(video) = result.evidence.video.as_deref() else {
                errors.push(format!("missing video evidence: {cell_id}"));
                continue;
            };
            if let Some(root) = artifact_root {
                let path = root.join(video);
                if std::fs::metadata(&path)
                    .map(|metadata| metadata.len() == 0)
                    .unwrap_or(true)
                {
                    errors.push(format!(
                        "video evidence is missing or empty: {}",
                        path.display()
                    ));
                }
            }
        }
    }

    for cell_id in declared.keys() {
        if !observed.contains(cell_id) {
            errors.push(format!("missing result: {cell_id}"));
        }
    }

    if errors.is_empty() {
        Ok(summary)
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delivered_case(id: &str) -> CaseSpec {
        CaseSpec::delivered(
            id,
            "electron",
            "chromium",
            "click",
            Targeting::Ax,
            Delivery::Background,
            Scope::Window,
            DriverRoute::UiaInvoke,
            vec![OracleKind::FixtureState],
        )
    }

    fn coverage_case(
        id: &str,
        harness: &str,
        action: &str,
        targeting: Targeting,
        delivery: Delivery,
    ) -> CaseSpec {
        CaseSpec::delivered(
            id,
            harness,
            harness,
            action,
            targeting,
            delivery,
            Scope::Window,
            DriverRoute::Composite,
            vec![OracleKind::FixtureState],
        )
    }

    #[test]
    fn required_delivery_rejects_honest_refusal() {
        let case = delivered_case("required-delivery");
        let result = CaseResult::evaluate(
            case,
            Observation::refused(
                RefusalCode::BackgroundUnavailable,
                vec![OracleKind::FixtureState],
                "honest refusal",
                Evidence::default(),
            ),
            Duration::from_millis(1),
        );
        assert_eq!(result.test_status, TestStatus::Fail);
        assert!(result.message.contains("required delivery was refused"));
    }

    #[test]
    fn refusal_requires_explicit_code_and_side_effect_oracles() {
        let case = delivered_case("expected-refusal")
            .expecting_refusal(vec![RefusalCode::BackgroundUnavailable]);
        assert!(case.validate().is_err());

        let mut case = case;
        case.oracles = vec![
            OracleKind::FixtureState,
            OracleKind::Focus,
            OracleKind::ZOrder,
            OracleKind::NoLeakedInput,
        ];
        let result = CaseResult::evaluate(
            case,
            Observation::refused(
                RefusalCode::BackgroundUnavailable,
                vec![
                    OracleKind::FixtureState,
                    OracleKind::Focus,
                    OracleKind::ZOrder,
                    OracleKind::NoLeakedInput,
                ],
                "",
                Evidence::default(),
            ),
            Duration::from_millis(1),
        );
        assert_eq!(result.test_status, TestStatus::Pass);
        assert_eq!(result.observed_behavior, ObservedBehavior::Refused);
    }

    #[test]
    fn unknown_background_error_is_not_a_refusal_code() {
        assert_eq!(
            RefusalCode::from_driver_code("background_unavailable"),
            Some(RefusalCode::BackgroundUnavailable)
        );
        assert_eq!(RefusalCode::from_driver_code("background_timeout"), None);
    }

    #[test]
    fn validator_rejects_missing_and_duplicate_results() {
        let case = delivered_case("one");
        let result = CaseResult::evaluate(
            case.clone(),
            Observation::delivered(vec![OracleKind::FixtureState], Evidence::default()),
            Duration::from_millis(1),
        );
        let errors = validate_catalog(
            std::slice::from_ref(&case),
            &[result.clone(), result],
            None,
            false,
        )
        .expect_err("duplicate result should fail");
        assert!(errors
            .iter()
            .any(|error| error.contains("duplicate result")));

        let errors =
            validate_catalog(&[case], &[], None, false).expect_err("missing result should fail");
        assert!(errors.iter().any(|error| error.contains("missing result")));
    }

    #[test]
    fn validator_rejects_empty_catalog() {
        let errors = validate_catalog(&[], &[], None, false)
            .expect_err("an empty E2E catalog must not pass");
        assert!(errors
            .iter()
            .any(|error| error.contains("no declarations")));
    }

    #[test]
    fn validator_rejects_contradictory_status_and_missing_video() {
        let case = delivered_case("contradiction");
        let mut result = CaseResult::evaluate(
            case.clone(),
            Observation::error("driver failed", Evidence::default()),
            Duration::from_millis(1),
        );
        result.test_status = TestStatus::Pass;
        let errors = validate_catalog(std::slice::from_ref(&case), &[result], None, false)
            .expect_err("contradictory status should fail");
        assert!(errors.iter().any(|error| error.contains("invalid status")));

        let result = CaseResult::evaluate(
            case.clone(),
            Observation::delivered(vec![OracleKind::FixtureState], Evidence::default()),
            Duration::from_millis(1),
        );
        let errors = validate_catalog(&[case], &[result], None, true)
            .expect_err("required video should fail");
        assert!(errors
            .iter()
            .any(|error| error.contains("missing video evidence")));
    }

    #[test]
    fn validator_rejects_unknown_result_schema() {
        let case = delivered_case("unknown-schema");
        let mut result = CaseResult::evaluate(
            case.clone(),
            Observation::delivered(vec![OracleKind::FixtureState], Evidence::default()),
            Duration::from_millis(1),
        );
        result.schema = "cua-e2e-result/v999".to_owned();
        let errors = validate_catalog(&[case], &[result], None, false)
            .expect_err("unknown result schema should fail");
        assert!(errors
            .iter()
            .any(|error| error.contains("unsupported result schema")));
    }

    #[test]
    fn valid_catalog_renders_delivered_rollup_and_flat_schema() {
        let case = delivered_case("rendered");
        let result = CaseResult::evaluate(
            case.clone(),
            Observation::delivered(vec![OracleKind::FixtureState], Evidence::default()),
            Duration::from_millis(7),
        );
        let summary = validate_catalog(
            std::slice::from_ref(&case),
            std::slice::from_ref(&result),
            None,
            false,
        )
        .expect("valid catalog");
        assert_eq!(summary.delivered, 1);
        assert!(summary
            .markdown_with_declarations(std::slice::from_ref(&case), std::slice::from_ref(&result),)
            .contains("1 delivered"));

        let value = serde_json::to_value(result).expect("serialize result");
        assert_eq!(value["schema"], RESULT_SCHEMA);
        assert_eq!(value["cell_id"], "rendered");
        assert!(value.get("case").is_none());
    }

    #[test]
    fn declared_coverage_distinguishes_statuses_and_groups_harness_actions() {
        let delivered = coverage_case(
            "electron-click-ax-background",
            "electron",
            "click",
            Targeting::Ax,
            Delivery::Background,
        );
        let mut refused = coverage_case(
            "electron-click-px-background",
            "electron",
            "click",
            Targeting::Px,
            Delivery::Background,
        )
        .expecting_refusal(vec![RefusalCode::BackgroundUnavailable]);
        refused.oracles = vec![
            OracleKind::FixtureState,
            OracleKind::Focus,
            OracleKind::ZOrder,
            OracleKind::NoLeakedInput,
        ];
        let failed = coverage_case(
            "electron-click-px-foreground",
            "electron",
            "click",
            Targeting::Px,
            Delivery::Foreground,
        );
        let grouped = coverage_case(
            "tauri-type-text-ax-background",
            "tauri",
            "type_text",
            Targeting::Ax,
            Delivery::Background,
        );

        let results = vec![
            CaseResult::evaluate(
                delivered.clone(),
                Observation::delivered(vec![OracleKind::FixtureState], Evidence::default()),
                Duration::from_millis(1),
            ),
            CaseResult::evaluate(
                refused.clone(),
                Observation::refused(
                    RefusalCode::BackgroundUnavailable,
                    vec![
                        OracleKind::FixtureState,
                        OracleKind::Focus,
                        OracleKind::ZOrder,
                        OracleKind::NoLeakedInput,
                    ],
                    "",
                    Evidence::default(),
                ),
                Duration::from_millis(1),
            ),
            CaseResult::evaluate(
                failed.clone(),
                Observation::error("driver error", Evidence::default()),
                Duration::from_millis(1),
            ),
            CaseResult::evaluate(
                grouped.clone(),
                Observation::delivered(vec![OracleKind::FixtureState], Evidence::default()),
                Duration::from_millis(1),
            ),
        ];
        let declarations = vec![delivered, refused, failed, grouped];
        let summary =
            validate_catalog(&declarations, &results, None, false).expect("valid catalog");
        let markdown = summary.markdown_with_declarations(&declarations, &results);

        assert!(markdown.contains("| electron | click | PASS | - | REFUSED | FAIL | - | - |"));
        assert!(markdown.contains("| tauri | type_text | PASS | - | - | - | - | - |"));
    }

    #[test]
    fn summary_records_the_exact_source_sha() {
        let case = delivered_case("source-sha");
        let result = CaseResult::evaluate(
            case.clone(),
            Observation::delivered(vec![OracleKind::FixtureState], Evidence::default()),
            Duration::from_millis(1),
        );
        let summary = validate_catalog(
            std::slice::from_ref(&case),
            std::slice::from_ref(&result),
            None,
            false,
        )
        .expect("valid catalog");
        let sha = "0123456789abcdef0123456789abcdef01234567";
        let markdown = summary.markdown_with_declarations_and_source(
            std::slice::from_ref(&case),
            std::slice::from_ref(&result),
            Some(sha),
        );

        assert!(markdown.contains(&format!("**Source SHA:** `{sha}`")));
    }

    #[test]
    fn declared_coverage_keeps_page_and_not_applicable_out_of_the_ax_px_grid() {
        let page = coverage_case(
            "web-evaluate-page-background",
            "web",
            "evaluate",
            Targeting::Page,
            Delivery::Background,
        );
        let not_applicable = coverage_case(
            "web-evaluate-not-applicable-background",
            "web",
            "evaluate",
            Targeting::NotApplicable,
            Delivery::Background,
        );
        let results = [page.clone(), not_applicable.clone()]
            .into_iter()
            .map(|case| {
                CaseResult::evaluate(
                    case,
                    Observation::delivered(vec![OracleKind::FixtureState], Evidence::default()),
                    Duration::from_millis(1),
                )
            })
            .collect::<Vec<_>>();
        let declarations = vec![page, not_applicable];
        let summary =
            validate_catalog(&declarations, &results, None, false).expect("valid catalog");
        let markdown = summary.markdown_with_declarations(&declarations, &results);

        assert!(markdown.contains(
            "| web | evaluate | - | - | - | - | Background: PASS | NotApplicable/Background: PASS |"
        ));
    }

    #[test]
    fn native_case_builders_encode_delivery_and_oracle_contracts() {
        let background =
            native_background_case("wpf", "left_click", Targeting::Ax, DriverRoute::UiaInvoke);
        assert_eq!(background.delivery, Delivery::Background);
        assert_eq!(
            background.cell_id,
            format!("{}-wpf-left-click-ax-background", std::env::consts::OS)
        );
        for oracle in [
            OracleKind::FixtureState,
            OracleKind::Focus,
            OracleKind::ZOrder,
            OracleKind::Cursor,
            OracleKind::NoLeakedInput,
        ] {
            assert!(background.oracles.contains(&oracle));
        }
        background.validate().expect("background case is valid");

        let foreground = native_foreground_case(
            "wpf",
            "right_click",
            Targeting::Ax,
            DriverRoute::WindowsSendInput,
        );
        assert_eq!(foreground.delivery, Delivery::Foreground);
        assert_eq!(foreground.oracles, vec![OracleKind::FixtureState]);

        let readonly = native_readonly_case(
            "wpf",
            "ax_tree",
            Targeting::Ax,
            DriverRoute::AxRead,
            vec![OracleKind::AxState],
        );
        assert_eq!(readonly.delivery, Delivery::NotApplicable);
        assert_eq!(readonly.oracles, vec![OracleKind::AxState]);
        readonly.validate().expect("read-only case is valid");
    }

    #[test]
    fn delivered_with_fixture_state_deduplicates_the_oracle() {
        let observation = Observation::delivered_with_fixture_state(vec![
            OracleKind::Focus,
            OracleKind::FixtureState,
        ]);
        assert_eq!(
            observation.passed_oracles,
            vec![OracleKind::FixtureState, OracleKind::Focus]
        );
    }

    #[test]
    fn shared_routes_are_explicit_for_the_current_matrix() {
        let mut cells = Vec::new();
        for action in [
            "left_click",
            "right_click",
            "double_click",
            "type_text",
            "press_key",
            "hotkey",
            "scroll",
            "child_window",
        ] {
            for targeting in [Targeting::Ax, Targeting::Px] {
                for delivery in [Delivery::Background, Delivery::Foreground] {
                    cells.push((action, targeting, delivery));
                }
            }
        }
        for delivery in [Delivery::Background, Delivery::Foreground] {
            cells.push(("drag", Targeting::Px, delivery));
            cells.push(("editor_save", Targeting::Ax, delivery));
        }
        assert_eq!(cells.len(), 36);
        for (platform, display_server) in [
            (Platform::Windows, DisplayServer::Win32),
            (Platform::Macos, DisplayServer::Quartz),
            (Platform::Linux, DisplayServer::X11),
            (Platform::Linux, DisplayServer::Wayland),
        ] {
            for (action, targeting, delivery) in cells.iter().copied() {
                shared_web_route(platform, display_server, action, targeting, delivery)
                    .unwrap_or_else(|error| panic!("{error}"));
            }
        }
    }

    #[test]
    fn windows_pixel_background_route_remains_targeted_injection() {
        assert_eq!(
            shared_web_route(
                Platform::Windows,
                DisplayServer::Win32,
                "left_click",
                Targeting::Px,
                Delivery::Background,
            ),
            Ok(DriverRoute::WindowsTargetedInjection)
        );
    }
}
