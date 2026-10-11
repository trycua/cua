//! [`ProviderKind::Contrib`] sandboxes: create, reattach and state files
//! over a registered [`crate::Provider`].

use super::*;
use crate::provider::{PortExposure, ProviderCapabilities, ProviderCreate, ProviderImage, RunKind};

/// State-file key naming the contrib provider (its presence marks a contrib
/// sandbox, whether or not this build has the provider).
pub(super) const CONTRIB_PROVIDER_KEY: &str = "contrib_provider";
/// State-file key of the provider's sandbox id.
pub(super) const CONTRIB_ID_KEY: &str = "contrib_id";
/// State-file key of the provider's public instance details.
pub(super) const DETAILS_KEY: &str = "provider_details";

/// How long image resolution may take before a contrib create continues
/// with the reference as given (the provider then builds by tag).
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(30);

/// The contrib provider a state file names.
pub(super) fn contrib_of(s: &SandboxState) -> Option<(&str, &str)> {
    let SandboxState::Local(l) = s else {
        return None;
    };
    let provider = l.extra.get(CONTRIB_PROVIDER_KEY)?.as_str()?;
    let id = l
        .extra
        .get(CONTRIB_ID_KEY)
        .and_then(Value::as_str)
        .unwrap_or_default();
    Some((provider, id))
}

/// Refuses what `caps` cannot run, before any API call.
fn check_request(word: &str, caps: &ProviderCapabilities, o: &CreateOptions) -> Result<()> {
    let unsupported = |op: String| crate::provider::unsupported(word, op);
    if o.network == NetworkMode::None {
        return Err(unsupported(
            "network=\"none\" (only local QEMU VMs run without outbound network)".into(),
        ));
    }
    if !o.sidecars.is_empty() {
        return Err(unsupported("sidecars".into()));
    }
    if o.fleet.pool.is_some() {
        return Err(Error::InvalidArgument(format!(
            "pools are Cua cloud capacity; --on {word} takes an image"
        )));
    }
    if !o.os.eq_ignore_ascii_case("linux") {
        return Err(Error::UnsupportedImage(format!(
            "{word} runs Linux sandboxes only (asked for {}); run {} guests with --on local or \
             --on cloud",
            o.os, o.os
        )));
    }
    if o.command.as_ref().is_some_and(|c| !c.is_empty()) && !caps.command {
        return Err(unsupported(
            "a command replacing the image's entrypoint".into(),
        ));
    }
    if o.registry_credentials.is_some() && !caps.private_registry {
        return Err(unsupported("private registry credentials".into()));
    }
    if let Some(max) = caps.max_cpus
        && o.cpus > max
    {
        return Err(Error::InvalidArgument(format!(
            "{word} sandboxes have at most {max} vCPUs (asked for {})",
            o.cpus
        )));
    }
    if let Some(max) = caps.max_memory_mb
        && o.memory_mb > max
    {
        return Err(Error::InvalidArgument(format!(
            "{word} sandboxes have at most {max} MiB of memory (asked for {})",
            o.memory_mb
        )));
    }
    if caps.ports == PortExposure::None && (!o.ports.is_empty() || !o.services.is_empty()) {
        return Err(unsupported("exposing guest ports".into()));
    }
    Ok(())
}

/// The run kind an image prefix asks for (`container:` / `vm:`), and the
/// reference without it.
fn split_kind(image: &str) -> (Option<RunKind>, &str) {
    let image = image.trim();
    for (prefix, kind) in [
        ("container:", RunKind::Container),
        ("docker:", RunKind::Container),
        ("vm:", RunKind::Vm),
    ] {
        if let Some(rest) = image.strip_prefix(prefix)
            && !rest.starts_with("//")
        {
            return (Some(kind), rest);
        }
    }
    (None, image)
}

/// Resolves and pins the image for a provider: the variant it can run,
/// on an architecture it offers. Refuses VM-only images on container
/// platforms (and the reverse) and images with no build for the provider's
/// architectures. When the registry cannot be read the reference runs as
/// given (unpinned).
pub(super) async fn resolve_image(
    word: &str,
    caps: &ProviderCapabilities,
    o: &CreateOptions,
) -> Result<(ProviderImage, Option<ImageInfo>)> {
    let (asked, reference) = split_kind(&o.image);
    let reference = cua_image::canonical::alias(reference).unwrap_or_else(|| reference.to_string());
    if reference.is_empty() {
        return Err(Error::InvalidArgument("image is required".into()));
    }
    let kinds: Vec<RunKind> = match asked {
        Some(k) if caps.kinds.contains(&k) => vec![k],
        Some(k) => {
            return Err(Error::UnsupportedImage(format!(
                "{word} does not run {} sandboxes (it runs: {}); use --on local or --on cloud",
                k.as_str(),
                caps.kinds
                    .iter()
                    .map(|k| k.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        None => caps.kinds.clone(),
    };
    let backend = match kinds.as_slice() {
        [RunKind::Container] => cua_image::Backend::Container,
        [RunKind::Vm] => cua_image::Backend::Vm,
        _ => cua_image::Backend::Fleet,
    };
    let arch = caps.arches.first().copied().unwrap_or("amd64");
    let creds = o.scoped_credentials();
    let resolved = tokio::time::timeout(
        RESOLVE_TIMEOUT,
        cua_image::resolve::resolve_with_credentials(&reference, backend, arch, creds.as_ref()),
    )
    .await;
    let r = match resolved {
        Ok(Ok(r)) => r,
        Ok(Err(cua_image::ImageError::UnsupportedVariant { found, .. })) => {
            return Err(Error::UnsupportedImage(format!(
                "{reference} has no variant {word} can run: it offers {} and {word} runs {} \
                 (a VM image needs a provider with VMs, or --on local / --on cloud)",
                if found.is_empty() {
                    "none".to_string()
                } else {
                    found.join(", ")
                },
                kinds
                    .iter()
                    .map(|k| match k {
                        RunKind::Container => "rootfs (container)",
                        RunKind::Vm => "containerdisk (VM)",
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        Ok(Err(
            e @ (cua_image::ImageError::NotFound(_) | cua_image::ImageError::Unauthorized(_)),
        )) => {
            return Err(Error::UnsupportedImage(format!("{reference}: {e}")));
        }
        Ok(Err(e)) => {
            tracing::warn!(image = %reference, error = %e, "image not resolved; running it as given");
            return Ok((unpinned(&reference, kinds[0], arch), None));
        }
        Err(_) => {
            tracing::warn!(image = %reference, "image resolution timed out; running it as given");
            return Ok((unpinned(&reference, kinds[0], arch), None));
        }
    };
    let kind = match r.variant {
        cua_image::Variant::Rootfs => RunKind::Container,
        cua_image::Variant::Containerdisk => RunKind::Vm,
        cua_image::Variant::Lume => {
            return Err(Error::UnsupportedImage(format!(
                "{reference} is a macOS (Lume) image; macOS runs locally with Lume only"
            )));
        }
    };
    if !kinds.contains(&kind) {
        return Err(Error::UnsupportedImage(format!(
            "{reference} is a {} image and {word} runs {} sandboxes only",
            kind.as_str(),
            kinds
                .iter()
                .map(|k| k.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    if r.os != "linux" {
        return Err(Error::UnsupportedImage(format!(
            "{reference} is a {} image; {word} runs Linux sandboxes only",
            r.os
        )));
    }
    let run_arch = r.arch.clone().unwrap_or_else(|| arch.to_string());
    if r.emulated || !caps.arches.contains(&run_arch.as_str()) {
        return Err(Error::UnsupportedImage(format!(
            "{reference} has no {} build ({word} runs {}; the image offers {})",
            caps.arches.join("/"),
            caps.arches.join(", "),
            r.architectures.join(", ")
        )));
    }
    let info = ImageInfo::from(&r);
    Ok((
        ProviderImage {
            reference: r.reference,
            pinned_ref: r.pinned_ref,
            digest: r.digest,
            kind,
            arch: run_arch,
            spacesd: r.spacesd,
        },
        Some(info),
    ))
}

fn unpinned(reference: &str, kind: RunKind, arch: &str) -> ProviderImage {
    ProviderImage {
        reference: reference.to_string(),
        pinned_ref: reference.to_string(),
        digest: String::new(),
        kind,
        arch: arch.to_string(),
        spacesd: None,
    }
}

impl Sandboxes {
    pub(super) async fn create_contrib(&self, o: &CreateOptions) -> Result<Sandbox> {
        let word = o
            .contrib
            .as_deref()
            .filter(|w| !w.is_empty())
            .ok_or_else(|| {
                Error::InvalidArgument(
                    "a contrib sandbox needs its provider (--on e2b, --on daytona, ...)".into(),
                )
            })?;
        let provider = self.contrib_provider(word)?.clone();
        let caps = provider.capabilities();
        check_request(word, &caps, o)?;
        provider.check_configured()?;
        let (image, image_info) = resolve_image(word, &caps, o).await?;
        let mut services = o.all_services();
        // `env` (3211) only for images that carry cua-spacesd (or might).
        if !o.services.contains_key("env") && image.spacesd == Some(false) {
            services.remove("env");
        }
        let mut ports: Vec<u16> = services.values().copied().chain(o.ports.clone()).collect();
        ports.sort_unstable();
        ports.dedup();
        if caps.ports == PortExposure::None {
            services.clear();
            ports.clear();
        }
        let ephemeral = o.name.is_none();
        let name = o
            .name
            .clone()
            .or_else(|| o.ephemeral_name.clone())
            .unwrap_or_else(|| format!("cua-eph-{:08x}", rand_u32()));
        // A GPU type the provider offers (refused before any API call).
        let gpu = match o.gpu.as_deref() {
            None => None,
            Some(g) => Some(
                crate::gpu::GpuSupport {
                    runtime: word.to_string(),
                    options: caps.gpus.clone(),
                    reason: format!("{word} offers no GPU sandboxes through cua"),
                }
                .pick(g)
                .map_err(Error::InvalidArgument)?
                .id
                .clone(),
            ),
        };
        // The token reaches spacesd through the environment only where the
        // platform hands it to the image's entrypoint; elsewhere the driver
        // starts in bootstrap mode and `spacesd()` installs one with `Init`.
        let env_token = if caps.env_to_entrypoint {
            local_env_token(o)
        } else {
            o.env_token.clone().filter(|t| !t.is_empty())
        };
        let env = if caps.env_to_entrypoint {
            guest_env(o, env_token.as_deref())
        } else {
            o.env.clone()
        };
        let mut labels = BTreeMap::new();
        labels.insert("cua.managed".to_string(), "true".to_string());
        labels.insert("cua.name".to_string(), name.clone());
        labels.insert("cua.ephemeral".to_string(), ephemeral.to_string());
        let spec = ProviderCreate {
            name: name.clone(),
            image,
            cpus: o.cpus,
            memory_mb: o.memory_mb,
            env,
            command: o.command.clone().filter(|c| !c.is_empty()),
            ports,
            ttl: o
                .fleet
                .ttl_seconds_after_created
                .filter(|t| *t > 0)
                .map(|t| Duration::from_secs(u64::from(t))),
            labels,
            registry_credentials: o.scoped_credentials(),
            timeout: o.ready_timeout,
            runtime: o.runtime.clone(),
            gpu,
        };
        let instance = provider.create(&spec).await?;
        let sandbox = Sandbox {
            mgr: self.clone(),
            name: name.clone(),
            ephemeral,
            env_token: env_token.clone(),
            bootstrap_token: Default::default(),
            services: services.clone(),
            image_info: image_info.clone(),
            target: Target::Contrib {
                provider: ContribHandle(provider.clone()),
                instance: instance.clone(),
            },
        };
        if !ephemeral
            && let Err(e) = self.save_contrib_state(
                word,
                &instance,
                o,
                &services,
                env_token.as_deref(),
                image_info.as_ref(),
            )
        {
            let _ = provider.delete(&instance.id).await;
            return Err(e);
        }
        Ok(sandbox)
    }

    fn save_contrib_state(
        &self,
        word: &str,
        instance: &crate::ProviderInstance,
        o: &CreateOptions,
        services: &BTreeMap<String, u16>,
        env_token: Option<&str>,
        image_info: Option<&ImageInfo>,
    ) -> Result<()> {
        let mut extra = Map::new();
        extra.insert(CONTRIB_PROVIDER_KEY.into(), Value::String(word.into()));
        extra.insert(CONTRIB_ID_KEY.into(), Value::String(instance.id.clone()));
        extra.insert("services".into(), serde_json::to_value(services)?);
        if let Some(t) = env_token {
            extra.insert("env_token".into(), Value::String(t.into()));
        }
        if let Some(i) = image_info {
            extra.insert(IMAGE_INFO_KEY.into(), serde_json::to_value(i)?);
        }
        for key in [crate::byoc::DETAIL_RELAY_MACHINE, crate::byoc::DETAIL_PLACE] {
            if let Some(v) = instance.details.get(key) {
                extra.insert(key.into(), Value::String(v.clone()));
            }
        }
        // The provider's public details (never `_`-prefixed private
        // state), so a listing from state shows them without a call.
        let public: Map<String, Value> = instance
            .details
            .iter()
            .filter(|(k, _)| !k.starts_with('_'))
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect();
        if !public.is_empty() {
            extra.insert(DETAILS_KEY.into(), Value::Object(public));
        }
        self.inner.state.save(&SandboxState::Local(LocalState {
            name: instance_name(instance, &o.name),
            runtime_type: word.into(),
            image: registry_image_dict(&o.image, &o.os, Some("container")),
            host: String::new(),
            api_port: ENV_PORT,
            os_type: Some(o.os.clone()),
            memory_mb: Some(o.memory_mb),
            cpu_count: Some(o.cpus),
            status: "running".into(),
            created_at: python_utc_now(),
            extra,
            ..Default::default()
        }))?;
        // The file holds the spacesd token (and the environment may carry
        // secrets): owner-only.
        self.inner.state.restrict(&instance_name(instance, &o.name))
    }

    /// Reattaches to a persisted contrib sandbox.
    pub(super) async fn connect_contrib(&self, l: LocalState) -> Result<Sandbox> {
        let state = SandboxState::Local(l.clone());
        let (word, id) = contrib_of(&state)
            .map(|(w, i)| (w.to_string(), i.to_string()))
            .ok_or_else(|| Error::InvalidArgument("not a contrib sandbox".into()))?;
        let provider = self.contrib_provider(&word)?.clone();
        provider.check_configured()?;
        let instance = provider.get(&id).await?;
        if instance.status != Status::Running {
            return Err(Error::InvalidArgument(format!(
                "sandbox {} is {:?} on {word}; resume it first",
                l.name, instance.status
            )));
        }
        let services = l
            .extra
            .get("services")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_else(|| [("env".to_string(), ENV_PORT)].into());
        Ok(Sandbox {
            mgr: self.clone(),
            name: l.name.clone(),
            ephemeral: false,
            env_token: l
                .extra
                .get("env_token")
                .and_then(Value::as_str)
                .map(str::to_string),
            bootstrap_token: Default::default(),
            services,
            image_info: image_info_of(&l.extra),
            target: Target::Contrib {
                provider: ContribHandle(provider),
                instance,
            },
        })
    }

    /// The provider and id of a persisted contrib sandbox.
    pub(super) fn contrib_target(
        &self,
        s: &SandboxState,
    ) -> Result<(Arc<dyn crate::Provider>, String)> {
        let (word, id) = contrib_of(s).ok_or_else(|| Error::NotFound(s.name().into()))?;
        Ok((self.contrib_provider(word)?.clone(), id.to_string()))
    }
}

fn instance_name(instance: &crate::ProviderInstance, name: &Option<String>) -> String {
    name.clone().unwrap_or_else(|| instance.name.clone())
}
