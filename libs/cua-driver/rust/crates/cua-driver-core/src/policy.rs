//! Optional permission-policy enforcement for MCP tool calls.

use std::path::Path;
#[cfg(feature = "rego")]
use std::path::PathBuf;
use std::sync::OnceLock;

#[cfg(any(feature = "yaml", feature = "rego"))]
use anyhow::Context;
use anyhow::{bail, Result};
use serde_json::Value;

pub const POLICY_FILE_ENV: &str = "CUA_DRIVER_POLICY_FILE";
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
    /// Load a policy file or Rego policy directory. A missing path disables
    /// enforcement to preserve the driver's behavior when no policy is present.
    pub fn load(path: &Path) -> Result<Option<Self>> {
        if !path.exists() {
            return Ok(None);
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
        let mut sanitized_args;
        let args = if args.get("_session_id").is_some() {
            sanitized_args = args.clone();
            if let Some(object) = sanitized_args.as_object_mut() {
                object.remove("_session_id");
            }
            &sanitized_args
        } else {
            args
        };

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

static CONFIGURED_POLICY: OnceLock<Result<Option<PolicyEngine>, String>> = OnceLock::new();

/// Return the process-wide policy configured through [`POLICY_FILE_ENV`].
/// The file is loaded once so stdio and HTTP requests use the same immutable
/// policy snapshot for the lifetime of the driver process.
pub fn configured_policy() -> Result<Option<&'static PolicyEngine>, String> {
    match CONFIGURED_POLICY.get_or_init(|| {
        let Some(path_os) = std::env::var_os(POLICY_FILE_ENV) else {
            return Ok(None);
        };
        let path = Path::new(&path_os);
        if !path.exists() {
            tracing::warn!(
                path = %path.display(),
                "{POLICY_FILE_ENV} is set but the path does not exist; \
                 policy enforcement is disabled"
            );
            return Ok(None);
        }
        PolicyEngine::load(path).map_err(|error| error.to_string())
    }) {
        Ok(policy) => Ok(policy.as_ref()),
        Err(error) => Err(error.clone()),
    }
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
    fn rego_does_not_receive_internal_session_arguments() {
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
                &serde_json::json!({"x": 10, "_session_id": "internal"})
            ),
            PolicyDecision::Allow
        );
    }

    #[test]
    fn missing_policy_file_disables_enforcement() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("missing.yaml");
        assert!(PolicyEngine::load(&missing).unwrap().is_none());
    }
}
