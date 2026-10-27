//! Which typed call reaches each advertised tool.
//!
//! The Swift package's README carries this as prose; `contract/manifest.json`
//! carries it as `sdk_symbol`. This is the Rust core's own claim, and
//! [`tests::every_advertised_tool_has_a_typed_path`] makes it checkable rather
//! than aspirational: the table and the contract must name exactly the same
//! tools, and no tool may be left without a symbol.
//!
//! The symbols are the **Rust** ones. A binding's own names are generated from
//! them by UniFFI's per-language renaming, so a binding asserts against this
//! table rather than transcribing it.

/// One row of the coverage table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCoverage {
    pub tool: String,
    /// The typed Rust path that reaches it.
    pub sdk_symbol: String,
    /// `local`, `direct`, ... — from the contract.
    pub providers: Vec<String>,
    /// `free`, `metered` or `cloud`, from the contract.
    pub metering: String,
}

/// tool name → the typed Rust path that reaches it.
const SYMBOLS: [(&str, &str); 114] = [
    ("add_space", "Connection::add_space"),
    ("remove_space", "Connection::remove_space"),
    ("list_spaces", "Connection::spaces"),
    ("create_space", "Connection::create_space"),
    ("delete_space", "Space::delete"),
    ("stop_space", "Space::stop"),
    ("start_space", "Space::start"),
    ("resize_space", "Space::resize"),
    ("space_bash", "Space::bash"),
    ("space_write", "Space::write_text"),
    ("space_service", "Space::keep_service"),
    ("upload", "Space::upload"),
    ("send_file", "Space::send_file"),
    ("download", "Space::download"),
    ("stream_endpoint", "Space::stream_endpoint"),
    ("list_space_windows", "Space::windows"),
    (
        "stream_space_window",
        "Space::stream_window_to_operator_desktop",
    ),
    (
        "show_space_pip",
        "Space::pin_picture_in_picture_on_operator_desktop",
    ),
    (
        "hide_space_pip",
        "Space::unpin_picture_in_picture_from_operator_desktop",
    ),
    (
        "open_space_viewer",
        "Space::open_viewer_on_operator_desktop",
    ),
    ("list_tools", "Space::service_catalog"),
    ("call_tool", "Space::call_service_tool"),
    ("agent_start", "Space::start_agent"),
    ("agent_message", "Space::send_message"),
    ("agent_status", "Space::run_status"),
    ("agent_events", "Space::events_after"),
    ("agent_interrupt", "Space::interrupt_run"),
    ("agent_stop", "Space::stop_run"),
    ("agent_list", "Space::runs"),
    ("agent_capabilities", "Space::harness_capabilities"),
    ("agent_add", "Connection::create_persistent_agent"),
    ("persistent_agent_list", "Connection::persistent_agents"),
    ("agent_remove", "Connection::remove_persistent_agent"),
    ("agent_send", "Connection::send_to_persistent_agent"),
    ("persistent_agent_save", "Connection::save_agent_home"),
    ("agent_pause", "Connection::pause_agent"),
    ("agent_resume", "Connection::resume_agent"),
    ("agent_schedule_add", "Connection::add_routine"),
    ("agent_schedule_list", "Connection::routines"),
    ("agent_schedule_remove", "Connection::remove_routine"),
    (
        "agent_schedule_set_enabled",
        "Connection::set_routine_enabled",
    ),
    ("notify_user", "Connection::notify_user"),
    ("notifications_list", "Connection::notifications"),
    ("notifications_ack", "Connection::mark_notifications_read"),
    ("computer_access_grant", "Connection::allow_computer"),
    ("computer_access_revoke", "Connection::revoke_computer"),
    ("computer_access_list", "Connection::computer_access"),
    ("thread_read", "Connection::thread_read"),
    ("thread_post", "Connection::thread_post"),
    ("thread_inbox", "Connection::thread_inbox"),
    ("thread_claim", "Connection::thread_claim"),
    ("thread_reply", "Connection::thread_reply"),
    ("thread_import", "Connection::thread_import"),
    ("agent_members", "Connection::agent_members"),
    ("agent_set_rights", "Connection::agent_set_rights"),
    ("thread_join", "Connection::thread_join"),
    (
        "thread_join_code_create",
        "Connection::thread_join_code_create",
    ),
    ("thread_join_code_list", "Connection::thread_join_code_list"),
    (
        "thread_join_code_cancel",
        "Connection::thread_join_code_cancel",
    ),
    ("thread_join_redeem", "Connection::thread_join_redeem"),
    ("thread_settings", "Connection::thread_settings"),
    ("thread_device_revoke", "Connection::thread_device_revoke"),
    ("teleport_manifest", "Space::teleport_manifest"),
    ("teleport_app", "Space::teleport_send"),
    ("request_site_login", "Space::request_site_login"),
    ("hotspot_start", "Connection::hotspot_start"),
    ("hotspot_stop", "Connection::hotspot_stop"),
    ("hotspot_status", "Connection::hotspot_status"),
    ("volume_ls", "Connection::volume_ls"),
    ("volume_read", "Connection::volume_read"),
    ("volume_write", "Connection::volume_write"),
    ("volume_delete", "Connection::volume_delete"),
    ("volume_history", "Connection::volume_history"),
    ("volume_restore", "Connection::volume_restore"),
    ("volume_grant", "Connection::volume_grant"),
    ("volume_revoke", "Connection::volume_revoke"),
    ("volume_grants", "Connection::volume_grants"),
    ("volume_request_access", "Connection::volume_request_access"),
    ("volume_requests", "Connection::volume_requests"),
    ("volume_approve", "Connection::volume_approve"),
    ("volume_deny", "Connection::volume_deny"),
    ("volume_audit", "Connection::volume_audit"),
    ("volume_storage", "Connection::volume_storage"),
    ("volume_storage_set", "Connection::volume_storage_set"),
    ("volume_mount_status", "Connection::volume_mount_status"),
    ("volume_mount", "Connection::volume_mount"),
    ("volume_unmount", "Connection::volume_unmount"),
    ("volume_sync_status", "Connection::volume_sync_status"),
    ("volume_sync_events", "Connection::volume_sync_events"),
    ("volume_sync_resolve", "Connection::volume_sync_resolve"),
    ("volume_cache_stats", "Connection::volume_cache_stats"),
    ("volume_cache_set", "Connection::volume_cache_set"),
    ("volume_cache_clear", "Connection::volume_cache_clear"),
    ("share_space", "Space::share_with"),
    ("unshare_space", "Space::unshare"),
    ("space_shares", "Space::shares"),
    ("relay_register_space", "Space::relay_register"),
    ("relay_unregister_space", "Space::relay_unregister"),
    ("cloud_status", "Connection::cloud_status"),
    ("cloud_connect", "Connection::cloud_connect"),
    ("cloud_test", "Connection::cloud_test"),
    ("cloud_disconnect", "Connection::cloud_disconnect"),
    ("cloud_sweep", "Connection::cloud_sweep"),
    ("cloud_plan", "Connection::cloud_plan"),
    ("cloud_hosts", "Connection::cloud_hosts"),
    ("cloud_add_host", "Connection::cloud_add_host"),
    ("cloud_remove_host", "Connection::cloud_remove_host"),
    ("cloud_budget", "Connection::cloud_budget"),
    ("cloud_reconcile", "Connection::cloud_reconcile"),
    ("prewarm", "Connection::prewarm"),
    ("prewarm_list", "Connection::prewarm_list"),
    ("prewarm_remove", "Connection::prewarm_remove"),
    ("cloud_audit", "Connection::cloud_audit"),
    ("cloud_cost", "Connection::cloud_cost"),
];

/// The coverage table, in contract order, with the provider and metering
/// facts read from the contract rather than restated here.
///
/// A person-only tool (`cloud_approve`) is left out: no MCP server serves it
/// to this client; the apps call it through the cua SDK.
pub fn coverage() -> Vec<ToolCoverage> {
    cua_spaces_contract::tools()
        .into_iter()
        .filter(|tool| !cua_spaces_contract::person_only(tool.name))
        .map(|tool| {
            let symbol = SYMBOLS
                .iter()
                .find(|(name, _)| *name == tool.name)
                .map(|(_, symbol)| (*symbol).to_string())
                .unwrap_or_default();
            ToolCoverage {
                tool: tool.name.to_string(),
                sdk_symbol: symbol,
                providers: tool
                    .providers
                    .iter()
                    .map(|provider| {
                        serde_json::to_value(provider)
                            .ok()
                            .and_then(|value| value.as_str().map(str::to_string))
                            .unwrap_or_default()
                    })
                    .collect(),
                metering: serde_json::to_value(tool.metering)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_string))
                    .unwrap_or_default(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape of the Swift package's `testEveryAdvertisedToolHasATypedPath`.
    /// Every tool the contract advertises is reachable through a typed call,
    /// and the table names no tool the contract does not.
    #[test]
    fn every_advertised_tool_has_a_typed_path() {
        let rows = coverage();
        assert_eq!(rows.len(), SYMBOLS.len(), "one row per advertised tool");

        let missing: Vec<&str> = rows
            .iter()
            .filter(|row| row.sdk_symbol.is_empty())
            .map(|row| row.tool.as_str())
            .collect();
        assert!(missing.is_empty(), "tools with no typed path: {missing:?}");

        let advertised: Vec<&str> = cua_spaces_contract::tools()
            .into_iter()
            .map(|tool| tool.name)
            .filter(|name| !cua_spaces_contract::person_only(name))
            .collect();
        let invented: Vec<&str> = SYMBOLS
            .iter()
            .map(|(name, _)| *name)
            .filter(|name| !advertised.contains(name))
            .collect();
        assert!(
            invented.is_empty(),
            "the table names tools the contract does not advertise: {invented:?}"
        );
    }

    #[test]
    fn no_two_tools_share_a_symbol() {
        let mut symbols: Vec<&str> = SYMBOLS.iter().map(|(_, symbol)| *symbol).collect();
        symbols.sort_unstable();
        let before = symbols.len();
        symbols.dedup();
        assert_eq!(before, symbols.len(), "two tools share one typed path");
    }

    /// The provider and metering facts come from the contract, so the table
    /// cannot disagree with it. `teleport_app` has a Local path; only
    /// `create_space` and `agent_resume` can be metered (when they create in
    /// the cloud).
    #[test]
    fn the_table_carries_the_contracts_provider_and_metering_facts() {
        let rows = coverage();
        let row = |name: &str| rows.iter().find(|row| row.tool == name).unwrap().clone();
        assert!(row("teleport_app").providers.contains(&"local".to_string()));
        let metered: Vec<String> = rows
            .iter()
            .filter(|row| row.metering != "free")
            .map(|row| row.tool.clone())
            .collect();
        assert_eq!(metered, vec!["create_space", "agent_resume"]);
    }
}
