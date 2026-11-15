// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The small tag in the title bar that says which Cua deployment a build
//! talks to ("Local · Staging", "Local · Prod"), so whoever runs a Local or
//! staging build can always tell it apart from the released app.
//!
//! The released app on production shows nothing: people who install Cua
//! Spaces never see it. Every other build shows it:
//!
//! | Channel | Environment | Updates  | Tag                 |
//! |---------|-------------|----------|---------------------|
//! | Stable  | prod        | any      | none                |
//! | Stable  | staging     | Stable   | `Staging`           |
//! | Stable  | staging     | Nightly  | `Nightly · Staging` |
//! | Local   | prod        | any      | `Local · Prod`      |
//! | Local   | staging     | any      | `Local · Staging`   |
//!
//! The host reads the channel and the environment from `cua-home`
//! (`CuaChannel`, `CuaEnv`, through `app_target` in the Spaces app export,
//! which every shell calls), so the macOS, Windows and Linux apps agree.

use serde::{Deserialize, Serialize};

use crate::about::UpdateChannel;

/// Production's sign-in host, when the host names none.
const PROD_AUTH_HOST: &str = "auth.cua.ai";
/// Staging's sign-in host, when the host names none.
const STAGING_AUTH_HOST: &str = "auth.staging.cua.ai";

/// Which build and deployment the app runs as, resolved by the host
/// (`cua_home::channel::CuaChannel`, `cua_home::env::CuaEnv`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Target {
    /// `stable` or `local` (`CuaChannel`); anything else is Stable.
    pub channel: String,
    /// `prod` or `staging` (`CuaEnv::name`).
    pub env: String,
    /// The sign-in server's host (`auth.staging.cua.ai`), from the resolved
    /// OIDC issuer, so an override (`CUA_OIDC_ISSUER`) shows as it is.
    /// Empty: the environment's default.
    pub auth_host: String,
}

/// Which deployment a tag names: its tint differs, so the two are told
/// apart at a glance.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TagEnv {
    /// Production.
    #[default]
    Prod,
    /// Staging.
    Staging,
}

/// The tag, when this build shows one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentTag {
    /// The words on the tag: `Local · Staging`.
    pub label: String,
    /// The deployment, for the tint.
    pub env: TagEnv,
    /// The tooltip and accessible name: `This app uses Cua Staging
    /// (auth.staging.cua.ai)`.
    pub description: String,
}

/// The tag for `target`, with the updater on `updates` (`None`: the host
/// has no updater); `None` for the released app on production.
pub fn view(target: &Target, updates: Option<UpdateChannel>) -> Option<EnvironmentTag> {
    let local = target.channel.trim().eq_ignore_ascii_case("local");
    let env = match target.env.trim().to_ascii_lowercase().as_str() {
        "staging" => TagEnv::Staging,
        "prod" | "production" => TagEnv::Prod,
        // As `cua-home` reads an unknown `CUA_ENV`: never production on Local.
        _ if local => TagEnv::Staging,
        _ => TagEnv::Prod,
    };
    let channel = if local {
        Some("Local")
    } else if env == TagEnv::Prod {
        // The released app on production: nothing to tell apart.
        return None;
    } else if updates == Some(UpdateChannel::Nightly) {
        Some("Nightly")
    } else {
        None
    };
    let (word, deployment, default_host) = match env {
        TagEnv::Prod => ("Prod", "Cua Production", PROD_AUTH_HOST),
        TagEnv::Staging => ("Staging", "Cua Staging", STAGING_AUTH_HOST),
    };
    let host = match target.auth_host.trim() {
        "" => default_host,
        h => h,
    };
    Some(EnvironmentTag {
        label: match channel {
            Some(c) => format!("{c} · {word}"),
            None => word.to_string(),
        },
        env,
        description: format!("This app uses {deployment} ({host})"),
    })
}

/// The sign-in host of an OIDC issuer URL
/// (`https://auth.staging.cua.ai/realms/cua-staging` is
/// `auth.staging.cua.ai`); empty when it has none.
pub fn auth_host(issuer: &str) -> String {
    let rest = issuer
        .trim()
        .split_once("://")
        .map_or(issuer.trim(), |(_, r)| r);
    rest.split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(channel: &str, env: &str) -> Target {
        Target {
            channel: channel.into(),
            env: env.into(),
            auth_host: String::new(),
        }
    }

    #[test]
    fn the_released_app_on_production_shows_nothing() {
        for updates in [
            None,
            Some(UpdateChannel::Stable),
            Some(UpdateChannel::Nightly),
        ] {
            assert_eq!(view(&target("stable", "prod"), updates), None);
        }
        // No channel at all is Stable.
        assert_eq!(view(&target("", "prod"), None), None);
    }

    #[test]
    fn local_names_its_deployment() {
        let staging = view(&target("local", "staging"), None).unwrap();
        assert_eq!(staging.label, "Local · Staging");
        assert_eq!(staging.env, TagEnv::Staging);
        assert_eq!(
            staging.description,
            "This app uses Cua Staging (auth.staging.cua.ai)"
        );

        let prod = view(&target("local", "prod"), Some(UpdateChannel::Nightly)).unwrap();
        assert_eq!(prod.label, "Local · Prod");
        assert_eq!(prod.env, TagEnv::Prod);
        assert_eq!(
            prod.description,
            "This app uses Cua Production (auth.cua.ai)"
        );
    }

    #[test]
    fn a_stable_build_on_staging_says_so() {
        let tag = view(&target("stable", "staging"), Some(UpdateChannel::Stable)).unwrap();
        assert_eq!(tag.label, "Staging");
        let nightly = view(&target("stable", "staging"), Some(UpdateChannel::Nightly)).unwrap();
        assert_eq!(nightly.label, "Nightly · Staging");
    }

    #[test]
    fn an_unknown_environment_is_never_production_on_local() {
        assert_eq!(
            view(&target("local", "prd"), None).unwrap().env,
            TagEnv::Staging
        );
        assert_eq!(view(&target("stable", "prd"), None), None);
    }

    #[test]
    fn the_description_names_the_resolved_sign_in_host() {
        let mut t = target("local", "staging");
        t.auth_host = "auth.example.test".into();
        assert_eq!(
            view(&t, None).unwrap().description,
            "This app uses Cua Staging (auth.example.test)"
        );
    }

    #[test]
    fn auth_host_reads_an_issuer() {
        assert_eq!(
            auth_host("https://auth.staging.cua.ai/realms/cua-staging"),
            "auth.staging.cua.ai"
        );
        assert_eq!(auth_host("https://auth.cua.ai"), "auth.cua.ai");
        assert_eq!(auth_host("auth.cua.ai/realms/x"), "auth.cua.ai");
        assert_eq!(auth_host(""), "");
    }

    #[test]
    fn the_wire_shape_is_camel_case() {
        let tag = view(&target("local", "staging"), None).unwrap();
        let json = serde_json::to_value(&tag).unwrap();
        assert_eq!(json["env"], "staging");
        assert_eq!(json["label"], "Local · Staging");
        let t: Target = serde_json::from_value(
            serde_json::json!({"channel": "local", "env": "prod", "authHost": "auth.cua.ai"}),
        )
        .unwrap();
        assert_eq!(t.auth_host, "auth.cua.ai");
    }
}
