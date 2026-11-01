// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! What makes a customer's cloud trust Cua's issuer, as files that ship
//! with Cua and the links and commands that run them. Nothing here calls a
//! cloud: a person runs them in their own console, or an agent signed in to
//! the cloud's CLI runs the exact command `cua cloud connect` gives it.
//!
//! | cloud | file | a person | an agent with the cloud's CLI |
//! |---|---|---|---|
//! | AWS | `templates/aws/cua-spaces.yaml` (CloudFormation) | quick-create link or CloudShell | `aws cloudformation deploy` |
//! | Google Cloud | `templates/gcp/cua-trust.sh` | Cloud Shell, then the script | `cua cloud trust gcp \| bash -s -- …` |
//! | Azure | `templates/azure/cua-spaces.json` (ARM, resource group scope) | Deploy to Azure | `az deployment group create` |
//!
//! `cua cloud trust <cloud>` prints the file, so an agent needs nothing but
//! `cua` and the cloud's CLI.

/// The repository the files are published in (Cloud Shell clones it, the
/// Deploy to Azure link reads the template from it).
pub const REPO: &str = "https://github.com/trycua/cua";

/// The branch or tag of [`REPO`] to read.
pub const REF: &str = "main";

/// Where the files are in [`REPO`].
pub const DIR: &str = "libs/cua/crates/cua-byoc/templates";

/// `CUA_TRUST_REPO` (another repository, a fork), else [`REPO`].
pub fn repo() -> String {
    env_or("CUA_TRUST_REPO", REPO)
        .trim_end_matches('/')
        .to_string()
}

/// `CUA_TRUST_REF`, else [`REF`].
pub fn git_ref() -> String {
    env_or("CUA_TRUST_REF", REF)
}

fn env_or(var: &str, default: &str) -> String {
    std::env::var(var)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// The raw URL of `file` (under [`DIR`]) in a GitHub `repo` at `git_ref`.
pub fn raw_url(repo: &str, git_ref: &str, file: &str) -> String {
    let path = repo
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .trim_start_matches("https://github.com/");
    format!("https://raw.githubusercontent.com/{path}/{git_ref}/{DIR}/{file}")
}

/// The issuer host of `issuer_url` (`oidc.cua.ai`).
pub fn issuer_host(issuer_url: &str) -> String {
    let s = issuer_url
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    s.split('/').next().unwrap_or(s).to_string()
}

/// Percent-encodes everything but RFC 3986's unreserved characters.
pub fn pct(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// The setup file of `cloud` (`cua cloud trust <cloud>`).
pub fn file(cloud: &str) -> Option<&'static str> {
    match cloud {
        "aws" => Some(aws::TEMPLATE),
        "gcp" => Some(gcp::SCRIPT),
        "azure" => Some(azure::TEMPLATE),
        _ => None,
    }
}

/// AWS: the CloudFormation stack (see [`crate::federation`] for the
/// quick-create link).
pub mod aws {
    /// The stack.
    pub const TEMPLATE: &str = include_str!("../templates/aws/cua-spaces.yaml");
}

/// Google Cloud: a workload identity pool and provider, a custom role and
/// one binding, set up by a script (`templates/gcp/cua-trust.sh`).
pub mod gcp {
    use super::*;

    /// The script.
    pub const SCRIPT: &str = include_str!("../templates/gcp/cua-trust.sh");

    /// The workload identity pool the script creates.
    pub const POOL: &str = "cua";

    /// The OIDC provider in it.
    pub const PROVIDER: &str = "cua";

    /// The custom role it creates (the permissions of
    /// [`crate::gcp::PERMISSIONS`] when that provider is built).
    pub const ROLE: &str = "cuaSpaces";

    /// The provider's full resource name, the token's audience:
    /// `//iam.googleapis.com/projects/<number>/locations/global/workloadIdentityPools/<pool>/providers/<provider>`.
    pub fn provider_name(project_number: &str, pool: &str, provider: &str) -> String {
        format!(
            "//iam.googleapis.com/projects/{project_number}/locations/global/workloadIdentityPools/{pool}/providers/{provider}"
        )
    }

    /// The project number in a provider name.
    pub fn project_number_of(provider_name: &str) -> Option<&str> {
        let n = provider_name
            .strip_prefix("//iam.googleapis.com/projects/")?
            .split('/')
            .next()?;
        (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).then_some(n)
    }

    /// The permissions the script's role gets, as the script lists them.
    pub fn script_permissions() -> Vec<&'static str> {
        let Some(start) = SCRIPT.find("PERMISSIONS=(") else {
            return vec![];
        };
        let body = &SCRIPT[start + "PERMISSIONS=(".len()..];
        let end = body.find(')').unwrap_or(body.len());
        body[..end]
            .split_whitespace()
            .filter(|w| !w.starts_with('#'))
            .collect()
    }

    /// The "Open in Cloud Shell" link: Google Cloud Shell clones `repo` at
    /// `git_ref` and opens a terminal in the directory of the script.
    pub fn cloud_shell_url(repo: &str, git_ref: &str) -> String {
        format!(
            "https://shell.cloud.google.com/cloudshell/editor?cloudshell_git_repo={}\
             &cloudshell_git_branch={}&cloudshell_workspace={}&show=terminal",
            pct(repo),
            pct(git_ref),
            pct(&format!("{DIR}/gcp")),
        )
    }

    /// The arguments of the script.
    fn args(project: &str, acct: &str, issuer_host: &str) -> String {
        format!("--project {project} --account {acct} --issuer-host {issuer_host}")
    }

    /// What a person pastes in Cloud Shell (opened by
    /// [`cloud_shell_url`], in the script's directory).
    pub fn paste(project: &str, acct: &str, issuer_host: &str) -> String {
        format!("bash cua-trust.sh {}", args(project, acct, issuer_host))
    }

    /// What an agent signed in to gcloud (with rights to manage IAM in the
    /// project) runs: the script this `cua` ships, piped to bash.
    pub fn agent_command(project: &str, acct: &str, issuer_host: &str) -> String {
        format!(
            "cua cloud trust gcp | bash -s -- {}",
            args(project, acct, issuer_host)
        )
    }
}

/// Azure: a user-assigned managed identity `cua-spaces` trusting Cua's
/// tokens, with Virtual Machine Contributor and Network Contributor on one
/// resource group only (`templates/azure/cua-spaces.json`, deployed at
/// resource group scope).
pub mod azure {
    use super::*;

    /// The ARM template.
    pub const TEMPLATE: &str = include_str!("../templates/azure/cua-spaces.json");

    /// The managed identity it creates.
    pub const IDENTITY: &str = "cua-spaces";

    /// The resource group Cua uses unless told otherwise.
    pub const DEFAULT_RESOURCE_GROUP: &str = "cua-spaces";

    /// The audience Entra expects of a federated token.
    pub const AUDIENCE: &str = "api://AzureADTokenExchange";

    /// The built-in roles it grants on the resource group, by id
    /// (Virtual Machine Contributor, Network Contributor).
    pub const ROLES: &[(&str, &str)] = &[
        (
            "Virtual Machine Contributor",
            "9980e02c-c2be-4d73-94e8-173b1dc7cf3c",
        ),
        (
            "Network Contributor",
            "4d97b98b-1d4f-4787-a291-c67834d212e7",
        ),
    ];

    /// `CUA_AZURE_TEMPLATE_URL`, else the template's raw URL in
    /// [`repo()`] at [`git_ref()`].
    pub fn template_url() -> String {
        env_or(
            "CUA_AZURE_TEMPLATE_URL",
            &raw_url(&repo(), &git_ref(), "azure/cua-spaces.json"),
        )
    }

    /// The "Deploy to Azure" link of the template at `template_url`.
    pub fn deploy_url(template_url: &str) -> String {
        format!(
            "https://portal.azure.com/#create/Microsoft.Template/uri/{}",
            pct(template_url)
        )
    }

    /// What an agent signed in to the az CLI (with rights to assign roles
    /// on the resource group) runs: the group, then the template this
    /// `cua` ships, at resource group scope. It prints the ids Cua needs.
    pub fn agent_command(
        subscription: Option<&str>,
        resource_group: &str,
        region: &str,
        acct: &str,
        issuer_host: &str,
    ) -> String {
        let sub = subscription
            .map(|s| format!(" --subscription {s}"))
            .unwrap_or_default();
        format!(
            "cua cloud trust azure > cua-spaces.json && \
             az group create --name {resource_group} --location {region}{sub} --output none && \
             az deployment group create --name cua-spaces --resource-group {resource_group}{sub} \
             --template-file cua-spaces.json --parameters cuaAccount={acct} issuerHost={issuer_host} \
             --query properties.outputs --output json"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_point_at_the_published_files() {
        assert_eq!(
            raw_url(REPO, REF, "azure/cua-spaces.json"),
            "https://raw.githubusercontent.com/trycua/cua/main/libs/cua/crates/cua-byoc/templates/azure/cua-spaces.json"
        );
        assert_eq!(
            gcp::cloud_shell_url(REPO, REF),
            "https://shell.cloud.google.com/cloudshell/editor?cloudshell_git_repo=https%3A%2F%2Fgithub.com%2Ftrycua%2Fcua\
             &cloudshell_git_branch=main&cloudshell_workspace=libs%2Fcua%2Fcrates%2Fcua-byoc%2Ftemplates%2Fgcp&show=terminal"
        );
        assert_eq!(
            azure::deploy_url("https://example.com/t.json"),
            "https://portal.azure.com/#create/Microsoft.Template/uri/https%3A%2F%2Fexample.com%2Ft.json"
        );
        assert_eq!(issuer_host("https://oidc.cua.ai/a/x"), "oidc.cua.ai");
        assert_eq!(
            gcp::provider_name("123456", gcp::POOL, gcp::PROVIDER),
            "//iam.googleapis.com/projects/123456/locations/global/workloadIdentityPools/cua/providers/cua"
        );
        assert_eq!(
            gcp::project_number_of(&gcp::provider_name("123456", "cua", "cua")),
            Some("123456")
        );
        assert_eq!(
            gcp::project_number_of("//iam.googleapis.com/projects/abc/x"),
            None
        );
        assert_eq!(
            gcp::paste("my-proj", "a1b2c3d4e5f6", "oidc.cua.ai"),
            "bash cua-trust.sh --project my-proj --account a1b2c3d4e5f6 --issuer-host oidc.cua.ai"
        );
    }

    /// The script trusts exactly what the docs and the provider say.
    #[test]
    fn the_gcp_script_sets_up_the_documented_trust() {
        let s = gcp::SCRIPT;
        for want in [
            "POOL=\"cua\"",
            "PROVIDER=\"cua\"",
            "ROLE=\"cuaSpaces\"",
            "google.subject=assertion.sub,attribute.account=assertion.sub,attribute.device=assertion.cua_device,attribute.purpose=assertion.cua_purpose",
            "CONDITION=\"assertion.sub=='account:${ACCOUNT}'\"",
            "ISSUER=\"https://${ISSUER_HOST}/a/${ACCOUNT}\"",
            "AUDIENCE=\"//iam.googleapis.com/${PROVIDER_NAME}\"",
            "--allowed-audiences=\"$AUDIENCE\"",
            "principalSet://iam.googleapis.com/projects/${NUMBER}/locations/global/workloadIdentityPools/${POOL}/attribute.account/account:${ACCOUNT}",
            "set -euo pipefail",
        ] {
            assert!(s.contains(want), "the script lacks {want}");
        }
        // Idempotent: each thing it creates is looked up first.
        for (look, make) in [
            (
                "workload-identity-pools describe",
                "workload-identity-pools create",
            ),
            ("providers describe", "providers create-oidc"),
            ("roles describe", "roles create"),
        ] {
            let (l, m) = (s.find(look).unwrap(), s.find(make).unwrap());
            assert!(l < m, "{make} is not behind {look}");
        }
        // No keys, no service accounts.
        assert!(!s.contains("keys create"));
        assert!(!s.contains("service-accounts create"));
        assert!(!gcp::script_permissions().is_empty());
    }

    #[test]
    fn the_azure_template_trusts_one_account_on_one_resource_group() {
        let t: serde_json::Value = serde_json::from_str(azure::TEMPLATE).unwrap();
        assert!(
            t["$schema"]
                .as_str()
                .unwrap()
                .ends_with("/2019-04-01/deploymentTemplate.json#"),
            "resource group scope"
        );
        let res = t["resources"].as_array().unwrap();
        let of = |ty: &str| -> Vec<&serde_json::Value> {
            res.iter().filter(|r| r["type"] == ty).collect()
        };
        let id = of("Microsoft.ManagedIdentity/userAssignedIdentities");
        assert_eq!(id.len(), 1);
        assert_eq!(t["variables"]["identityName"], azure::IDENTITY);
        let fic =
            of("Microsoft.ManagedIdentity/userAssignedIdentities/federatedIdentityCredentials");
        assert_eq!(fic.len(), 1);
        let p = &fic[0]["properties"];
        assert_eq!(
            p["issuer"],
            "[format('https://{0}/a/{1}', parameters('issuerHost'), parameters('cuaAccount'))]"
        );
        assert_eq!(
            p["subject"],
            "[format('account:{0}', parameters('cuaAccount'))]"
        );
        assert_eq!(p["audiences"], serde_json::json!([azure::AUDIENCE]));
        let roles = of("Microsoft.Authorization/roleAssignments");
        assert_eq!(roles.len(), azure::ROLES.len());
        for (name, role) in azure::ROLES {
            assert_eq!(
                t["variables"]
                    .as_object()
                    .unwrap()
                    .values()
                    .filter(|v| v.as_str() == Some(role))
                    .count(),
                1,
                "{name}"
            );
        }
        for r in roles {
            // No `scope`: the deployment's resource group, nothing wider.
            assert!(r.get("scope").is_none(), "{r}");
            assert_eq!(r["properties"]["principalType"], "ServicePrincipal");
        }
        for out in ["clientId", "tenantId", "subscriptionId", "resourceGroup"] {
            assert!(t["outputs"].get(out).is_some(), "{out}");
        }
        // Nothing else: no keys, no secrets, no other resources.
        assert_eq!(res.len(), 4);
        assert!(!azure::TEMPLATE.to_ascii_lowercase().contains("secret"));
    }
}
