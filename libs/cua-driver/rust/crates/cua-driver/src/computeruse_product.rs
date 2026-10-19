//! ComputerUse product build policy (feature "computeruse").
//!
//! The driver is embedded by ComputerUseHost and launched as "cua-driver mcp". The host pins the binary
//! and owns its lifecycle, so CLI verbs that reach the network, install or replace files, or register
//! persistent OS integrations are disabled. Observation, input, recording/history and diagnostics stay.

/// Top-level verbs (and verb/subcommand pairs) refused by the product build.
pub fn cli_verb_disabled(verb: &str, subcommand: Option<&str>) -> bool {
    match verb {
        // GitHub release check / self-update / release channel switching.
        "update" | "check-update" | "channel" => true,
        // Network installers and persistent OS integration.
        "skills" | "extension" | "perception" | "autostart" | "mcp-config" => true,
        // Telemetry is compiled off; only the read-only status stays.
        "telemetry" => !matches!(subcommand, None | Some("status")),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::cli_verb_disabled;

    #[test]
    fn network_and_install_verbs_are_disabled_runtime_verbs_stay() {
        for verb in ["update", "check-update", "channel", "skills", "extension", "perception", "autostart", "mcp-config"] {
            assert!(cli_verb_disabled(verb, None), "{verb}");
        }
        assert!(cli_verb_disabled("telemetry", Some("enable")));
        assert!(!cli_verb_disabled("telemetry", Some("status")));
        for verb in ["mcp", "serve", "call", "list-tools", "describe", "status", "stop", "doctor", "diagnose", "recording", "manifest"] {
            assert!(!cli_verb_disabled(verb, None), "{verb}");
        }
    }
}
