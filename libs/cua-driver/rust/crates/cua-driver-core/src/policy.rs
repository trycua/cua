//! Optional permission-policy enforcement for MCP tool calls.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::Context;
use anyhow::{bail, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const POLICY_FILE_ENV: &str = "CUA_DRIVER_POLICY_FILE";
pub const MANAGED_POLICY_FILE_ENV: &str = "CUA_DRIVER_MANAGED_POLICY_FILE";
#[cfg(feature = "rego")]
const REGO_ALLOW_RULE: &str = "data.cua.policy.allow";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyDecision {
    Allow,
    Deny(String),
    Error(String),
}

#[derive(Debug, Clone)]
pub enum PolicyEngine {
    #[cfg(feature = "yaml")]
    Yaml(YamlPolicy),
    #[cfg(feature = "rego")]
    Rego(Box<RegoPolicy>),
    #[cfg(not(any(feature = "yaml", feature = "rego")))]
    Disabled,
}

impl PolicyEngine {
    /// Load an explicitly configured policy file or Rego policy directory.
    ///
    /// Callers represent an *unset* policy as `None` before invoking this
    /// function. Once a path is supplied, a missing path is a configuration
    /// error rather than an implicit request to disable enforcement.
    pub fn load(path: &Path) -> Result<Option<Self>> {
        if !path.exists() {
            bail!(
                "configured permission policy path does not exist: {}",
                path.display()
            );
        }

        if path.is_dir() {
            #[cfg(feature = "rego")]
            {
                return Ok(Some(Self::Rego(Box::new(RegoPolicy::load_directory(
                    path,
                )?))));
            }
            #[cfg(not(feature = "rego"))]
            bail!("Rego policy support is not enabled");
        }

        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .ok_or_else(|| {
                anyhow::anyhow!("policy file has no supported extension: {}", path.display())
            })?;

        match extension.as_str() {
            "yaml" | "yml" => {
                #[cfg(feature = "yaml")]
                {
                    Ok(Some(Self::Yaml(YamlPolicy::load(path)?)))
                }
                #[cfg(not(feature = "yaml"))]
                bail!("YAML policy support is not enabled")
            }
            "rego" => {
                #[cfg(feature = "rego")]
                {
                    Ok(Some(Self::Rego(Box::new(RegoPolicy::load_files(&[
                        path.to_path_buf()
                    ])?))))
                }
                #[cfg(not(feature = "rego"))]
                bail!("Rego policy support is not enabled")
            }
            _ => bail!(
                "unsupported policy format for {}; expected .yaml, .yml, .rego, or a directory",
                path.display()
            ),
        }
    }

    pub fn evaluate(&self, tool: &str, args: &Value) -> PolicyDecision {
        let tool = canonical_tool_name(tool);
        let mut sanitized_args = args.clone();
        crate::tool_args::sanitize_reserved_args(&mut sanitized_args);
        let args = &sanitized_args;

        #[cfg(not(any(feature = "yaml", feature = "rego")))]
        let _ = (tool, args);
        match self {
            #[cfg(feature = "yaml")]
            Self::Yaml(policy) => policy.evaluate(tool, args),
            #[cfg(feature = "rego")]
            Self::Rego(policy) => policy.evaluate(tool, args),
            #[cfg(not(any(feature = "yaml", feature = "rego")))]
            Self::Disabled => PolicyDecision::Error("policy support is not enabled".to_owned()),
        }
    }
}

fn canonical_tool_name(tool: &str) -> &str {
    match tool {
        "type_text_chars" => "type_text",
        other => other,
    }
}

#[derive(Debug, Clone)]
struct ConfiguredPolicyLayer {
    engine: PolicyEngine,
    sha256: String,
}

static CONFIGURED_POLICY: OnceLock<Result<Option<ConfiguredPolicyLayer>, String>> = OnceLock::new();
static CONFIGURED_MANAGED_POLICY: OnceLock<Result<Option<ConfiguredPolicyLayer>, String>> =
    OnceLock::new();

fn policy_files(path: &Path) -> Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }
    let mut files = Vec::new();
    for entry in std::fs::read_dir(path)
        .with_context(|| format!("failed to read policy directory {}", path.display()))?
    {
        let entry = entry.with_context(|| {
            format!(
                "failed to read an entry in policy directory {}",
                path.display()
            )
        })?;
        let entry_path = entry.path();
        if entry_path.is_file()
            && entry_path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("rego"))
        {
            files.push(entry_path);
        }
    }
    files.sort();
    Ok(files)
}

fn policy_sha256(path: &Path) -> Result<String> {
    let files = policy_files(path)?;
    let mut digest = Sha256::new();
    digest.update(b"cua-driver-policy-v1\0");
    for file in files {
        let name = file
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("policy");
        let bytes = std::fs::read(&file)
            .with_context(|| format!("failed to hash policy {}", file.display()))?;
        digest.update((name.len() as u64).to_be_bytes());
        digest.update(name.as_bytes());
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(bytes);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn load_configured_layer(env_name: &str) -> Result<Option<ConfiguredPolicyLayer>, String> {
    let Some(path_os) = std::env::var_os(env_name) else {
        return Ok(None);
    };
    let path = Path::new(&path_os);
    if !path.exists() {
        return Err(format!(
            "configured permission policy path does not exist: {}",
            path.display()
        ));
    }
    let hash_before_load = policy_sha256(path).map_err(|error| error.to_string())?;
    let engine = PolicyEngine::load(path).map_err(|error| error.to_string())?;
    let Some(engine) = engine else {
        return Ok(None);
    };
    let sha256 = policy_sha256(path).map_err(|error| error.to_string())?;
    if hash_before_load != sha256 {
        return Err(format!(
            "configured permission policy changed while it was being loaded: {}",
            path.display()
        ));
    }
    Ok(Some(ConfiguredPolicyLayer { engine, sha256 }))
}

/// Return the process-wide policy configured through [`POLICY_FILE_ENV`].
/// The file is loaded once so stdio and HTTP requests use the same immutable
/// policy snapshot for the lifetime of the driver process.
pub fn configured_policy() -> Result<Option<&'static PolicyEngine>, String> {
    match CONFIGURED_POLICY.get_or_init(|| load_configured_layer(POLICY_FILE_ENV)) {
        Ok(policy) => Ok(policy.as_ref().map(|layer| &layer.engine)),
        Err(error) => Err(error.clone()),
    }
}

/// Return the immutable administrator-owned ceiling. A user policy is always
/// intersected with this layer and can never widen it.
pub fn configured_managed_policy() -> Result<Option<&'static PolicyEngine>, String> {
    match CONFIGURED_MANAGED_POLICY.get_or_init(|| load_configured_layer(MANAGED_POLICY_FILE_ENV)) {
        Ok(policy) => Ok(policy.as_ref().map(|layer| &layer.engine)),
        Err(error) => Err(error.clone()),
    }
}

fn layer_sha256(
    layer: &'static OnceLock<Result<Option<ConfiguredPolicyLayer>, String>>,
    env_name: &str,
) -> Result<Option<String>, String> {
    match layer.get_or_init(|| load_configured_layer(env_name)) {
        Ok(policy) => Ok(policy.as_ref().map(|configured| configured.sha256.clone())),
        Err(error) => Err(error.clone()),
    }
}

pub fn user_policy_sha256() -> Result<Option<String>, String> {
    layer_sha256(&CONFIGURED_POLICY, POLICY_FILE_ENV)
}

pub fn managed_policy_sha256() -> Result<Option<String>, String> {
    layer_sha256(&CONFIGURED_MANAGED_POLICY, MANAGED_POLICY_FILE_ENV)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthorizationError {
    #[error("Permission denied: {0}")]
    Denied(String),
    #[error("Policy evaluation error: {0}")]
    Evaluation(String),
    #[error("Policy loading error: {0}")]
    Loading(String),
}

fn authorize_policy_layers<'a>(
    tool: &str,
    args: &Value,
    layers: impl IntoIterator<Item = (&'a str, &'a PolicyEngine)>,
) -> Result<(), AuthorizationError> {
    for (name, policy) in layers {
        match policy.evaluate(tool, args) {
            PolicyDecision::Allow => {}
            PolicyDecision::Deny(reason) => {
                return Err(AuthorizationError::Denied(format!(
                    "{name} policy: {reason}"
                )))
            }
            PolicyDecision::Error(message) => {
                return Err(AuthorizationError::Evaluation(format!(
                    "{name} policy: {message}"
                )))
            }
        }
    }
    Ok(())
}

/// Canonical process-wide capability-policy decision for one tool call.
///
/// Every daemon dispatch path must call this function immediately before
/// registry lookup/invocation. A proxy may also call it as an early denial,
/// but the daemon remains authoritative.
pub fn authorize_tool_call(tool: &str, args: &Value) -> Result<(), AuthorizationError> {
    let managed = configured_managed_policy()
        .map_err(|message| AuthorizationError::Loading(format!("managed policy: {message}")))?;
    let user = configured_policy()
        .map_err(|message| AuthorizationError::Loading(format!("user policy: {message}")))?;
    let mut layers = Vec::with_capacity(2);
    if let Some(policy) = managed {
        layers.push(("managed", policy));
    }
    if let Some(policy) = user {
        layers.push(("user", policy));
    }
    authorize_policy_layers(tool, args, layers)
}

/// Eagerly validate the immutable process policy before any action endpoint is
/// exposed. This prevents a configured typo from producing a listening daemon
/// that only discovers the error after clients begin issuing calls.
pub fn validate_configured_policy() -> Result<(), AuthorizationError> {
    configured_managed_policy()
        .map_err(|message| AuthorizationError::Loading(format!("managed policy: {message}")))?;
    configured_policy()
        .map_err(|message| AuthorizationError::Loading(format!("user policy: {message}")))?;
    Ok(())
}

#[cfg(feature = "yaml")]
#[derive(Debug, Clone)]
pub struct YamlPolicy {
    allowed_tools: Vec<String>,
    rules: Vec<YamlRule>,
    denied_tools: Vec<String>,
}

#[cfg(feature = "yaml")]
impl YamlPolicy {
    fn load(path: &Path) -> Result<Self> {
        let source = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read YAML policy {}", path.display()))?;
        Self::from_str(&source)
            .with_context(|| format!("failed to parse YAML policy {}", path.display()))
    }

    fn from_str(source: &str) -> Result<Self> {
        let document: RawYamlPolicy = serde_yaml_ng::from_str(source)?;
        let (allowed_tools, raw_rules) = document.allow.into_parts();
        let denied_tools = document.deny.into_tools();
        for tool in allowed_tools.iter().chain(&denied_tools) {
            if tool.trim().is_empty() {
                bail!("YAML tool lists cannot contain an empty tool name");
            }
        }
        let rules = raw_rules
            .into_iter()
            .map(YamlRule::compile)
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            allowed_tools,
            rules,
            denied_tools,
        })
    }

    fn evaluate(&self, tool: &str, args: &Value) -> PolicyDecision {
        if self.denied_tools.iter().any(|denied| denied == tool) {
            return PolicyDecision::Deny(format!("tool '{tool}' is explicitly denied"));
        }
        if self.allowed_tools.iter().any(|allowed| allowed == tool) {
            return PolicyDecision::Allow;
        }

        let matching_rules: Vec<&YamlRule> =
            self.rules.iter().filter(|rule| rule.tool == tool).collect();
        if matching_rules.is_empty() {
            return PolicyDecision::Deny(format!(
                "tool '{tool}' is not allowed by the YAML policy"
            ));
        }

        let mut failures = Vec::new();
        for rule in matching_rules {
            match rule.matches(args) {
                Ok(()) => return PolicyDecision::Allow,
                Err(reason) => failures.push(reason),
            }
        }
        PolicyDecision::Deny(format!(
            "tool '{tool}' argument constraints were not satisfied: {}",
            failures.join("; ")
        ))
    }
}

#[cfg(feature = "yaml")]
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawYamlPolicy {
    #[serde(default)]
    allow: RawAllow,
    #[serde(default)]
    deny: RawDeny,
}

#[cfg(feature = "yaml")]
#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
enum RawAllow {
    Detailed(RawAllowDetailed),
    Entries(Vec<RawAllowEntry>),
}

#[cfg(feature = "yaml")]
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAllowDetailed {
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    rules: Vec<RawYamlRule>,
}

#[cfg(feature = "yaml")]
impl Default for RawAllow {
    fn default() -> Self {
        Self::Detailed(RawAllowDetailed {
            tools: Vec::new(),
            rules: Vec::new(),
        })
    }
}

#[cfg(feature = "yaml")]
impl RawAllow {
    fn into_parts(self) -> (Vec<String>, Vec<RawYamlRule>) {
        match self {
            Self::Detailed(section) => (section.tools, section.rules),
            Self::Entries(entries) => {
                let mut tools = Vec::new();
                let mut rules = Vec::new();
                for entry in entries {
                    match entry {
                        RawAllowEntry::Tool(tool) => tools.push(tool),
                        RawAllowEntry::Rule(rule) => rules.push(rule),
                    }
                }
                (tools, rules)
            }
        }
    }
}

#[cfg(feature = "yaml")]
#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
enum RawAllowEntry {
    Tool(String),
    Rule(RawYamlRule),
}

#[cfg(feature = "yaml")]
#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
enum RawDeny {
    Detailed(RawDenyDetailed),
    Tools(Vec<String>),
}

#[cfg(feature = "yaml")]
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDenyDetailed {
    #[serde(default)]
    tools: Vec<String>,
}

#[cfg(feature = "yaml")]
impl Default for RawDeny {
    fn default() -> Self {
        Self::Detailed(RawDenyDetailed { tools: Vec::new() })
    }
}

#[cfg(feature = "yaml")]
impl RawDeny {
    fn into_tools(self) -> Vec<String> {
        match self {
            Self::Detailed(section) => section.tools,
            Self::Tools(tools) => tools,
        }
    }
}

#[cfg(feature = "yaml")]
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawYamlRule {
    tool: String,
    #[serde(default)]
    constraints: std::collections::BTreeMap<String, RawConstraint>,
}

#[cfg(feature = "yaml")]
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConstraint {
    min: Option<f64>,
    max: Option<f64>,
    max_length: Option<usize>,
    pattern: Option<String>,
    allowed: Option<Vec<Value>>,
}

#[cfg(feature = "yaml")]
#[derive(Debug, Clone)]
struct YamlRule {
    tool: String,
    constraints: std::collections::BTreeMap<String, Constraint>,
}

#[cfg(feature = "yaml")]
impl YamlRule {
    fn compile(raw: RawYamlRule) -> Result<Self> {
        if raw.tool.trim().is_empty() {
            bail!("YAML allow rule has an empty tool name");
        }
        let constraints = raw
            .constraints
            .into_iter()
            .map(|(argument, constraint)| {
                Constraint::compile(constraint)
                    .with_context(|| format!("invalid constraint for '{}.{}'", raw.tool, argument))
                    .map(|constraint| (argument, constraint))
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            tool: raw.tool,
            constraints,
        })
    }

    fn matches(&self, args: &Value) -> Result<(), String> {
        let object = args
            .as_object()
            .ok_or_else(|| "arguments must be a JSON object".to_owned())?;
        for (argument, constraint) in &self.constraints {
            let value = object
                .get(argument)
                .ok_or_else(|| format!("missing required argument '{argument}'"))?;
            constraint.matches(argument, value)?;
        }
        Ok(())
    }
}

#[cfg(feature = "yaml")]
#[derive(Debug, Clone)]
struct Constraint {
    min: Option<f64>,
    max: Option<f64>,
    max_length: Option<usize>,
    pattern: Option<regex::Regex>,
    allowed: Option<Vec<Value>>,
}

#[cfg(feature = "yaml")]
impl Constraint {
    fn compile(raw: RawConstraint) -> Result<Self> {
        if raw.min.is_none()
            && raw.max.is_none()
            && raw.max_length.is_none()
            && raw.pattern.is_none()
            && raw.allowed.is_none()
        {
            bail!("constraint must define at least one check");
        }
        if let (Some(min), Some(max)) = (raw.min, raw.max) {
            if min > max {
                bail!("min cannot be greater than max");
            }
        }
        let pattern = raw
            .pattern
            .map(|pattern| regex::Regex::new(&pattern))
            .transpose()
            .context("invalid regex pattern")?;
        Ok(Self {
            min: raw.min,
            max: raw.max,
            max_length: raw.max_length,
            pattern,
            allowed: raw.allowed,
        })
    }

    fn matches(&self, argument: &str, value: &Value) -> Result<(), String> {
        if self.min.is_some() || self.max.is_some() {
            let number = value
                .as_f64()
                .ok_or_else(|| format!("argument '{argument}' must be a number"))?;
            if let Some(min) = self.min {
                if number < min {
                    return Err(format!("argument '{argument}' must be >= {min}"));
                }
            }
            if let Some(max) = self.max {
                if number > max {
                    return Err(format!("argument '{argument}' must be <= {max}"));
                }
            }
        }
        if self.max_length.is_some() || self.pattern.is_some() {
            let text = value
                .as_str()
                .ok_or_else(|| format!("argument '{argument}' must be a string"))?;
            if let Some(max_length) = self.max_length {
                if text.chars().count() > max_length {
                    return Err(format!(
                        "argument '{argument}' must be at most {max_length} characters"
                    ));
                }
            }
            if let Some(pattern) = &self.pattern {
                if !pattern.is_match(text) {
                    return Err(format!(
                        "argument '{argument}' does not match the required pattern"
                    ));
                }
            }
        }
        if let Some(allowed) = &self.allowed {
            if !allowed.contains(value) {
                return Err(format!(
                    "argument '{argument}' is not in the allowed values"
                ));
            }
        }
        Ok(())
    }
}

#[cfg(feature = "rego")]
#[derive(Debug, Clone)]
pub struct RegoPolicy {
    engine: regorus::Engine,
}

#[cfg(feature = "rego")]
impl RegoPolicy {
    fn load_directory(path: &Path) -> Result<Self> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(path)
            .with_context(|| format!("failed to read Rego policy directory {}", path.display()))?
        {
            let entry = entry.with_context(|| {
                format!(
                    "failed to read an entry in Rego policy directory {}",
                    path.display()
                )
            })?;
            let entry_path = entry.path();
            if entry_path.is_file()
                && entry_path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("rego"))
            {
                files.push(entry_path);
            }
        }
        files.sort();
        if files.is_empty() {
            bail!(
                "Rego policy directory {} contains no .rego files",
                path.display()
            );
        }
        Self::load_files(&files)
    }

    fn load_files(paths: &[PathBuf]) -> Result<Self> {
        let mut engine = regorus::Engine::new();
        for path in paths {
            engine
                .add_policy_from_file(path)
                .with_context(|| format!("failed to load Rego policy {}", path.display()))?;
        }
        engine.set_input_json(r#"{"server":"cua-driver","tool":"","arguments":{}}"#)?;
        match engine
            .eval_rule(REGO_ALLOW_RULE.to_owned())
            .context("failed to validate data.cua.policy.allow")?
        {
            regorus::Value::Bool(_) | regorus::Value::Undefined => {}
            value => bail!("{REGO_ALLOW_RULE} must evaluate to a boolean, got {value:?}"),
        }
        Ok(Self { engine })
    }

    fn evaluate(&self, tool: &str, args: &Value) -> PolicyDecision {
        let input = serde_json::json!({
            "server": "cua-driver",
            "tool": tool,
            "arguments": args,
        });
        let input_json = match serde_json::to_string(&input) {
            Ok(input) => input,
            Err(error) => return PolicyDecision::Error(error.to_string()),
        };
        let mut engine = self.engine.clone();
        if let Err(error) = engine.set_input_json(&input_json) {
            return PolicyDecision::Error(error.to_string());
        }
        match engine.eval_rule(REGO_ALLOW_RULE.to_owned()) {
            Ok(regorus::Value::Bool(true)) => PolicyDecision::Allow,
            Ok(regorus::Value::Bool(false) | regorus::Value::Undefined) => {
                PolicyDecision::Deny(format!("tool '{tool}' is not allowed by the Rego policy"))
            }
            Ok(value) => PolicyDecision::Error(format!(
                "{REGO_ALLOW_RULE} must evaluate to a boolean, got {value:?}"
            )),
            Err(error) => PolicyDecision::Error(error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(any(feature = "yaml", feature = "rego"))]
    use std::fs;
    use tempfile::tempdir;

    #[cfg(feature = "yaml")]
    fn yaml_policy(source: &str) -> PolicyEngine {
        PolicyEngine::Yaml(YamlPolicy::from_str(source).unwrap())
    }

    #[cfg(feature = "yaml")]
    #[test]
    fn yaml_parses_and_evaluates_tool_lists() {
        let policy = yaml_policy(
            r#"
allow:
  tools: [screenshot, wait]
deny:
  tools: [shell_execute]
"#,
        );
        assert_eq!(
            policy.evaluate("screenshot", &serde_json::json!({})),
            PolicyDecision::Allow
        );
        assert!(matches!(
            policy.evaluate("shell_execute", &serde_json::json!({})),
            PolicyDecision::Deny(reason) if reason.contains("explicitly denied")
        ));
    }

    #[cfg(feature = "yaml")]
    #[test]
    fn yaml_is_deny_by_default() {
        let policy = yaml_policy("allow:\n  tools: [screenshot]\n");
        assert!(matches!(
            policy.evaluate("click", &serde_json::json!({"x": 10, "y": 10})),
            PolicyDecision::Deny(_)
        ));
    }

    #[cfg(feature = "yaml")]
    #[test]
    fn managed_and_user_layers_are_intersected() {
        let managed = yaml_policy("allow:\n  tools: [click, wait]\n");
        let user = yaml_policy("allow:\n  tools: [wait]\n");

        authorize_policy_layers(
            "wait",
            &serde_json::json!({}),
            [("managed", &managed), ("user", &user)],
        )
        .unwrap();

        let denied = authorize_policy_layers(
            "click",
            &serde_json::json!({"x": 1, "y": 2}),
            [("managed", &managed), ("user", &user)],
        )
        .expect_err("user layer must be able to narrow managed allow");
        assert!(denied.to_string().contains("user policy"));
    }

    #[cfg(feature = "yaml")]
    #[test]
    fn policy_hash_is_content_bound_and_stable() {
        let dir = tempdir().unwrap();
        let first = dir.path().join("first.yaml");
        let second = dir.path().join("second.yaml");
        fs::write(&first, "allow:\n  tools: [wait]\n").unwrap();
        fs::write(&second, "allow:\n  tools: [wait]\n").unwrap();
        assert_eq!(
            policy_sha256(&first).unwrap(),
            policy_sha256(&first).unwrap()
        );
        assert_ne!(
            policy_sha256(&first).unwrap(),
            policy_sha256(&second).unwrap(),
            "file identity is included so separately managed sources have distinct provenance"
        );
        fs::write(&first, "allow:\n  tools: [click]\n").unwrap();
        assert_ne!(
            policy_sha256(&first).unwrap(),
            policy_sha256(&second).unwrap()
        );
    }

    #[cfg(feature = "yaml")]
    #[test]
    fn yaml_enforces_numeric_string_and_allowed_constraints() {
        let policy = yaml_policy(
            r#"
allow:
  rules:
    - tool: click
      constraints:
        x: { min: 0, max: 1920 }
        y: { min: 0, max: 1080 }
    - tool: type
      constraints:
        text: { max_length: 5, pattern: "^[a-z]+$" }
    - tool: launch_app
      constraints:
        app:
          allowed: [Calculator, TextEdit]
"#,
        );
        assert_eq!(
            policy.evaluate("click", &serde_json::json!({"x": 1920, "y": 0})),
            PolicyDecision::Allow
        );
        assert!(matches!(
            policy.evaluate("click", &serde_json::json!({"x": 1921, "y": 0})),
            PolicyDecision::Deny(_)
        ));
        assert_eq!(
            policy.evaluate("type", &serde_json::json!({"text": "hello"})),
            PolicyDecision::Allow
        );
        assert!(matches!(
            policy.evaluate("type", &serde_json::json!({"text": "hello!"})),
            PolicyDecision::Deny(_)
        ));
        assert_eq!(
            policy.evaluate("launch_app", &serde_json::json!({"app": "Calculator"})),
            PolicyDecision::Allow
        );
        assert!(matches!(
            policy.evaluate("launch_app", &serde_json::json!({"app": "Terminal"})),
            PolicyDecision::Deny(_)
        ));
    }

    #[cfg(feature = "yaml")]
    #[test]
    fn explicit_deny_overrides_allow() {
        let policy = yaml_policy("allow:\n  tools: [screenshot]\ndeny:\n  tools: [screenshot]\n");
        assert!(matches!(
            policy.evaluate("screenshot", &serde_json::json!({})),
            PolicyDecision::Deny(_)
        ));
    }

    #[cfg(feature = "yaml")]
    #[test]
    fn empty_tool_names_fail_at_load_time() {
        let error = YamlPolicy::from_str("allow:\n  tools: ['']\n").unwrap_err();
        assert!(error.to_string().contains("empty tool name"));
    }

    #[cfg(feature = "yaml")]
    #[test]
    fn invalid_patterns_fail_at_load_time() {
        let error = YamlPolicy::from_str(
            "allow:\n  rules:\n    - tool: type\n      constraints:\n        text: { pattern: '[' }\n",
        )
        .unwrap_err();
        assert!(error.to_string().contains("invalid constraint"));
    }

    #[cfg(feature = "rego")]
    #[test]
    fn rego_evaluates_allow_and_constraints() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("policy.rego");
        fs::write(
            &path,
            r#"
package cua.policy

default allow = false

allow if {
    input.server == "cua-driver"
    input.tool == "screenshot"
}

allow if {
    input.tool == "click"
    input.arguments.x >= 0
    input.arguments.x <= 1920
    input.arguments.y >= 0
    input.arguments.y <= 1080
}
"#,
        )
        .unwrap();
        let policy = PolicyEngine::load(&path).unwrap().unwrap();
        assert_eq!(
            policy.evaluate("screenshot", &serde_json::json!({})),
            PolicyDecision::Allow
        );
        assert_eq!(
            policy.evaluate("click", &serde_json::json!({"x": 20, "y": 30})),
            PolicyDecision::Allow
        );
        assert!(matches!(
            policy.evaluate("click", &serde_json::json!({"x": 2000, "y": 30})),
            PolicyDecision::Deny(_)
        ));
        assert!(matches!(
            policy.evaluate("shell_execute", &serde_json::json!({})),
            PolicyDecision::Deny(_)
        ));
    }

    #[cfg(feature = "rego")]
    #[test]
    fn rego_directory_loads_all_policy_files() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("base.rego"),
            "package cua.policy\ndefault allow = false\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("screenshot.rego"),
            "package cua.policy\nallow if input.tool == \"screenshot\"\n",
        )
        .unwrap();
        let policy = PolicyEngine::load(dir.path()).unwrap().unwrap();
        assert_eq!(
            policy.evaluate("screenshot", &serde_json::json!({})),
            PolicyDecision::Allow
        );
    }

    #[cfg(feature = "yaml")]
    #[test]
    fn policy_uses_canonical_tool_names() {
        let policy = yaml_policy("allow:\n  tools: [type_text]\n");
        assert_eq!(
            policy.evaluate("type_text_chars", &serde_json::json!({"text": "hello"})),
            PolicyDecision::Allow
        );
    }

    #[cfg(feature = "rego")]
    #[test]
    fn rego_does_not_receive_internal_transport_arguments() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("policy.rego");
        fs::write(
            &path,
            r#"
package cua.policy

default allow = false

allow if {
    input.tool == "click"
    object.keys(input.arguments) == {"x"}
}
"#,
        )
        .unwrap();
        let policy = PolicyEngine::load(&path).unwrap().unwrap();
        assert_eq!(
            policy.evaluate(
                "click",
                &serde_json::json!({
                    "x": 10,
                    "_session_id": "public-session",
                    "_transport_session_id": "transport-session",
                    "_cua_browser_prepare_mcp_host_approved": true,
                })
            ),
            PolicyDecision::Allow
        );
    }

    #[test]
    fn explicitly_configured_missing_policy_fails_closed() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("missing.yaml");
        let error = PolicyEngine::load(&missing).unwrap_err();
        assert!(error
            .to_string()
            .contains("configured permission policy path does not exist"));
    }
}
