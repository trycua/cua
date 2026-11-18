//! What `cua run --compat e2b --prewarm TEMPLATE[=N]` needs to know about E2B.
//!
//! The CLI keeps capacity warm for a Cua image; this says which Cua image an
//! E2B template reference starts from, so `Sandbox.create(template=...)`
//! afterwards lands on warm capacity.

use std::collections::BTreeMap;

use super::{TemplateTarget, code_interpreter};

/// The Cua image behind an E2B template reference (`None`: the gateway's
/// default Linux image), or why there is nothing to keep warm.
///
/// - `base`, the empty reference and `code-interpreter-v1` start from the
///   default image (the latter adds its server at create, a few hundred ms).
/// - A template mapped in the gateway's config to an image is that image.
/// - A Cua image reference (`vm:`, a registry path) is itself.
/// - A configured checkpoint, a snapshot or a template built by the gateway
///   is a fork of a memory checkpoint, which is already near-instant and
///   lives in the gateway's state: it has no image to prewarm.
pub fn resolve_image(
    reference: &str,
    templates: &BTreeMap<String, TemplateTarget>,
) -> Result<Option<String>, String> {
    let r = reference.trim();
    match templates.get(r) {
        Some(TemplateTarget::Image(i)) => return Ok(Some(i.clone())),
        Some(TemplateTarget::Checkpoint(_)) => {
            return Err(format!(
                "template '{r}' is a checkpoint: creating from it forks a saved state, so there is no image to keep warm"
            ));
        }
        None => {}
    }
    if r.is_empty() || r == "base" || code_interpreter::wants(r) {
        return Ok(None);
    }
    if r.starts_with("vm:") || r.starts_with("ghcr.io/trycua/") || r.contains("@sha256:") {
        return Ok(Some(r.to_string()));
    }
    Err(format!(
        "template '{r}' is built or saved by the gateway (a checkpoint) or unknown: prewarm a Cua image instead (--prewarm base, or an image reference)"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_templates_to_images() {
        let mut t = BTreeMap::new();
        t.insert(
            "mine".to_string(),
            TemplateTarget::Image("vm:/x.img".into()),
        );
        t.insert("saved".to_string(), TemplateTarget::Checkpoint("cp".into()));
        assert_eq!(resolve_image("base", &t), Ok(None));
        assert_eq!(resolve_image("", &t), Ok(None));
        assert_eq!(resolve_image("code-interpreter-v1", &t), Ok(None));
        assert_eq!(resolve_image("mine", &t), Ok(Some("vm:/x.img".into())));
        assert_eq!(
            resolve_image("ghcr.io/trycua/linux:24.04", &t),
            Ok(Some("ghcr.io/trycua/linux:24.04".into()))
        );
        assert!(
            resolve_image("saved", &t)
                .unwrap_err()
                .contains("checkpoint")
        );
        assert!(resolve_image("some-built-template", &t).is_err());
    }
}
