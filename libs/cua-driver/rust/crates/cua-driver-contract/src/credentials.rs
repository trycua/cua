// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Provider-neutral credential discovery and target-bound delivery contracts.

use crate::{
    ActionResult, CursorAction, CursorSemantics, Platform, SchemaMode, ToolAnnotations,
    ToolContract, ToolInput, ToolOutput, MULTI_CALL_SESSION_DESCRIPTION,
};
use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

const ALL_PLATFORMS: [Platform; 3] = [Platform::Macos, Platform::Windows, Platform::Linux];

fn bounded_string_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "string", "minLength": 1, "maxLength": 256 })
}

fn handle_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": "string",
        "minLength": 35,
        "maxLength": 35,
        "pattern": "^ch-[0-9a-f]{32}$"
    })
}

fn safe_label_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "string", "minLength": 1, "maxLength": 80 })
}

fn session_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": "string",
        "minLength": 1,
        "maxLength": 200,
        "description": MULTI_CALL_SESSION_DESCRIPTION
    })
}

#[derive(
    Debug,
    Clone,
    Copy,
    Serialize,
    Deserialize,
    JsonSchema,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    uniffi::Enum,
)]
#[serde(rename_all = "snake_case")]
pub enum CredentialField {
    Password,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Enum)]
#[serde(rename_all = "snake_case")]
pub enum CredentialProviderClass {
    ServiceAccountVault,
    InteractiveDesktop,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Record)]
#[serde(deny_unknown_fields)]
pub struct CredentialDescriptor {
    #[schemars(schema_with = "handle_schema")]
    pub handle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "safe_label_schema")]
    pub label: Option<String>,
    #[schemars(length(min = 1, max = 1))]
    pub fields: Vec<CredentialField>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_class: Option<CredentialProviderClass>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Record)]
#[serde(deny_unknown_fields)]
pub struct FindCredentialsInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "session_schema")]
    pub session: Option<String>,
    #[schemars(schema_with = "bounded_string_schema")]
    pub target_id: String,
    #[schemars(schema_with = "bounded_string_schema")]
    pub tab_id: String,
    #[serde(rename = "ref")]
    #[schemars(schema_with = "bounded_string_schema")]
    pub element_ref: String,
}

impl ToolInput for FindCredentialsInput {
    const TOOL_NAME: &'static str = "find_credentials";
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Record)]
#[serde(deny_unknown_fields)]
pub struct FindCredentialsOutput {
    pub credentials: Vec<CredentialDescriptor>,
}

impl ToolOutput for FindCredentialsOutput {
    fn validate(&self) -> Result<(), String> {
        let mut handles = BTreeSet::new();
        for descriptor in &self.credentials {
            if !valid_handle(&descriptor.handle) {
                return Err("credential descriptor handle is malformed".into());
            }
            if !handles.insert(descriptor.handle.as_str()) {
                return Err("credential descriptor handles must be unique".into());
            }
            if descriptor.fields.as_slice() != [CredentialField::Password] {
                return Err(
                    "credential descriptor fields must contain password exactly once".into(),
                );
            }
            if descriptor.label.as_ref().is_some_and(|label| {
                label.is_empty()
                    || label.chars().count() > 80
                    || label.chars().any(char::is_control)
            }) {
                return Err("credential descriptor label is invalid".into());
            }
        }
        Ok(())
    }

    fn output_schema() -> Value {
        crate::outputs::output_schema_with_additional_properties::<Self>(false)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Record)]
#[serde(deny_unknown_fields)]
pub struct TypeSecretInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "session_schema")]
    pub session: Option<String>,
    #[schemars(schema_with = "bounded_string_schema")]
    pub target_id: String,
    #[schemars(schema_with = "bounded_string_schema")]
    pub tab_id: String,
    #[serde(rename = "ref")]
    #[schemars(schema_with = "bounded_string_schema")]
    pub element_ref: String,
    #[schemars(schema_with = "handle_schema")]
    pub handle: String,
    pub field: CredentialField,
}

impl ToolInput for TypeSecretInput {
    const TOOL_NAME: &'static str = "type_secret";
}

pub fn contracts() -> Vec<ToolContract> {
    vec![find_credentials(), type_secret()]
}

fn find_credentials() -> ToolContract {
    contract::<FindCredentialsInput, FindCredentialsOutput>(
        "find_credentials",
        "Find trusted-host-registered credentials that are already authorized for one exact live semantic password field. Returns only safe descriptors and fresh opaque target-bound handles; it never queries a provider vault or returns secret material.",
        &["credentials.find", "browser.state"],
        ToolAnnotations {
            read_only: true,
            destructive: false,
            idempotent: false,
            open_world: false,
        },
        CursorAction::Observe,
    )
}

fn type_secret() -> ToolContract {
    contract::<TypeSecretInput, ActionResult>(
        "type_secret",
        "Release one trusted-host-registered credential through a fresh single-use handle to the same exact live semantic password field. The secret never appears in public arguments or results, clipboard state, generic typing, or replayable recordings.",
        &["credentials.release", "browser.input.secret"],
        ToolAnnotations {
            read_only: false,
            destructive: true,
            idempotent: false,
            open_world: true,
        },
        CursorAction::Text,
    )
}

fn contract<I: ToolInput, O: ToolOutput>(
    name: &str,
    description: &str,
    capabilities: &[&str],
    annotations: ToolAnnotations,
    cursor_action: CursorAction,
) -> ToolContract {
    assert_eq!(name, I::TOOL_NAME, "typed input is bound to the wrong tool");
    ToolContract {
        name: name.into(),
        description: description.into(),
        platforms: ALL_PLATFORMS.to_vec(),
        aliases: Vec::new(),
        capabilities: capabilities.iter().map(|value| (*value).into()).collect(),
        annotations,
        schema_mode: SchemaMode::CanonicalRuntime,
        cursor_semantics: Some(CursorSemantics::new(cursor_action)),
        input_schema: I::input_schema(),
        success_output_schema: Some(O::output_schema()),
        output_validator: crate::validate_typed_output::<O>,
    }
}

fn valid_handle(value: &str) -> bool {
    value.len() == 35
        && value.starts_with("ch-")
        && value[3..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_schemas_are_closed_and_have_no_secret_or_provider_locator_input() {
        for contract in contracts() {
            assert_eq!(contract.input_schema["additionalProperties"], false);
            let rendered = contract.input_schema.to_string();
            for forbidden in [
                "secret",
                "password_value",
                "locator",
                "vault",
                "provider_id",
            ] {
                assert!(
                    !rendered.contains(forbidden),
                    "{} schema leaked {forbidden}",
                    contract.name
                );
            }
        }
    }

    #[test]
    fn descriptor_validation_rejects_malformed_or_duplicate_handles() {
        let descriptor = CredentialDescriptor {
            handle: "ch-0123456789abcdef0123456789abcdef".into(),
            label: Some("Synthetic login".into()),
            fields: vec![CredentialField::Password],
            provider_class: Some(CredentialProviderClass::ServiceAccountVault),
        };
        assert!(FindCredentialsOutput {
            credentials: vec![descriptor.clone()]
        }
        .validate()
        .is_ok());
        assert!(FindCredentialsOutput {
            credentials: vec![descriptor.clone(), descriptor]
        }
        .validate()
        .is_err());
    }
}
