// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The least-privilege access Cua needs in your cloud account, published.
//!
//! Cua creates no IAM users, roles or keys, so it runs as whatever identity
//! you connect. [`aws_policy_json`] is the IAM policy to give that identity
//! instead of broad credentials: it lets Cua create only resources it tags
//! `cua-managed=true`, and stop, start, terminate and delete only resources
//! that carry the tag. The file is the single source of truth; the docs page
//! [`DOCS_URL`] shows it verbatim, and tests keep the three (the policy, the
//! calls `aws.rs` makes, the docs) from drifting apart. The Google Cloud
//! list is [`crate::gcp::PERMISSIONS`], checked against the same page and
//! the trust script's custom role; Azure's built-in roles are checked
//! against the trust template and the page.

/// The docs page that shows the policy and how to attach it.
pub const DOCS_URL: &str =
    "https://cua.ai/docs/cua-sdk/guides/your-cloud-safety#least-privilege-access";

/// The IAM policy document (JSON) for the `aws` provider.
pub fn aws_policy_json() -> &'static str {
    include_str!("../policy/aws-policy.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::collections::BTreeSet;

    const AWS_SOURCE: &str = include_str!("aws.rs");
    const GCP_SOURCE: &str = include_str!("gcp.rs");
    const DOCS: &str =
        include_str!("../../../../../docs/content/docs/cua-sdk/guides/your-cloud-safety.mdx");

    /// Actions the provider uses without calling them by name: tags set in
    /// a create call are authorized as `ec2:CreateTags`.
    const IMPLICIT: &[&str] = &["ec2:CreateTags"];

    /// Resource types a request cannot tag, so a launch or group create may
    /// touch them without the tag condition.
    const UNTAGGED_OK: &[&str] = &[
        "image/",
        "subnet/",
        "network-interface/",
        "security-group/*",
        "vpc/",
    ];

    /// The inline policy of the CloudFormation trust stack: between the
    /// `# BEGIN cua-policy` and `# END cua-policy` lines, after
    /// `PolicyDocument:`.
    fn template_policy() -> Value {
        let t = include_str!("../templates/aws/cua-spaces.yaml");
        let block = t
            .split("# BEGIN cua-policy")
            .nth(1)
            .and_then(|b| b.split("# END cua-policy").next())
            .expect("the template marks its policy");
        let json = block
            .split_once("PolicyDocument:")
            .expect("PolicyDocument")
            .1;
        serde_json::from_str(json).expect("the template's policy is JSON")
    }

    #[test]
    fn the_trust_stack_grants_exactly_the_published_policy_plus_a_cost_read() {
        let mut want = policy();
        want["Statement"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "Sid": "CuaReadCostOfWhatItCreates",
                "Effect": "Allow",
                "Action": "ce:GetCostAndUsage",
                "Resource": "*"
            }));
        assert_eq!(template_policy(), want);
        let t = include_str!("../templates/aws/cua-spaces.yaml");
        // No IAM actions, and the trust names the per-account issuer's
        // audience and subject.
        let actions: Vec<String> = template_policy()["Statement"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|s| strings(&s["Action"]))
            .collect();
        assert!(actions.contains(&"ec2:RunInstances".to_string()));
        assert!(!actions.iter().any(|a| a.starts_with("iam:")));
        assert!(t.contains("RoleName: cua-spaces"));
        assert!(t.contains(r#""${IssuerHost}/a/${CuaAccount}:aud": "sts.amazonaws.com""#));
        assert!(t.contains(r#""${IssuerHost}/a/${CuaAccount}:sub": "account:${CuaAccount}""#));
        assert!(t.contains("sts:AssumeRoleWithWebIdentity"));
        assert!(t.contains("MaxSessionDuration: 3600"));
    }

    fn policy() -> Value {
        serde_json::from_str(aws_policy_json()).expect("the policy is JSON")
    }

    fn strings(v: &Value) -> Vec<String> {
        match v {
            Value::String(s) => vec![s.clone()],
            Value::Array(a) => a.iter().flat_map(strings).collect(),
            _ => vec![],
        }
    }

    fn statements() -> Vec<Value> {
        policy()["Statement"].as_array().expect("Statement").clone()
    }

    fn pascal(snake: &str) -> String {
        snake
            .split('_')
            .map(|w| w[..1].to_uppercase() + &w[1..])
            .collect()
    }

    /// Every `ec2.<op>(` and `sts.<op>(` call in `aws.rs` outside its tests,
    /// as `service:Operation`, plus the SSM read the image parameter needs.
    fn used_actions() -> BTreeSet<String> {
        let src = AWS_SOURCE.split("#[cfg(test)]").next().unwrap();
        let b = src.as_bytes();
        let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
        let mut out = BTreeSet::new();
        for svc in ["ec2", "sts"] {
            let mut from = 0;
            while let Some(at) = src[from..].find(svc) {
                let i = from + at;
                from = i + svc.len();
                if i > 0 && ident(b[i - 1]) {
                    continue;
                }
                let mut j = i + svc.len();
                let skip = |j: &mut usize| {
                    while *j < b.len() && b[*j].is_ascii_whitespace() {
                        *j += 1;
                    }
                };
                skip(&mut j);
                if b.get(j) != Some(&b'.') {
                    continue;
                }
                j += 1;
                skip(&mut j);
                let start = j;
                while j < b.len() && ident(b[j]) {
                    j += 1;
                }
                if b.get(j) == Some(&b'(') && j > start {
                    out.insert(format!("{svc}:{}", pascal(&src[start..j])));
                }
            }
        }
        if src.contains("resolve:ssm:") {
            out.insert("ssm:GetParameters".into());
        }
        if src.contains(".tag_specifications(") {
            out.insert("ec2:CreateTags".into());
        }
        out
    }

    fn allowed_actions() -> BTreeSet<String> {
        statements()
            .iter()
            .filter(|s| s["Effect"] == "Allow")
            .flat_map(|s| strings(&s["Action"]))
            .collect()
    }

    #[test]
    fn the_policy_is_valid_and_every_statement_is_complete() {
        let p = policy();
        assert_eq!(p["Version"], "2012-10-17");
        let mut sids = BTreeSet::new();
        for s in statements() {
            let sid = s["Sid"].as_str().expect("every statement has a Sid");
            assert!(sids.insert(sid.to_string()), "duplicate Sid {sid}");
            assert!(
                sid.chars().all(|c| c.is_ascii_alphanumeric()),
                "Sid {sid} must be alphanumeric"
            );
            assert!(
                matches!(s["Effect"].as_str(), Some("Allow" | "Deny")),
                "{sid}"
            );
            assert!(!strings(&s["Action"]).is_empty(), "{sid} has no Action");
            assert!(!strings(&s["Resource"]).is_empty(), "{sid} has no Resource");
            assert!(s.get("NotAction").is_none() && s.get("NotResource").is_none());
            for a in strings(&s["Action"]) {
                assert!(!a.contains('*'), "{sid}: wildcard action {a}");
            }
        }
        assert!(
            aws_policy_json().len() < 6144,
            "IAM managed policies are limited to 6,144 characters"
        );
    }

    #[test]
    fn every_call_the_provider_makes_is_allowed_and_nothing_else_is() {
        let used = used_actions();
        // The scanner itself works: the calls we know are there.
        for known in [
            "ec2:RunInstances",
            "ec2:TerminateInstances",
            "ec2:DescribeInstances",
            "sts:GetCallerIdentity",
            "ec2:GetConsoleOutput",
        ] {
            assert!(used.contains(known), "scanner missed {known}: {used:?}");
        }
        let allowed = allowed_actions();
        let missing: Vec<_> = used.difference(&allowed).collect();
        assert!(
            missing.is_empty(),
            "aws.rs calls {missing:?} but policy/aws-policy.json does not allow it: add it \
             (tag-scoped if it changes anything) and the docs page"
        );
        let stale: Vec<_> = allowed
            .difference(&used)
            .filter(|a| !IMPLICIT.contains(&a.as_str()))
            .collect();
        assert!(
            stale.is_empty(),
            "policy/aws-policy.json allows {stale:?} but aws.rs never calls it: remove it"
        );
    }

    #[test]
    fn nothing_that_changes_the_account_lacks_the_cua_tag_condition() {
        const MUTATING: &[&str] = &[
            "Run",
            "Create",
            "Delete",
            "Terminate",
            "Stop",
            "Start",
            "Authorize",
            "Revoke",
            "Modify",
            "Attach",
            "Detach",
            "Associate",
            "Disassociate",
            "Reboot",
            "Put",
        ];
        for s in statements().iter().filter(|s| s["Effect"] == "Allow") {
            let sid = s["Sid"].as_str().unwrap();
            let acts = strings(&s["Action"]);
            let changes = acts.iter().any(|a| {
                let op = a.split(':').nth(1).unwrap_or("");
                MUTATING.iter().any(|m| op.starts_with(m))
            });
            if !changes {
                continue;
            }
            let cond = s["Condition"].to_string();
            let tagged = cond.contains("aws:RequestTag/cua-managed")
                || cond.contains("aws:ResourceTag/cua-managed");
            if tagged {
                continue;
            }
            // The only exception: the untaggable things a launch or a group
            // create must reference, and only for those two actions.
            let res = strings(&s["Resource"]);
            let reference_only = acts
                .iter()
                .all(|a| a == "ec2:RunInstances" || a == "ec2:CreateSecurityGroup")
                && res
                    .iter()
                    .all(|r| UNTAGGED_OK.iter().any(|u| r.contains(u)));
            assert!(
                reference_only,
                "{sid}: {acts:?} changes the account without a cua-managed tag condition"
            );
        }
        // Tagging is limited to resources being created.
        let tags = statements()
            .into_iter()
            .find(|s| strings(&s["Action"]).contains(&"ec2:CreateTags".to_string()))
            .expect("CreateTags statement");
        assert!(tags["Condition"].to_string().contains("ec2:CreateAction"));
        // No identity changes, ever.
        for a in allowed_actions() {
            assert!(!a.starts_with("iam:"), "{a}");
        }
    }

    #[test]
    fn the_docs_show_the_policy_and_the_gcp_permissions_verbatim() {
        assert!(
            DOCS.contains(aws_policy_json().trim()),
            "docs/content/docs/cua-sdk/guides/your-cloud-safety.mdx must contain \
             crates/cua-byoc/policy/aws-policy.json verbatim in its Least-privilege access section"
        );
        let block = GCP_SOURCE
            .split("pub const PERMISSIONS: &[&str] = &[")
            .nth(1)
            .and_then(|r| r.split("];").next())
            .expect("gcp PERMISSIONS");
        let code: BTreeSet<String> = block
            .split('"')
            .skip(1)
            .step_by(2)
            .map(str::to_string)
            .collect();
        assert!(code.len() >= 14 && code.contains("compute.instances.create"));
        let docs: BTreeSet<String> = DOCS
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '.'))
            .filter(|w| w.starts_with("compute.") && w.matches('.').count() == 2)
            .map(str::to_string)
            .collect();
        assert_eq!(
            docs, code,
            "the docs list the GCP permissions in gcp.rs, exactly"
        );
        // The trust script's role grants exactly these too.
        let script: BTreeSet<String> = crate::trust::gcp::script_permissions()
            .into_iter()
            .map(str::to_string)
            .collect();
        assert_eq!(
            script, code,
            "templates/gcp/cua-trust.sh grants the permissions in gcp.rs, exactly"
        );
        // Azure: the template grants the two roles the page names, by id.
        let template = crate::trust::azure::TEMPLATE;
        for (name, id) in crate::trust::azure::ROLES {
            assert!(template.contains(id), "the Azure template lacks {name}");
            assert!(
                DOCS.contains(name) && DOCS.contains(id),
                "your-cloud-safety.mdx must name the Azure role {name} ({id})"
            );
        }
        assert!(DOCS.contains("## Least-privilege access"));
        assert!(DOCS_URL.ends_with("#least-privilege-access"));
    }
}
