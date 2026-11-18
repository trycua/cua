// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! "Reset & Show" (Settings, About): what the welcome remembers, and so what
//! going back to its first step clears.
//!
//! One list for every app. The welcome's own state lives in two kinds of
//! place, and the app that runs the shared web UI clears exactly these and
//! nothing else:
//!
//! * [`WEB_KEYS`], the web UI's browser storage: where the welcome stopped,
//!   the first run's answers (the agents picked, the first Space's request,
//!   "Skip for now"), the plan card's "Not now", the teammates flag, and the
//!   coding apps choice (`cua.connectedApps`: which apps Cua was set up in
//!   or turned off, and whether to set up newly installed ones).
//! * [`HOST_FILES`], the app's `onboarding.json` beside its settings:
//!   whether the first run finished, and as what (this computer as a host
//!   or a client). The SwiftUI app, the Electron app and the Tauri app keep
//!   the same file.
//!
//! What it never touches: Spaces, computers, Cua Volumes, the Keyvault and
//! the keychain, the sign-in, connected clouds, experiments, the update
//! channel, the usage-data choice and every other setting, the getting
//! started checklist, tips, and the agents' own config files (the coding
//! apps stay connected; only Cua's record of the choice is cleared).
//! [`NEVER`] names them, for the tests and the docs.
//!
//! The lists are the contract. `contracts/onboarding-reset.json` is the
//! golden of [`plan`]; the web UI's, the Electron app's and the SwiftUI
//! app's tests read it and check their own keys and file names against it.

use serde::Serialize;

use crate::first_run::{self, FirstRunStep};

/// The web UI's browser-storage keys the welcome keeps.
pub const WEB_KEYS: [&str; 5] = [
    // Where the welcome stopped, and "Skip for now" (`bridge/onboarding.ts`).
    "cua-spaces:onboarding",
    // The first run's answers (`bridge/first-run.ts`).
    "cua-spaces:first-run",
    // "Not now" on the plan card (`UPSELL_DISMISSED_KEY`).
    "cua.upsell.dismissed.onboarding",
    // The teammates flag (`TEAMMATES_KEY`).
    "cua.firstRun.teammates",
    // The coding apps choice (`lib/connected-apps.ts`).
    "cua.connectedApps",
];

/// The files beside the app's settings the welcome keeps.
pub const HOST_FILES: [&str; 1] = ["onboarding.json"];

/// What a reset never touches, in words (docs and tests).
pub const NEVER: [&str; 12] = [
    "Spaces",
    "computers",
    "Cua Volumes",
    "Keyvault and keychain items",
    "sign-in and session",
    "connected clouds",
    "experiments",
    "update channel",
    "usage-data choice",
    "other settings",
    "getting started checklist and tips",
    "the coding apps' own config files",
];

/// What a reset clears, and where the welcome starts afterwards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingResetPlan {
    /// Browser-storage keys to remove.
    pub web_keys: Vec<String>,
    /// Files beside the app's settings to remove.
    pub host_files: Vec<String>,
    /// The welcome starts here (the first run's first step).
    pub first_step: FirstRunStep,
}

/// The reset, as data. Removing what it names twice is the same as once.
pub fn plan() -> OnboardingResetPlan {
    OnboardingResetPlan {
        web_keys: WEB_KEYS.iter().map(|k| k.to_string()).collect(),
        host_files: HOST_FILES.iter().map(|f| f.to_string()).collect(),
        first_step: first_run::steps(&first_run::FirstRunInput::default())
            .first()
            .copied()
            .unwrap_or(FirstRunStep::Welcome),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_starts_at_the_first_step() {
        assert_eq!(plan().first_step, FirstRunStep::Welcome);
    }

    #[test]
    fn it_names_each_key_once_and_only_welcome_state() {
        let p = plan();
        let mut keys = p.web_keys.clone();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), p.web_keys.len());
        // Nothing of the rest of the app is in it.
        for k in &p.web_keys {
            for other in [
                "theme",
                "ui",
                "checklist",
                "layout",
                "notifications",
                "mini-player",
                "call-split",
                "stream",
            ] {
                assert!(!k.contains(other), "{k} is not the welcome's");
            }
        }
        assert_eq!(p.host_files, vec!["onboarding.json".to_string()]);
    }

    #[test]
    fn the_pages_core_answers_it_through_dispatch() {
        let got: serde_json::Value =
            serde_json::from_str(&crate::dispatch::call("onboarding.resetPlan", "{}").unwrap())
                .unwrap();
        assert_eq!(got, serde_json::to_value(plan()).unwrap());
        assert_eq!(got["firstStep"], "welcome");
        assert!(got["webKeys"].as_array().unwrap().len() == WEB_KEYS.len());
    }

    /// The golden the apps check their own lists against.
    #[test]
    fn it_matches_the_golden() {
        let path = format!(
            "{}/contracts/onboarding-reset.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let got = serde_json::to_string_pretty(&plan()).unwrap() + "\n";
        if std::env::var("UPDATE_PARITY").is_ok() {
            std::fs::write(&path, &got).unwrap();
        }
        let want = std::fs::read_to_string(&path).expect("contracts/onboarding-reset.json");
        assert_eq!(got, want, "run with UPDATE_PARITY=1 to update");
    }
}
