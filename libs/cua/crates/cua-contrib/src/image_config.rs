//! The runtime config of a registry image (`Entrypoint`, `Cmd`, `User`,
//! `WorkingDir`, `Env`), for platforms that do not run the image's own
//! entrypoint and need it as a start command (E2B templates).

use crate::common::sh_join;
use cua_sandbox_core::{Error, ProviderImage, RegistryCredentials, Result};
use std::sync::Arc;

/// What an image runs by default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImageConfig {
    /// `Entrypoint`.
    pub entrypoint: Vec<String>,
    /// `Cmd`.
    pub cmd: Vec<String>,
    /// `User` (empty: root).
    pub user: String,
    /// `WorkingDir`.
    pub workdir: String,
    /// `Env` (`K=V`).
    pub env: Vec<String>,
}

impl ImageConfig {
    /// Parses an OCI image config blob.
    pub fn parse(blob: &[u8]) -> Result<Self> {
        let v: serde_json::Value = serde_json::from_slice(blob)?;
        let c = v.get("config").cloned().unwrap_or_default();
        let list = |k: &str| -> Vec<String> {
            c.get(k)
                .and_then(|a| a.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        let text = |k: &str| {
            c.get(k)
                .and_then(|s| s.as_str())
                .unwrap_or_default()
                .to_string()
        };
        Ok(Self {
            entrypoint: list("Entrypoint"),
            cmd: list("Cmd"),
            user: text("User"),
            workdir: text("WorkingDir"),
            env: list("Env"),
        })
    }

    /// The process the image starts (`Entrypoint` + `Cmd`), or `command`
    /// when given (it replaces both, like the core's `command`).
    pub fn argv(&self, command: Option<&[String]>) -> Vec<String> {
        match command {
            Some(c) if !c.is_empty() => c.to_vec(),
            _ => self.entrypoint.iter().chain(&self.cmd).cloned().collect(),
        }
    }

    /// [`Self::argv`] as a shell line, detached from the caller (the start
    /// command of a template must not block its build), with the image's
    /// environment and working directory.
    pub fn start_line(&self, command: Option<&[String]>) -> Option<String> {
        let argv = self.argv(command);
        if argv.is_empty() {
            return None;
        }
        let mut line = String::new();
        if !self.workdir.is_empty() {
            line.push_str(&format!(
                "cd {} && ",
                crate::common::sh_quote(&self.workdir)
            ));
        }
        for kv in &self.env {
            if let Some((k, _)) = kv.split_once('=')
                && !k.is_empty()
                && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                line.push_str(&format!("export {}; ", crate::common::sh_quote(kv)));
            }
        }
        line.push_str(&format!(
            "nohup {} >/tmp/cua-entrypoint.log 2>&1 &",
            sh_join(&argv)
        ));
        Some(line)
    }
}

/// Reads image configs (the registry, or a fixed answer in tests).
#[async_trait::async_trait]
pub trait ImageConfigSource: Send + Sync {
    /// The config of `image` (its pinned, single-platform reference).
    async fn config(
        &self,
        image: &ProviderImage,
        creds: Option<&RegistryCredentials>,
    ) -> Result<ImageConfig>;
}

/// Reads configs from the image's registry.
#[derive(Clone, Copy, Debug, Default)]
pub struct RegistryConfigSource;

#[async_trait::async_trait]
impl ImageConfigSource for RegistryConfigSource {
    async fn config(
        &self,
        image: &ProviderImage,
        creds: Option<&RegistryCredentials>,
    ) -> Result<ImageConfig> {
        let client = match creds {
            Some(c) => cua_image::RegistryClient::with_credentials(c.clone()),
            None => cua_image::RegistryClient::default(),
        };
        let reg = |e: cua_image::ImageError| {
            Error::UnsupportedImage(format!(
                "{}: cannot read the image config: {e}",
                image.reference
            ))
        };
        let (pinned, manifest, _) = client
            .resolve_platform(&image.pinned_ref, &image.arch)
            .await
            .map_err(reg)?;
        let desc = manifest.config.ok_or_else(|| {
            Error::UnsupportedImage(format!("{}: the manifest has no config", image.reference))
        })?;
        let blob = client.blob_bytes(&pinned, &desc).await.map_err(reg)?;
        ImageConfig::parse(&blob)
    }
}

/// A fixed config for every image (tests).
#[derive(Clone, Debug, Default)]
pub struct FixedConfigSource(pub ImageConfig);

#[async_trait::async_trait]
impl ImageConfigSource for FixedConfigSource {
    async fn config(
        &self,
        _: &ProviderImage,
        _: Option<&RegistryCredentials>,
    ) -> Result<ImageConfig> {
        Ok(self.0.clone())
    }
}

/// The default source.
pub fn registry() -> Arc<dyn ImageConfigSource> {
    Arc::new(RegistryConfigSource)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_builds_a_detached_start_line() {
        let c = ImageConfig::parse(
            br#"{"config":{"Entrypoint":["/usr/bin/tini","--"],"Cmd":["/start.sh"],"User":"root","WorkingDir":"/root","Env":["PATH=/usr/bin","DISPLAY=:1"]}}"#,
        )
        .unwrap();
        assert_eq!(c.argv(None), ["/usr/bin/tini", "--", "/start.sh"]);
        let line = c.start_line(None).unwrap();
        assert!(line.starts_with("cd /root && export PATH=/usr/bin; export DISPLAY=:1; nohup /usr/bin/tini -- /start.sh"), "{line}");
        assert!(line.ends_with("&"));
        assert_eq!(c.argv(Some(&["sleep".into(), "1".into()])), ["sleep", "1"]);
        assert_eq!(ImageConfig::default().start_line(None), None);
    }
}
