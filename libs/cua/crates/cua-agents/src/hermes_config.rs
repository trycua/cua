//! Update connection-owned routing without replacing Hermes user settings.

use serde_yaml_ng::Value;
use yaml_edit::{AnchorRegistry, AsYaml, Mapping, MappingMergedExt, YamlFile, YamlNode};

fn invalid(path: &str, detail: impl std::fmt::Display) -> crate::Error {
    crate::Error::Invalid(format!("cannot update Hermes config {path}: {detail}"))
}

fn parse(path: &str, text: &str) -> crate::Result<(YamlFile, Mapping, AnchorRegistry)> {
    // Serde rejects duplicate keys and multiple documents. Do not serialize
    // through it: Hermes's PyYAML loader has different scalar resolution rules.
    let mut value: Value =
        serde_yaml_ng::from_str(text).map_err(|e| invalid(path, format!("invalid YAML: {e}")))?;
    if !matches!(value, Value::Mapping(_) | Value::Null) {
        return Err(invalid(path, "root must be a mapping"));
    }
    value
        .apply_merge()
        .map_err(|e| invalid(path, format!("invalid YAML merge: {e}")))?;
    let mut file: YamlFile = text
        .parse()
        .map_err(|e| invalid(path, format!("invalid YAML: {e}")))?;
    if file.ensure_document().as_mapping().is_none() {
        file = YamlFile::new();
    }
    let doc = file.ensure_document();
    let root = doc.as_mapping().expect("empty file has a mapping");
    let anchors = AnchorRegistry::from_document(&doc);
    Ok((file, root, anchors))
}

fn get(
    path: &str,
    map: &Mapping,
    anchors: &AnchorRegistry,
    key: &str,
) -> crate::Result<Option<YamlNode>> {
    Ok(map
        .merged(anchors)
        .map_err(|e| invalid(path, format!("invalid YAML merge: {e}")))?
        .get(key))
}

fn is_string(node: &YamlNode) -> bool {
    node.as_scalar().is_some()
        && matches!(
            serde_yaml_ng::from_str::<Value>(&node.to_string()),
            Ok(Value::String(_))
        )
}

fn section(
    path: &str,
    root: &Mapping,
    anchors: &AnchorRegistry,
    name: &str,
) -> crate::Result<Option<Mapping>> {
    match get(path, root, anchors, name)? {
        Some(YamlNode::Mapping(map)) => Ok(Some(map)),
        Some(node) if node.as_scalar().is_some_and(|s| s.is_null()) => Ok(None),
        // The legacy model-name scalar contains only routing.
        Some(node) if name == "model" && is_string(&node) => Ok(None),
        None => Ok(None),
        _ => Err(invalid(path, format!("{name} must be a mapping"))),
    }
}

fn editable_section(path: &str, root: &Mapping, name: &str) -> crate::Result<Mapping> {
    let anchors = current_anchors(root);
    editable_mapping(
        path,
        root,
        &anchors,
        name,
        section(path, root, &anchors, name)?,
    )
}

fn current_anchors(map: &Mapping) -> AnchorRegistry {
    // AnchorRegistry stores byte offsets. An earlier edit can move an alias,
    // so resolve against the current document, not a registry from before edits.
    let root = map
        .as_node()
        .expect("mapping has a syntax node")
        .ancestors()
        .last()
        .expect("mapping has a root");
    AnchorRegistry::from_tree(&root)
}

fn editable_mapping(
    path: &str,
    root: &Mapping,
    anchors: &AnchorRegistry,
    key: impl AsYaml,
    resolved: Option<Mapping>,
) -> crate::Result<Mapping> {
    if let Some(direct) = root.get_mapping(&key)
        && !direct.contains_key("<<")
    {
        return Ok(direct);
    }
    // Copy inherited fields before editing so removing stale routing cannot
    // expose it again through a merge key, or mutate the source anchor.
    let copy = Mapping::new();
    if let Some(resolved) = resolved {
        for (key, value) in resolved
            .merged(anchors)
            .map_err(|e| invalid(path, format!("invalid YAML merge: {e}")))?
            .iter()
        {
            copy.set(key, value);
        }
    }
    root.set(&key, copy);
    Ok(root.get_mapping(&key).expect("mapping just inserted"))
}

fn same_value(left: &YamlNode, right: &YamlNode) -> bool {
    // Avoid YAML 1.2 equality: quotes can change the type in Hermes's YAML 1.1
    // loader. Compare exact syntax first, then only unambiguous string values
    // and collections of those values. Uncertain scalar forms are replaced.
    let left_text = left.to_string();
    let right_text = right.to_string();
    if left_text == right_text {
        return true;
    }
    match (left, right) {
        (YamlNode::Scalar(a), YamlNode::Scalar(b)) => {
            let unambiguous = |scalar: &yaml_edit::Scalar| {
                scalar.is_quoted() || scalar.is_block() || {
                    let value = scalar.as_string();
                    value.starts_with(|c: char| c.is_ascii_alphabetic())
                        && !matches!(
                            value.to_ascii_lowercase().as_str(),
                            "y" | "n" | "yes" | "no" | "on" | "off" | "true" | "false" | "null"
                        )
                }
            };
            unambiguous(a)
                && unambiguous(b)
                && matches!((serde_yaml_ng::from_str::<Value>(&left_text), serde_yaml_ng::from_str::<Value>(&right_text)), (Ok(Value::String(a)), Ok(Value::String(b))) if a == b)
        }
        (YamlNode::Sequence(a), YamlNode::Sequence(b)) => {
            a.len() == b.len() && a.values().zip(b.values()).all(|(a, b)| same_value(&a, &b))
        }
        (YamlNode::Mapping(a), YamlNode::Mapping(b)) => {
            a.len() == b.len()
                && a.iter().all(|(ak, av)| {
                    b.iter()
                        .any(|(bk, bv)| same_value(&ak, &bk) && same_value(&av, &bv))
                })
        }
        _ => false,
    }
}

fn set(saved: &Mapping, key: impl AsYaml, value: YamlNode) {
    if saved.get(&key).is_none_or(|old| !same_value(&old, &value)) {
        // Copy syntax nodes, including scalar quotes, rather than decoded values.
        saved.set(key, value);
    }
}

fn apply(
    path: &str,
    saved: &Mapping,
    key: YamlNode,
    value: YamlNode,
    supplied_anchors: &AnchorRegistry,
) -> crate::Result<()> {
    let saved_anchors = current_anchors(saved);
    let old = saved
        .merged(&saved_anchors)
        .map_err(|e| invalid(path, format!("invalid YAML merge: {e}")))?
        .get(&key);
    if let (Some(old), Some(new)) = (
        old.and_then(|n| n.as_mapping().cloned()),
        value.as_mapping(),
    ) {
        let target = editable_mapping(path, saved, &saved_anchors, &key, Some(old))?;
        overlay(path, &target, new, supplied_anchors)?;
    } else {
        set(saved, key, value);
    }
    Ok(())
}

fn overlay(
    path: &str,
    saved: &Mapping,
    supplied: &Mapping,
    supplied_anchors: &AnchorRegistry,
) -> crate::Result<()> {
    for (key, value) in supplied
        .merged(supplied_anchors)
        .map_err(|e| invalid(path, format!("invalid YAML merge: {e}")))?
        .iter()
    {
        apply(path, saved, key, value, supplied_anchors)?;
    }
    Ok(())
}

/// Merge only a persistent Hermes config. Keep scalar spelling because Hermes
/// reads YAML 1.1: `"off"` and `off`, for example, have different meanings.
/// Existing context overrides remain; routing, fallbacks and external-login
/// policy come from Cua. Invalid input is refused before the file is replaced.
pub(crate) fn merge(path: &str, existing: &[u8], generated: &str) -> crate::Result<String> {
    let text =
        std::str::from_utf8(existing).map_err(|e| invalid(path, format!("invalid YAML: {e}")))?;
    let (file, saved, saved_anchors) = parse(path, text)?;
    let (_supplied_file, supplied, supplied_anchors) = parse(path, generated)?;
    section(path, &saved, &saved_anchors, "model")?;
    section(path, &saved, &saved_anchors, "auth")?;
    let updates = section(path, &supplied, &supplied_anchors, "model")?.unwrap_or_default();
    let provider = get(path, &updates, &supplied_anchors, "provider")?;
    if !provider.as_ref().is_some_and(|p| {
        is_string(p)
            && p.as_scalar()
                .is_some_and(|s| !s.as_string().trim().is_empty())
    }) {
        return Err(invalid(
            path,
            "generated model.provider must be a non-empty string",
        ));
    }
    let model = editable_section(path, &saved, "model")?;
    for field in [
        "provider",
        "default",
        "base_url",
        "api_key",
        "api_mode",
        "openai_runtime",
        "key_env",
        "api_key_env",
    ] {
        match get(path, &updates, &supplied_anchors, field)? {
            Some(value) => {
                if !is_string(&value) {
                    return Err(invalid(
                        path,
                        format!("generated model.{field} must be a string"),
                    ));
                }
                set(&model, field, value);
            }
            None => {
                model.remove(field);
            }
        }
    }
    for (key, value) in updates
        .merged(&supplied_anchors)
        .map_err(|e| invalid(path, format!("invalid YAML merge: {e}")))?
        .iter()
    {
        if key
            .as_scalar()
            .is_some_and(|s| s.as_string() == "context_length")
            && model.contains_key("context_length")
        {
            continue;
        }
        apply(path, &model, key, value, &supplied_anchors)?;
    }
    match get(path, &supplied, &supplied_anchors, "fallback_providers")? {
        Some(value) if value.is_sequence() => {
            set(&saved, "fallback_providers", value);
        }
        None => {
            set(
                &saved,
                "fallback_providers",
                YamlNode::Sequence(yaml_edit::Sequence::new()),
            );
        }
        _ => {
            return Err(invalid(
                path,
                "generated fallback_providers must be a sequence",
            ));
        }
    }
    let auth = editable_section(path, &saved, "auth")?;
    if let Some(updates) = section(path, &supplied, &supplied_anchors, "auth")? {
        overlay(path, &auth, &updates, &supplied_anchors)?;
    }
    auth.set("adopt_external_logins", false);
    for (key, value) in supplied
        .merged(&supplied_anchors)
        .map_err(|e| invalid(path, format!("invalid YAML merge: {e}")))?
        .iter()
    {
        if key.as_scalar().is_some_and(|s| {
            matches!(
                s.as_string().as_str(),
                "model" | "auth" | "fallback_providers"
            )
        }) {
            continue;
        }
        apply(path, &saved, key, value, &supplied_anchors)?;
    }
    let result = file.to_string();
    parse(path, &result)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yaml(text: &str) -> Value {
        serde_yaml_ng::from_str(text).unwrap()
    }

    #[test]
    fn updating_connections_preserves_settings_mcp_and_context_override() {
        let existing = r#"
model:
  provider: custom
  default: old-model
  base_url: https://old.example/v1
  api_key: ${OLD_API_KEY}
  context_length: 32768
  temperature: 0.4
fallback_providers:
  - provider: old-provider
    model: old-fallback
auth:
  adopt_external_logins: true
  timeout: 30
mcp_servers:
  documents:
    command: document-server
    args: [--read-only]
terminal:
  backend: local
skills:
  external_dirs: [/work/my-skills]
"#;
        let generated = r#"
model:
  provider: custom
  default: new-model
  base_url: https://new.example/v1
  api_key: ${CUSTOM_ENDPOINT_API_KEY}
  context_length: 131072
fallback_providers:
  - provider: openrouter
    model: fallback-model
auth:
  adopt_external_logins: false
"#;
        let merged = merge("/agent/hermes/config.yaml", existing.as_bytes(), generated).unwrap();
        assert_eq!(
            yaml(&merged),
            yaml(
                r#"
model:
  provider: custom
  default: new-model
  base_url: https://new.example/v1
  api_key: ${CUSTOM_ENDPOINT_API_KEY}
  context_length: 32768
  temperature: 0.4
fallback_providers:
  - provider: openrouter
    model: fallback-model
auth:
  adopt_external_logins: false
  timeout: 30
mcp_servers:
  documents:
    command: document-server
    args: [--read-only]
terminal:
  backend: local
skills:
  external_dirs: [/work/my-skills]
"#
            )
        );
    }

    #[test]
    fn switching_provider_clears_old_endpoint_key_model_and_fallbacks() {
        let existing = b"model:\n  provider: custom\n  default: old\n  base_url: https://old\n  api_key: old-key\n  api_mode: responses\n  openai_runtime: codex_app_server\n  key_env: OLD_API_KEY\n  api_key_env: OTHER_API_KEY\n  temperature: 0.4\nfallback_providers:\n  - provider: old\n    model: fallback\n";
        let merged = merge(
            "/agent/hermes/config.yaml",
            existing,
            "model:\n  provider: openrouter\n",
        )
        .unwrap();
        assert_eq!(
            yaml(&merged),
            yaml(
                "model:\n  provider: openrouter\n  temperature: 0.4\nfallback_providers: []\nauth:\n  adopt_external_logins: false\n"
            )
        );
    }

    #[test]
    fn legacy_scalar_model_is_replaced_without_losing_user_settings() {
        let existing = b"model: 'old-provider/old-model'\nterminal:\n  backend: local\nmcp_servers:\n  docs:\n    command: document-server\n";
        let merged = merge(
            "/agent/hermes/config.yaml",
            existing,
            "model:\n  provider: openrouter\n  default: new-model\n",
        )
        .unwrap();
        assert_eq!(
            yaml(&merged),
            yaml(
                "model:\n  provider: openrouter\n  default: new-model\nterminal:\n  backend: local\nmcp_servers:\n  docs:\n    command: document-server\nfallback_providers: []\nauth:\n  adopt_external_logins: false\n"
            )
        );
    }

    #[test]
    fn new_endpoint_config_supplies_default_context_length() {
        let endpoint = crate::harness::Endpoint {
            base_url: "https://endpoint.example".into(),
            wire: None,
            model: Some("model".into()),
        };
        let launch = crate::harness::launch_with_config(
            crate::harness::harness("hermes").unwrap(),
            Some(&endpoint),
            None,
            "/run/1",
            "/work",
            Some("/agent"),
            None,
        )
        .unwrap();
        let generated = &launch.files[0];
        let merged = merge(&generated.rel, b"", &generated.body).unwrap();
        assert_eq!(
            yaml(&merged),
            yaml(
                "model:\n  provider: custom\n  default: model\n  base_url: https://endpoint.example/v1\n  api_key: ${OPENAI_API_KEY}\n  context_length: 131072\nauth:\n  adopt_external_logins: false\nfallback_providers: []\n"
            )
        );
    }

    #[test]
    fn inherited_user_settings_survive_explicit_connection_routing() {
        let existing = b"defaults: &defaults\n  model:\n    provider: custom\n    temperature: 0.4\n  auth:\n    timeout: 30\n<<: *defaults\n";
        let merged = yaml(
            &merge(
                "/agent/hermes/config.yaml",
                existing,
                "model:\n  provider: openrouter\n",
            )
            .unwrap(),
        );
        assert_eq!(
            merged["model"],
            yaml("provider: openrouter\ntemperature: 0.4\n")
        );
        assert_eq!(
            merged["auth"],
            yaml("adopt_external_logins: false\ntimeout: 30\n")
        );
        assert_eq!(
            merged["defaults"],
            yaml("model:\n  provider: custom\n  temperature: 0.4\nauth:\n  timeout: 30\n")
        );
    }

    #[test]
    fn invalid_existing_config_is_refused_with_its_path() {
        for (existing, detail) in [
            ("model: [", "invalid YAML"),
            ("- unexpected-root", "root must be a mapping"),
            ("model: [old-model]", "model must be a mapping"),
            ("auth: true", "auth must be a mapping"),
        ] {
            let err = merge(
                "/agent/hermes/config.yaml",
                existing.as_bytes(),
                "model:\n  provider: openrouter\n",
            )
            .unwrap_err();
            assert!(matches!(&err, crate::Error::Invalid(_)), "{err}");
            assert!(
                err.to_string().contains("/agent/hermes/config.yaml"),
                "{err}"
            );
            assert!(err.to_string().contains(detail), "{err}");
        }
    }

    #[test]
    fn invalid_generated_routing_is_refused() {
        for (generated, detail) in [
            ("model: [", "invalid YAML"),
            (
                "model: {}",
                "generated model.provider must be a non-empty string",
            ),
            (
                "model:\n  provider: openrouter\n  api_key: []\n",
                "generated model.api_key must be a string",
            ),
            (
                "model:\n  provider: openrouter\nfallback_providers: old\n",
                "generated fallback_providers must be a sequence",
            ),
        ] {
            let err = merge(
                "/agent/hermes/config.yaml",
                b"model:\n  provider: custom\n",
                generated,
            )
            .unwrap_err();
            assert!(matches!(&err, crate::Error::Invalid(_)), "{err}");
            assert!(err.to_string().contains(detail), "{err}");
        }
    }

    #[test]
    fn generated_settings_are_applied_without_discarding_unrelated_saved_values() {
        let existing = b"model:\n  provider: custom\n  temperature: 0.4\nterminal:\n  backend: local\n  timeout: 90\nmcp_servers:\n  saved:\n    command: saved-server\n  shared:\n    command: old-server\n    args: [old]\nauth:\n  timeout: 30\n";
        let generated = "model:\n  provider: openrouter\n  temperature: 0.7\n  reasoning_effort: high\nterminal:\n  backend: docker\nmcp_servers:\n  supplied:\n    command: supplied-server\n  shared:\n    command: new-server\n    args: [new]\nauth:\n  timeout: 60\n  adopt_external_logins: false\nextra_setting: enabled\n";
        let result = yaml(&merge("config.yaml", existing, generated).unwrap());
        assert_eq!(result["model"]["temperature"].as_f64(), Some(0.7));
        assert_eq!(result["model"]["reasoning_effort"].as_str(), Some("high"));
        assert_eq!(result["terminal"], yaml("backend: docker\ntimeout: 90\n"));
        assert_eq!(
            result["mcp_servers"],
            yaml(
                "saved:\n  command: saved-server\nsupplied:\n  command: supplied-server\nshared:\n  command: new-server\n  args: [new]\n"
            )
        );
        assert_eq!(
            result["auth"],
            yaml("timeout: 60\nadopt_external_logins: false\n")
        );
        assert_eq!(result["extra_setting"].as_str(), Some("enabled"));
        // The same caller-supplied values also apply to a first launch.
        let first = yaml(&merge("config.yaml", b"", generated).unwrap());
        assert_eq!(
            first["mcp_servers"]["supplied"]["command"].as_str(),
            Some("supplied-server")
        );
        assert_eq!(first["extra_setting"].as_str(), Some("enabled"));
    }

    #[test]
    fn unchanged_config_keeps_comments_formatting_and_anchors() {
        let existing = b"# Keep the server's tuning note.\nmodel: {provider: custom, default: example, context_length: 32768}\nterminal: &terminal {backend: local}\nother_terminal: *terminal\nfallback_providers: []\nauth: {adopt_external_logins: false}\n";
        let generated = "model:\n  provider: custom\n  default: example\n  context_length: 131072\nfallback_providers: []\nauth:\n  adopt_external_logins: false\n";
        assert_eq!(
            merge("config.yaml", existing, generated).unwrap(),
            std::str::from_utf8(existing).unwrap()
        );
    }

    #[test]
    fn context_override_survives_model_and_provider_changes() {
        let existing =
            b"model:\n  provider: custom\n  default: local-model\n  context_length: 32768\n";
        for generated in [
            "model:\n  provider: custom\n  default: other-model\n  context_length: 131072\n",
            "model:\n  provider: openrouter\n  default: hosted-model\n",
        ] {
            let result = yaml(&merge("config.yaml", existing, generated).unwrap());
            assert_eq!(result["model"]["context_length"].as_u64(), Some(32768));
            assert_ne!(result["model"]["default"].as_str(), Some("local-model"));
        }
    }

    #[test]
    fn updating_routing_keeps_yaml_scalar_spelling_for_hermes() {
        // Hermes uses PyYAML's YAML 1.1 resolver. These quoted and plain
        // scalars have different types there, even when YAML 1.2 equates them.
        let settings = "settings:\n  quoted_off: \"off\"\n  plain_off: off\n  quoted_yes: 'yes'\n  plain_yes: yes\n  quoted_date: \"2026-11-16\"\n  plain_date: 2026-11-16\n  quoted_time: '12:34:56'\n  plain_time: 12:34:56\n  octal: 0123\n";
        let existing = format!("model:\n  provider: custom\n{settings}");
        let merged = merge(
            "config.yaml",
            existing.as_bytes(),
            "model:\n  provider: openrouter\n",
        )
        .unwrap();
        assert!(
            merged.contains(settings),
            "saved scalar spelling changed:\n{merged}"
        );
    }

    #[test]
    fn supplied_scalar_type_change_is_not_a_no_op() {
        let existing = b"model: {provider: custom}\nsetting: off\nfallback_providers: []\nauth: {adopt_external_logins: false}\n";
        let merged = merge(
            "config.yaml",
            existing,
            "model: {provider: custom}\nsetting: \"off\"\n",
        )
        .unwrap();
        assert!(
            merged.contains("setting: \"off\""),
            "supplied string became a YAML 1.1 boolean:\n{merged}"
        );
    }

    #[test]
    fn supplied_nested_settings_keep_inherited_siblings() {
        let existing = b"defaults: &defaults\n  mcp_servers:\n    saved: {command: saved-server}\nmodel: {provider: custom}\n<<: *defaults\n";
        let generated =
            "model: {provider: openrouter}\nmcp_servers:\n  supplied: {command: supplied-server}\n";
        let merged = merge("config.yaml", existing, generated).unwrap();
        let mut parsed: Value = serde_yaml_ng::from_str(&merged).unwrap();
        parsed.apply_merge().unwrap();
        assert_eq!(
            parsed["mcp_servers"]["saved"]["command"].as_str(),
            Some("saved-server")
        );
        assert_eq!(
            parsed["mcp_servers"]["supplied"]["command"].as_str(),
            Some("supplied-server")
        );
    }

    #[test]
    fn unchanged_routing_keeps_existing_scalar_quotes() {
        let existing = b"# handwritten configuration\nmodel: {provider: custom, default: example}\nfallback_providers: []\nauth: {adopt_external_logins: false}\n";
        let generated = "model:\n  provider: \"custom\"\n  default: \"example\"\n";
        assert_eq!(
            merge("config.yaml", existing, generated).unwrap(),
            std::str::from_utf8(existing).unwrap()
        );
    }

    #[test]
    fn shortening_routing_keeps_later_aliases_resolvable() {
        let existing = format!(
            "model: {{provider: custom, base_url: https://old.example/{}}}\nterminal: &terminal {{backend: local}}\nother_terminal: *terminal\n",
            "x".repeat(200)
        );
        let merged = merge(
            "config.yaml",
            existing.as_bytes(),
            "model: {provider: openrouter}\n",
        )
        .unwrap();
        assert_eq!(yaml(&merged)["terminal"], yaml(&merged)["other_terminal"]);
        assert!(merged.contains("other_terminal: *terminal"));
    }

    #[test]
    fn invalid_merge_in_untouched_settings_is_refused() {
        for (existing, generated) in [
            (
                "model: {provider: custom}\nterminal: {<<: 42}\n",
                "model: {provider: openrouter}\n",
            ),
            (
                "model: {provider: custom}\n",
                "model: {provider: openrouter}\nterminal: {<<: 42}\n",
            ),
        ] {
            let error = merge("config.yaml", existing.as_bytes(), generated).unwrap_err();
            assert!(
                matches!(error, crate::Error::Invalid(ref message) if message.starts_with("cannot update Hermes config config.yaml: invalid YAML merge:")),
                "{error}"
            );
        }
    }

    #[test]
    fn direct_routing_aliases_do_not_change_source_defaults() {
        let existing = b"model_defaults: &m {provider: custom, base_url: https://old, temperature: 0.4}\nauth_defaults: &a {adopt_external_logins: true, timeout: 30}\nmodel: *m\nauth: *a\n";
        let result =
            yaml(&merge("config.yaml", existing, "model: {provider: openrouter}\n").unwrap());
        assert_eq!(
            result["model_defaults"]["provider"].as_str(),
            Some("custom")
        );
        assert_eq!(
            result["auth_defaults"]["adopt_external_logins"].as_bool(),
            Some(true)
        );
        assert_eq!(result["model"]["provider"].as_str(), Some("openrouter"));
        assert!(result["model"]["base_url"].is_null());
        assert_eq!(result["model"]["temperature"].as_f64(), Some(0.4));
        assert_eq!(
            result["auth"]["adopt_external_logins"].as_bool(),
            Some(false)
        );
        assert_eq!(result["auth"]["timeout"].as_u64(), Some(30));
    }

    #[test]
    fn unchanged_sequence_values_keep_existing_formatting() {
        let existing = b"model: {provider: custom}\nfallback_providers: [{model: fallback, provider: openrouter}]\nauth: {adopt_external_logins: false}\nargs: [run, fast]\n";
        let generated = "model: {provider: custom}\nfallback_providers:\n  - provider: \"openrouter\"\n    model: \"fallback\"\nargs:\n  - \"run\"\n  - \"fast\"\n";
        assert_eq!(
            merge("config.yaml", existing, generated).unwrap(),
            std::str::from_utf8(existing).unwrap()
        );
    }
}
