use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::{fmt, str::FromStr, sync::Arc};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, uniffi::Object)]
pub struct PreservedJson(Value);

#[derive(Debug, Error, uniffi::Error)]
pub enum JsonValueError {
    #[error("value must be valid JSON: {reason}")]
    Invalid { reason: String },
}

impl PreservedJson {
    pub fn parse(value: String) -> Result<Self, JsonValueError> {
        serde_json::from_str::<Value>(&value)
            .map_err(|error| JsonValueError::Invalid {
                reason: error.to_string(),
            })
            .and_then(Self::from_value)
    }

    fn from_value(value: Value) -> Result<Self, JsonValueError> {
        if value.is_object() {
            Ok(Self(value))
        } else {
            Err(JsonValueError::Invalid {
                reason: "value must be a JSON object".into(),
            })
        }
    }

    pub fn as_value(&self) -> &Value {
        &self.0
    }
}

#[uniffi::export]
impl PreservedJson {
    #[uniffi::constructor]
    pub fn from_json(value: String) -> Result<Arc<Self>, JsonValueError> {
        Self::parse(value).map(Arc::new)
    }

    pub fn to_json(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for PreservedJson {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for PreservedJson {
    type Err = JsonValueError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value.to_owned())
    }
}

impl Serialize for PreservedJson {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PreservedJson {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Value::deserialize(deserializer)
            .and_then(|value| Self::from_value(value).map_err(serde::de::Error::custom))
    }
}

impl JsonSchema for PreservedJson {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "PreservedJson".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "object",
            "x-kubernetes-preserve-unknown-fields": true,
        })
    }
}
