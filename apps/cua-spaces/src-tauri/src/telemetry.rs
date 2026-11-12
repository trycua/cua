// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Usage telemetry for the Spaces app, on the SDK's telemetry
//! core (cua-telemetry): the same schema, install id, switches and
//! `$CUA_HOME/config.toml` setting as the `cua` CLI. The app shows the
//! first-run notice in its onboarding (nothing is sent before), and
//! Settings > Privacy turns it off.
//!
//! Only fixed feature names, funnel steps, the app core's telemetry
//! signals (`cua_spaces_app_core::telemetry`, the same ones the SwiftUI app
//! sends) and teleport outcomes are recorded here; never Space names, app
//! names outside the public catalog, paths, URLs or anything on screen.

use std::path::Path;
use std::sync::OnceLock;
use std::time::Instant;

use cua_spaces_app_core::telemetry::{self as app_telemetry, TelemetrySignal};
use cua_telemetry::events::{self, CallerKind, CallerKindSource, Outcome, TeleportOutcome};
use cua_telemetry::{NoticeMode, Telemetry};
use serde::Serialize;

static CLIENT: OnceLock<Telemetry> = OnceLock::new();

/// Creates the app's client for `home` (`$CUA_HOME`). Call once at start.
/// The SDK's own events from this process (creates, teleports, presence)
/// are the app's too, and wait for the same notice.
pub fn init(home: &Path) -> &'static Telemetry {
    CLIENT.get_or_init(|| {
        let sdk = cua_telemetry::init("spaces_app", env!("CARGO_PKG_VERSION"));
        sdk.set_notice_mode(NoticeMode::External);
        let t = Telemetry::builder()
            .home(home)
            .product("spaces_app", env!("CARGO_PKG_VERSION"))
            .notice_mode(NoticeMode::External)
            .build();
        // `app_launched` now, or once the first run shows the notice.
        app_telemetry::start(&t);
        t
    })
}

fn t() -> Option<&'static Telemetry> {
    CLIENT.get()
}

/// Asks the Spaces API for the signed-in account's opaque id and hands it
/// to telemetry (CUA-1881). The state is the install's, in
/// `$CUA_HOME/telemetry`, so the app's and the SDK's clients both see it.
pub async fn sync_account(session: &crate::auth::SessionHandle) {
    let Some(t) = t() else { return };
    if !t.is_enabled() {
        return;
    }
    let budget = std::time::Duration::from_secs(10);
    let Ok(Ok(token)) = tokio::time::timeout(budget, session.get_valid_token(false)).await else {
        return;
    };
    let Ok(api) = cua_host::spaces_api::SpacesApiClient::current() else {
        return;
    };
    if let Ok(Ok(me)) = tokio::time::timeout(budget, api.me(&token)).await {
        t.offer_account(me.analytics_id.as_deref());
    }
}

/// Records signals the app core derived (both shells send them the same
/// way) and marks the day active.
pub fn record(signals: &[TelemetrySignal]) {
    if let Some(t) = t() {
        app_telemetry::record(t, signals);
    }
}

/// Records a Spaces app feature (a fixed name; others are dropped).
pub fn feature(name: &str) {
    record(&app_telemetry::feature_used(name));
}

/// Records a funnel step.
pub fn step(name: &str, ok: bool) {
    record(&[TelemetrySignal::Step {
        step: name.into(),
        ok,
    }]);
}

/// Records a teleport the app ran itself. No broker verified this path, so
/// the caller kind is self-reported (`unknown` for the app).
pub fn teleport<T>(
    app_id: &str,
    capability: &str,
    move_kind: &str,
    started: Instant,
    r: &Result<T, String>,
    items: u64,
) {
    let Some(t) = t() else { return };
    let info = events::TeleportInfo::new(
        app_id,
        capability,
        move_kind,
        CallerKind::self_reported(t.product()),
        CallerKindSource::SelfReported,
        "sdk",
    );
    t.capture(events::teleport_attempted(&info));
    let o = match r {
        Ok(_) => TeleportOutcome::Ok,
        Err(m) if m.contains("https://cua.ai/download") || m.contains("cua://keyvault") => {
            TeleportOutcome::RequiresCuaApp
        }
        Err(m) if m.to_ascii_lowercase().contains("declined") || m.contains("not approved") => {
            TeleportOutcome::ConsentDenied
        }
        Err(_) => TeleportOutcome::Error,
    };
    t.capture(events::teleport_completed(
        &info,
        o,
        started.elapsed(),
        items,
    ));
    if r.is_ok() {
        t.capture_step("first_teleport", Outcome::Ok);
    }
    t.capture_active_day();
}

/// Settings > Privacy.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelemetryView {
    pub enabled: bool,
    /// Why (`env DO_NOT_TRACK`, `config <path>`, `default`, ...).
    pub source: String,
    /// `do_not_track`, `env`, `legacy_env`, `config`, `ci` or `default`.
    pub source_kind: String,
    pub notice_shown: bool,
    pub notice_text: String,
    pub docs_url: String,
    /// Show the one-time linking notice now (CUA-1881).
    pub link_notice_due: bool,
}

fn client() -> Result<&'static Telemetry, String> {
    t().ok_or_else(|| "telemetry is not initialised".to_string())
}

fn view(t: &Telemetry) -> TelemetryView {
    t.refresh();
    let s = t.status();
    TelemetryView {
        enabled: s.enabled,
        source: s.source,
        source_kind: s.source_kind,
        notice_shown: s.notice_shown,
        notice_text: cua_telemetry::notice::TEXT.to_string(),
        docs_url: cua_telemetry::notice::DOCS_URL.to_string(),
        link_notice_due: t.link_notice_due(),
    }
}

/// The one-time linking notice (CUA-1881) as the web UI shows it: the words
/// from `cua_telemetry::link`, one copy for every shell, and whether it is
/// due now.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkNoticeView {
    pub due: bool,
    pub title: String,
    pub body: String,
    /// "Got it": keeps usage data on.
    pub ok_label: String,
    /// "Turn off usage data".
    pub turn_off_label: String,
    /// The link to what is collected.
    pub details_label: String,
    pub docs_url: String,
}

fn link_notice_view(due: bool) -> LinkNoticeView {
    use cua_telemetry::link;
    LinkNoticeView {
        due,
        title: link::NOTICE_TITLE.into(),
        body: link::NOTICE_BODY.into(),
        ok_label: link::NOTICE_OK.into(),
        turn_off_label: link::NOTICE_TURN_OFF.into(),
        details_label: link::NOTICE_DETAILS.into(),
        docs_url: cua_telemetry::notice::DOCS_URL.into(),
    }
}

/// The one-time linking notice: its words and whether it is due now.
#[tauri::command]
pub fn telemetry_link_notice() -> Result<LinkNoticeView, String> {
    let t = client()?;
    t.refresh();
    Ok(link_notice_view(t.link_notice_due()))
}

/// The one-time linking notice was closed (either button).
#[tauri::command]
pub fn telemetry_acknowledge_link_notice() -> Result<TelemetryView, String> {
    let t = client()?;
    t.acknowledge_link_notice();
    Ok(view(t))
}

#[tauri::command]
pub fn telemetry_status() -> Result<TelemetryView, String> {
    Ok(view(client()?))
}

/// The Settings switch: writes `[telemetry] enabled` to
/// `$CUA_HOME/config.toml` (the same setting as `cua telemetry off`).
#[tauri::command]
pub fn telemetry_set_enabled(enabled: bool) -> Result<TelemetryView, String> {
    let t = client()?;
    t.set_enabled(enabled).map_err(|e| e.to_string())?;
    // The SDK's client in this process reads the same setting.
    cua_telemetry::global().refresh();
    Ok(view(t))
}

/// The onboarding showed the notice.
#[tauri::command]
pub fn telemetry_acknowledge_notice() -> Result<TelemetryView, String> {
    let t = client()?;
    app_telemetry::acknowledge_notice(t);
    Ok(view(t))
}

/// The first run left Welcome with its usage-data switch at `on`: writes the
/// machine's setting when it changed, then records that the notice was
/// shown. Nothing was sent before; with it off, nothing is after.
#[tauri::command]
pub fn telemetry_welcome_left(on: bool) -> Result<TelemetryView, String> {
    let t = client()?;
    app_telemetry::welcome_left(t, on).map_err(|e| e.to_string())?;
    // The SDK's client in this process reads the same setting.
    cua_telemetry::global().refresh();
    Ok(view(t))
}

/// Signals the webview's core calls derived (`telemetry.*`: the first run,
/// Space creates, Storage, Share, enrollment, the New Space panel, the
/// updater). Fixed words only; anything outside the schema is dropped.
#[tauri::command]
pub fn telemetry_record_signals(signals: Vec<TelemetrySignal>) {
    record(&signals);
}

/// The app's interface language (`ui_locale` on the day's `cua_app_active`):
/// one of a fixed set, anything else is `other`. Nothing when telemetry is off.
#[tauri::command]
pub fn telemetry_set_ui_locale(locale: String) {
    if let Some(t) = t() {
        t.set_ui_locale(&locale);
    }
}

/// A feature used in the webview (fixed names only).
#[tauri::command]
pub fn telemetry_record_feature(feature_name: String) {
    feature(&feature_name);
}

/// A funnel step reached in the webview (fixed names only).
#[tauri::command]
pub fn telemetry_record_step(step_name: String, ok: bool) {
    step(&step_name, ok);
}

/// A viewer's media session ended (the webview's `MediaSession` counters).
/// Sent as coarse buckets only (`cua_stream_stats`, sampled); nothing when
/// telemetry is off or the stream carried no video.
#[tauri::command]
pub fn telemetry_record_stream(codec: String, frames: u64, height: u32, duration_ms: u64) {
    if let (Some(t), Some(e)) = (t(), stream_event(&codec, frames, height, duration_ms)) {
        t.capture(e);
    }
}

/// The webview decodes with WebCodecs over the media WebSocket; whether that
/// is hardware accelerated is not observable there.
fn stream_event(codec: &str, frames: u64, height: u32, duration_ms: u64) -> Option<events::Event> {
    events::stream_summary(
        "websocket",
        codec,
        None,
        frames,
        height,
        None,
        std::time::Duration::from_millis(duration_ms),
    )
}

/// Before the app exits: flush briefly, spool the rest.
pub fn shutdown() {
    if let Some(t) = t() {
        t.shutdown(std::time::Duration::from_millis(400));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_link_notice_carries_the_sdk_words_in_the_pages_shape() {
        let v = serde_json::to_value(link_notice_view(true)).unwrap();
        assert_eq!(v["due"], true);
        assert_eq!(v["title"], cua_telemetry::link::NOTICE_TITLE);
        assert_eq!(v["body"], cua_telemetry::link::NOTICE_BODY);
        assert_eq!(v["okLabel"], "Got it");
        assert_eq!(v["turnOffLabel"], "Turn off usage data");
        assert_eq!(v["detailsLabel"], "What is collected");
        assert_eq!(
            v["docsUrl"],
            "https://cua.ai/docs/cua-sdk/concepts/telemetry"
        );
        assert_eq!(
            serde_json::to_value(link_notice_view(false)).unwrap()["due"],
            false
        );
    }
}
