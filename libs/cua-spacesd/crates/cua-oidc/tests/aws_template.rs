// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Drift gate: the AWS template's inline policy is cua-byoc's least-privilege
//! `policy/aws-policy.json` plus read-only `ce:GetCostAndUsage`, statement
//! for statement, and the trust policy names only this account's issuer.

use serde_json::{json, Value};

const AWS_POLICY: &str = include_str!("../../../../cua/crates/cua-byoc/policy/aws-policy.json");

fn yaml_to_json(v: serde_yaml::Value) -> Value {
    serde_json::to_value(v).unwrap()
}

fn rendered() -> Value {
    let text = cua_oidc::api::render_aws_template("https://oidc.cua.ai", "abcdefgh2345");
    // CloudFormation's short-form tags (!Sub, !GetAtt, !Ref) are YAML tags;
    // read the document with them stripped to plain values.
    let plain = text
        .replace("!Sub ", "")
        .replace("!GetAtt ", "")
        .replace("!Ref ", "");
    yaml_to_json(serde_yaml::from_str(&plain).unwrap())
}

/// What CloudFormation's `!Sub` makes of `s` with this account's defaults.
fn sub(doc: &Value, s: &str) -> String {
    let p = &doc["Parameters"];
    s.replace(
        "${IssuerHost}",
        p["IssuerHost"]["Default"].as_str().unwrap(),
    )
    .replace(
        "${CuaAccount}",
        p["CuaAccount"]["Default"].as_str().unwrap(),
    )
}

#[test]
fn template_policy_is_aws_policy_json_plus_cost_read() {
    let doc = rendered();
    let policies = doc["Resources"]["CuaSpacesRole"]["Properties"]["Policies"]
        .as_array()
        .unwrap();
    assert_eq!(policies.len(), 1);
    let mut want: Value = serde_json::from_str(AWS_POLICY).unwrap();
    want["Statement"].as_array_mut().unwrap().push(json!({
        "Sid": "CuaReadCostOfWhatItCreates",
        "Effect": "Allow",
        "Action": "ce:GetCostAndUsage",
        "Resource": "*",
    }));
    assert_eq!(policies[0]["PolicyDocument"], want);
    // Still no IAM actions.
    assert!(!policies[0]["PolicyDocument"].to_string().contains("\"iam:"));
}

#[test]
fn defaults_are_this_accounts_issuer() {
    let doc = rendered();
    assert_eq!(doc["Parameters"]["CuaAccount"]["Default"], "abcdefgh2345");
    assert_eq!(doc["Parameters"]["IssuerHost"]["Default"], "oidc.cua.ai");
    // Exactly one Default each (the shared template's own one is replaced).
    let text = cua_oidc::api::render_aws_template("https://oidc.staging.cua.ai/", "abcdefgh2345");
    assert_eq!(text.matches("Default: \"oidc.staging.cua.ai\"").count(), 1);
    assert!(!text.contains("Default: oidc.cua.ai"));
}

#[test]
fn trust_is_scoped_to_the_accounts_issuer() {
    let doc = rendered();
    let provider = &doc["Resources"]["CuaOidcProvider"]["Properties"];
    assert_eq!(
        sub(&doc, provider["Url"].as_str().unwrap()),
        "https://oidc.cua.ai/a/abcdefgh2345"
    );
    assert_eq!(provider["ClientIdList"], json!(["sts.amazonaws.com"]));
    assert!(provider.get("ThumbprintList").is_none());
    let role = &doc["Resources"]["CuaSpacesRole"]["Properties"];
    assert_eq!(role["RoleName"], "cua-spaces");
    assert_eq!(role["MaxSessionDuration"], 3600);
    let trust: Value = serde_json::from_str(&sub(
        &doc,
        role["AssumeRolePolicyDocument"].as_str().unwrap(),
    ))
    .unwrap();
    let stmt = &trust["Statement"];
    assert_eq!(stmt.as_array().unwrap().len(), 1);
    assert_eq!(stmt[0]["Action"], "sts:AssumeRoleWithWebIdentity");
    assert_eq!(
        stmt[0]["Condition"],
        json!({"StringEquals": {
            "oidc.cua.ai/a/abcdefgh2345:aud": "sts.amazonaws.com",
            "oidc.cua.ai/a/abcdefgh2345:sub": "account:abcdefgh2345",
        }})
    );
    assert!(doc["Outputs"]["RoleArn"].is_object());
}
