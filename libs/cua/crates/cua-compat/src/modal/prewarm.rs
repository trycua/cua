//! What `cua run --compat modal --prewarm IMAGE[=N]` needs to know about Modal.
//!
//! The CLI keeps capacity warm for a Cua image; this module says which Cua
//! image a Modal image reference starts from, so a Sandbox or Function
//! container created afterwards lands on warm capacity, and what Modal's own
//! warm-pool settings amount to.

use crate::modal::image::{Base, classify_base};
use crate::modal::pb::api as pb;
use std::collections::HashMap;

/// The Cua image a Modal base image reference starts from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The Cua image to keep warm (`None`: the gateway's default Cua image).
    pub cua_image: Option<String>,
    /// A Python the gateway installs on top (`python:<v>` bases): built once
    /// as a cached layer, then every Sandbox forks it.
    pub python: Option<String>,
}

/// Resolves `reference` the way the image builder does at create: the Cua
/// images map to themselves, `images.json` entries to their target,
/// `python:*`, `ubuntu:*` and `debian:*` to the default Cua image (with the
/// Python version for the first); another reference is an error saying how to
/// map it, as `ImageBuildError` is at create.
pub fn resolve_image(
    reference: &str,
    image_map: &HashMap<String, String>,
    default_image: &str,
) -> Result<Target, String> {
    let default = Some(default_image)
        .filter(|i| !i.is_empty())
        .map(String::from);
    match classify_base(reference, image_map)? {
        Base::Cua(img) => Ok(Target {
            cua_image: Some(img),
            python: None,
        }),
        Base::Python(v) => Ok(Target {
            cua_image: default,
            python: Some(v),
        }),
        Base::Distro | Base::Micromamba(_) => Ok(Target {
            cua_image: default,
            python: None,
        }),
        Base::Scratch => Err("FROM scratch has nothing to keep warm".into()),
    }
}

/// How many containers Modal's settings keep ready for a Function:
/// `min_containers` always, plus `buffer_containers` while inputs run.
pub fn warm_containers(
    def: &pb::Function,
    min_override: Option<u32>,
    buffer_override: Option<u32>,
) -> u32 {
    let s = def.autoscaler_settings.unwrap_or_default();
    let min = min_override
        .or(s.min_containers)
        .unwrap_or(def.warm_pool_size);
    let buffer = buffer_override
        .or(s.buffer_containers)
        .unwrap_or(def.experimental_buffer_containers);
    min + buffer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bases_resolve_as_the_builder_resolves_them() {
        let mut map = HashMap::new();
        map.insert("my/img:1".to_string(), "ghcr.io/me/mine".to_string());
        let r = |s: &str| resolve_image(s, &map, "ghcr.io/trycua/linux:24.04");
        assert_eq!(
            r("ghcr.io/trycua/linux:24.04").unwrap(),
            Target {
                cua_image: Some("ghcr.io/trycua/linux:24.04".into()),
                python: None
            }
        );
        assert_eq!(
            r("my/img:1").unwrap().cua_image.as_deref(),
            Some("ghcr.io/me/mine")
        );
        let py = r("python:3.12-slim").unwrap();
        assert_eq!(py.python.as_deref(), Some("3.12"));
        assert_eq!(py.cua_image.as_deref(), Some("ghcr.io/trycua/linux:24.04"));
        assert_eq!(r("ubuntu:22.04").unwrap().python, None);
        assert!(
            r("nvidia/cuda:12")
                .unwrap_err()
                .contains("cannot run on Cua Spaces")
        );
        assert!(r("scratch").is_err());
    }

    #[test]
    fn modal_warm_settings_add_up() {
        let mut def = pb::Function {
            warm_pool_size: 2,
            ..Default::default()
        };
        assert_eq!(warm_containers(&def, None, None), 2);
        assert_eq!(warm_containers(&def, Some(1), Some(3)), 4);
        def.autoscaler_settings = Some(pb::AutoscalerSettings {
            min_containers: Some(5),
            buffer_containers: Some(1),
            ..Default::default()
        });
        assert_eq!(warm_containers(&def, None, None), 6);
    }
}
