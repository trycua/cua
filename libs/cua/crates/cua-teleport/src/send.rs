// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The send flow: manifest → selection → consent → OS authorization →
//! capture → chunked upload through `cua.env.v1.TeleportService`.

use std::collections::HashSet;
use std::sync::Arc;

use cua_spacesd_client::{EndpointKind, SpacesdClient, pb};
use sha2::{Digest, Sha256};

use crate::host::HostEffects;
use crate::registry::ExportRegistry;
use crate::{AppRef, ManifestItem, TeleportError, TransferManifest, TransferScope};

/// Shown (and required to be shown) to the user before a caller sets
/// [`SendOptions::relay_plaintext_ack`] (S1). Keep this exact wording in
/// sync anywhere it is surfaced in a client UI.
pub const RELAY_UNSEALED_WARNING: &str =
    "this Space's image predates end-to-end sealing; the Cua relay could read these secrets";

/// Where this sender's per-Space TOFU pins of a guest's sealed-delivery
/// public key persist (S1): `<CUA_HOME>/machine-seal-pins`, 0600.
fn open_relay_pin_store() -> Option<cua_machine_seal::PinStore> {
    let path = cua_home::cua_home().join("machine-seal-pins");
    match cua_machine_seal::PinStore::open(path) {
        Ok(store) => Some(store),
        Err(e) => {
            // Strictly better than not sealing at all: seal this one
            // delivery without a persistent pin rather than refuse the
            // whole send over an unrelated local IO problem.
            tracing::warn!(error = %e, "machine-seal pin store unavailable; sealing without a persistent pin");
            None
        }
    }
}

/// Seals `source`'s full bytes to the destination's pinned sealed-delivery
/// key (S1), when the destination is a `relay:` Space that reports one.
/// TOFU-pins a newly seen key; a *different* key for a Space already
/// pinned is refused outright (its identity changed unexpectedly) rather
/// than silently sealed to. Returns the sealed wire bytes and their
/// SHA-256 -- what the caller actually transmits and what the guest's
/// existing integrity check verifies -- or `None` when sealing does not
/// apply: not a relay destination, the guest predates sealing
/// (`machine_seal_public_key` empty), or the bundle is larger than this
/// crate seals in memory ([`cua_machine_seal::MAX_PLAINTEXT_LEN`]). `None`
/// leaves the existing relay: consent gate as the caller's next check.
async fn seal_for_relay(
    env: &SpacesdClient,
    import_id: &str,
    source: &mut BundleSource<'_>,
) -> Result<Option<(Vec<u8>, String)>, Error> {
    let EndpointKind::Relay { machine_id } = env.endpoint().kind() else {
        return Ok(None);
    };
    let machine_id = machine_id.clone();
    let Some(pub_key) = env.machine_seal_public_key().await.map_err(Error::Env)? else {
        return Ok(None);
    };
    let total = source.len().await?;
    if total > cua_machine_seal::MAX_PLAINTEXT_LEN as u64 {
        return Ok(None);
    }
    let plaintext = source.read_at(0, total as usize).await?;
    let recipient = cua_machine_seal::MachinePublicKey(pub_key);
    if let Some(pins) = open_relay_pin_store()
        && let Err(e) = pins.pin(&machine_id, recipient)
    {
        return Err(Error::Env(cua_spacesd_client::Error::Protocol(format!(
            "this Space's sealed-delivery key changed unexpectedly: {e}"
        ))));
    }
    let envelope = cua_machine_seal::seal(
        recipient,
        import_id,
        cua_machine_seal::purpose::TELEPORT_BUNDLE,
        &plaintext,
    )
    .map_err(|e| Error::Env(cua_spacesd_client::Error::Protocol(e.to_string())))?;
    let wire = envelope.to_bytes();
    let sha = hex::encode(Sha256::digest(&wire));
    Ok(Some((wire, sha)))
}

/// Why a teleport send failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Describing or capturing the session failed (no provider, unsupported
    /// scope, unreadable profile, denied OS authorization, bundle limits).
    #[error(transparent)]
    Teleport(#[from] TeleportError),
    /// The selection names items the manifest does not offer, or selects
    /// nothing.
    #[error("invalid selection: {0}")]
    InvalidSelection(String),
    /// The [`Approval`] callback declined. Nothing was read or sent.
    #[error("teleport was not approved")]
    NotApproved,
    /// The sandbox has no importer for this app.
    #[error("the sandbox cannot import {app:?}: {reason}")]
    Unsupported {
        /// Provider id.
        app: String,
        /// The guest's explanation.
        reason: String,
    },
    /// The destination is a `relay:` Space and nothing has sealed this
    /// delivery end to end (S1): refused unless
    /// [`SendOptions::relay_plaintext_ack`] is set, which the caller must
    /// only do after showing [`RELAY_UNSEALED_WARNING`] and getting
    /// explicit, per-delivery consent. Local and direct Spaces are
    /// unaffected (no relay in the path).
    #[error("{RELAY_UNSEALED_WARNING}")]
    RelayUnsealed,
    /// Talking to cua-spacesd failed.
    #[error(transparent)]
    Env(#[from] cua_spacesd_client::Error),
    /// The blocking export task panicked or was cancelled.
    #[error("export task failed: {0}")]
    Task(String),
}

/// Which manifest items to send.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Selection {
    /// The provider's default selection: every item with
    /// `default_checked: true`. Items a provider leaves unchecked (Claude
    /// Code's transcripts, Chrome's cookies) are sensitive or bulky and must
    /// be opted into with [`Selection::Items`].
    #[default]
    Default,
    /// Everything the scope implies.
    All,
    /// Exactly these manifest `rel_path`s (what a consent UI's checkboxes
    /// produce). Each must be offered by the manifest.
    Items(Vec<String>),
}

/// What an [`Approval`] is asked to consent to.
#[derive(Clone, Debug)]
pub struct ApprovalRequest<'a> {
    /// The full manifest (what could move).
    pub manifest: &'a TransferManifest,
    /// The items that will move.
    pub selected: Vec<ManifestItem>,
    /// Whether any selected item is sensitive. When true, the OS
    /// authorization prompt (Touch ID / passcode) follows an approval.
    pub sensitive: bool,
    /// Where the session goes (the spacesd endpoint).
    pub destination: String,
}

/// The consent gate: shown the manifest and the selection before anything is
/// read. Returning `false` aborts with [`Error::NotApproved`].
///
/// This is in addition to, never instead of, the OS authorization that
/// [`crate::ExportProvider::export_selected`] enforces for sensitive items.
pub trait Approval: Send + Sync {
    /// Approve or decline the transfer.
    fn approve(&self, request: &ApprovalRequest<'_>) -> bool;
}

impl<F> Approval for F
where
    F: Fn(&ApprovalRequest<'_>) -> bool + Send + Sync,
{
    fn approve(&self, request: &ApprovalRequest<'_>) -> bool {
        self(request)
    }
}

/// Approves every request (the caller already has consent, for example an
/// explicit `cua teleport push`). Sensitive items still need OS authorization.
#[derive(Clone, Copy, Debug, Default)]
pub struct AutoApprove;

impl Approval for AutoApprove {
    fn approve(&self, _: &ApprovalRequest<'_>) -> bool {
        true
    }
}

/// Upload progress callback: `(sent_bytes, total_bytes)`.
pub type Progress = Arc<dyn Fn(u64, u64) + Send + Sync>;

/// Upload and import options.
#[derive(Clone)]
pub struct SendOptions {
    /// Launch the app in the sandbox after importing (default true).
    pub launch_after: bool,
    /// Ask the guest to close a running instance first.
    pub close_running_app: bool,
    /// Replace existing state instead of merging.
    pub replace_existing: bool,
    /// Chunk size; default: the guest's preferred chunk size (1 MiB).
    pub chunk_bytes: Option<usize>,
    /// Import id (stable across retries); default: random.
    pub import_id: Option<String>,
    /// When the guest must wipe the delivered copy, Unix ms (`0`: no
    /// guest-side expiry). The Keyvault broker sets this so a delivered
    /// session has a TTL on the target even if the host never calls
    /// `WipeImport` (design 5.9).
    pub expires_at_ms: u64,
    /// Upload progress: bytes the Space has, of the total. It reports the
    /// total as the last chunk goes out, so `sent == total` means the Space
    /// is importing (the call returns once it has).
    pub progress: Option<Progress>,
    /// Marks a delivery the Keyvault broker authorized
    /// (`ImportOptions.broker_grant`); `None` for direct sends.
    pub broker_grant: Option<String>,
    /// Explicit, per-delivery opt-in to send over a `relay:` Space whose
    /// image predates end-to-end sealing (S1). `false` refuses such a send
    /// with [`Error::RelayUnsealed`] before anything is read or uploaded;
    /// set this only right after showing the caller
    /// [`RELAY_UNSEALED_WARNING`] and getting their explicit consent for
    /// *this* delivery -- never a standing setting. Local and direct
    /// Spaces are unaffected.
    pub relay_plaintext_ack: bool,
}

impl Default for SendOptions {
    fn default() -> Self {
        Self {
            launch_after: true,
            close_running_app: false,
            replace_existing: false,
            chunk_bytes: None,
            import_id: None,
            expires_at_ms: 0,
            progress: None,
            broker_grant: None,
            relay_plaintext_ack: false,
        }
    }
}

impl std::fmt::Debug for SendOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SendOptions")
            .field("launch_after", &self.launch_after)
            .field("close_running_app", &self.close_running_app)
            .field("replace_existing", &self.replace_existing)
            .field("chunk_bytes", &self.chunk_bytes)
            .field("import_id", &self.import_id)
            .finish_non_exhaustive()
    }
}

/// A captured, not yet uploaded, bundle.
#[derive(Clone, Debug)]
pub struct ExportedBundle {
    /// Provider id (the receiver's importer).
    pub provider_id: String,
    /// Scope.
    pub scope: TransferScope,
    /// The `SessionBundle` bytes.
    pub bytes: Vec<u8>,
    /// Lowercase hex SHA-256 of `bytes`.
    pub sha256: String,
    /// Manifest items that were selected.
    pub selected: Vec<ManifestItem>,
    /// Manifest items that were not (default selection only).
    pub withheld: Vec<ManifestItem>,
}

/// The result of a completed send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendOutcome {
    /// Provider id.
    pub provider_id: String,
    /// Import id used for the upload.
    pub import_id: String,
    /// Bundle size.
    pub bundle_bytes: u64,
    /// Bundle SHA-256.
    pub sha256: String,
    /// Selected manifest `rel_path`s.
    pub sent: Vec<String>,
    /// Manifest `rel_path`s left out by the default selection.
    pub withheld: Vec<String>,
    /// Items the guest imported.
    pub imported: Vec<String>,
    /// Items the guest skipped, as "item: reason".
    pub skipped: Vec<String>,
    /// Whether the guest launched the app.
    pub launched: bool,
}

/// Runs teleports from this machine: a provider registry (acting on one
/// host) plus upload options.
pub struct Teleporter {
    registry: Arc<ExportRegistry>,
    options: SendOptions,
}

impl Default for Teleporter {
    fn default() -> Self {
        Self::new()
    }
}

impl Teleporter {
    /// Built-in providers on [`crate::host::default_host`].
    pub fn new() -> Self {
        Self::with_registry(ExportRegistry::with_builtin())
    }

    /// Built-in providers on `host` (tests pass a
    /// [`crate::host::FakeHost`] with a temporary home).
    pub fn with_host(host: Arc<dyn HostEffects>) -> Self {
        Self::with_registry(ExportRegistry::with_builtin_host(host))
    }

    /// A custom registry.
    pub fn with_registry(registry: ExportRegistry) -> Self {
        Self {
            registry: Arc::new(registry),
            options: SendOptions::default(),
        }
    }

    /// Sets the upload options.
    pub fn options(mut self, options: SendOptions) -> Self {
        self.options = options;
        self
    }

    /// The provider registry.
    pub fn registry(&self) -> &ExportRegistry {
        &self.registry
    }

    /// The provider id that would handle `app`.
    pub fn provider_id(&self, app: &AppRef) -> Result<String, Error> {
        Ok(self.registry.resolve_for_app(app)?.id().to_string())
    }

    /// Describes what a transfer would move (blocking: reads the profile and
    /// may query the live app). Never prompts.
    pub fn manifest(&self, app: &AppRef, scope: TransferScope) -> Result<TransferManifest, Error> {
        Ok(self
            .registry
            .resolve_for_app(app)?
            .manifest(app, None, scope)?)
    }

    /// Captures the selected items into a bundle (blocking). Asks `approval`
    /// first, then the OS authorization for sensitive items; nothing is read
    /// before both pass.
    pub fn export(
        &self,
        app: &AppRef,
        scope: TransferScope,
        selection: &Selection,
        approval: &dyn Approval,
        destination: &str,
    ) -> Result<ExportedBundle, Error> {
        export_blocking(&self.registry, app, scope, selection, approval, destination)
    }

    /// The whole flow against a cua-spacesd: checks the guest can import
    /// the app, captures (after consent and OS authorization) and uploads the
    /// bundle with `TeleportService.ImportSession` in chunks.
    pub async fn send(
        &self,
        env: &SpacesdClient,
        app: &AppRef,
        scope: TransferScope,
        selection: Selection,
        approval: Arc<dyn Approval>,
    ) -> Result<SendOutcome, Error> {
        // Not `require_sealed_or_ack` here: it does not know whether this
        // destination can be sealed, so it would refuse a perfectly sealed
        // relay delivery too. `upload_bundle` makes the real decision,
        // after trying to seal, only reaching the gate when it could not.
        let provider_id = self.provider_id(app)?;
        // Ask the guest first, so the user is never prompted for a transfer
        // the sandbox cannot take.
        let manifest = env
            .teleport()
            .get_manifest(pb::GetManifestRequest {
                app: provider_id.clone(),
                scope: proto_scope(scope) as i32,
            })
            .await
            .map_err(cua_spacesd_client::Error::from)?
            .into_inner();
        if !manifest.supported {
            return Err(Error::Unsupported {
                app: provider_id,
                reason: manifest.limitation,
            });
        }
        let registry = self.registry.clone();
        let destination = env.endpoint().to_string();
        let app_owned = app.clone();
        let exported = tokio::task::spawn_blocking(move || {
            export_blocking(
                &registry,
                &app_owned,
                scope,
                &selection,
                approval.as_ref(),
                &destination,
            )
        })
        .await
        .map_err(|e| Error::Task(e.to_string()))??;
        let import_id = self
            .options
            .import_id
            .clone()
            .unwrap_or_else(random_import_id);
        let result = upload_bundle(
            env,
            &import_id,
            &exported.provider_id,
            scope,
            BundleSource::Bytes(&exported.bytes),
            &exported.sha256,
            &self.options,
        )
        .await?;
        Ok(SendOutcome {
            provider_id: exported.provider_id,
            import_id,
            bundle_bytes: exported.bytes.len() as u64,
            sha256: exported.sha256,
            sent: exported.selected.into_iter().map(|i| i.rel_path).collect(),
            withheld: exported.withheld.into_iter().map(|i| i.rel_path).collect(),
            imported: result.imported,
            skipped: result.skipped,
            launched: result.launched,
        })
    }
}

/// Teleports `app` from this machine into the sandbox behind `env` with the
/// built-in providers acting on the real host. See [`Teleporter::send`].
pub async fn send(
    env: &SpacesdClient,
    app: &AppRef,
    scope: TransferScope,
    selection: Selection,
    approval: Arc<dyn Approval>,
) -> Result<SendOutcome, Error> {
    Teleporter::new()
        .send(env, app, scope, selection, approval)
        .await
}

/// S1's mandatory minimum: refuse a `relay:` delivery unless the bundle was
/// sealed to the guest's pinned key, or the caller explicitly opted in for
/// this one send (the app's "this Space predates end-to-end sealing; the
/// Cua relay could read these secrets. Send anyway?" dialog, wired through
/// [`SendOptions::relay_plaintext_ack`] and the `ux` module's
/// `Consent::acknowledge_relay_plaintext`).
/// **On**: end-to-end sealing (the guest keypair, capability, TOFU pin,
/// seal/unseal) and the app consent dialogs both ship together with this
/// flip, so a sealed-capable Space (any guest image built from this point
/// on) never hits the refusal at all, and an older, unrepublished image
/// over a relay refuses until the app's opt-in or a republish.
pub const RELAY_SEALING_ENFORCED: bool = true;

/// The enforcement decision, kept pure and independent of any transport so
/// both states are trivial to test without a live or mocked spacesd.
fn gate_relay_delivery(kind: &EndpointKind, ack: bool, enforced: bool) -> Result<(), Error> {
    if enforced && matches!(kind, EndpointKind::Relay { .. }) && !ack {
        return Err(Error::RelayUnsealed);
    }
    Ok(())
}

fn require_sealed_or_ack(env: &SpacesdClient, options: &SendOptions) -> Result<(), Error> {
    gate_relay_delivery(
        env.endpoint().kind(),
        options.relay_plaintext_ack,
        RELAY_SEALING_ENFORCED,
    )
}

fn proto_scope(scope: TransferScope) -> pb::TeleportScope {
    match scope {
        TransferScope::TabsOnly => pb::TeleportScope::Session,
        TransferScope::FullProfile => pb::TeleportScope::Profile,
    }
}

fn random_import_id() -> String {
    format!("tp-{}", hex::encode(rand::random::<[u8; 12]>()))
}

fn export_blocking(
    registry: &ExportRegistry,
    app: &AppRef,
    scope: TransferScope,
    selection: &Selection,
    approval: &dyn Approval,
    destination: &str,
) -> Result<ExportedBundle, Error> {
    let provider = registry.resolve_for_app(app)?;
    let manifest = provider.manifest(app, None, scope)?;
    let (include, selected, withheld) = resolve_selection(&manifest, selection)?;
    let request = ApprovalRequest {
        manifest: &manifest,
        sensitive: selected.iter().any(|i| i.sensitive),
        selected: selected.clone(),
        destination: destination.to_string(),
    };
    if !approval.approve(&request) {
        return Err(Error::NotApproved);
    }
    let mut bytes = Vec::new();
    provider.export_selected(app, scope, include.as_ref(), &mut bytes)?;
    let sha256 = hex::encode(Sha256::digest(&bytes));
    Ok(ExportedBundle {
        provider_id: provider.id().to_string(),
        scope,
        bytes,
        sha256,
        selected,
        withheld,
    })
}

/// The `include` set for a provider, plus the selected and withheld items.
pub type Resolved = (
    Option<HashSet<String>>,
    Vec<ManifestItem>,
    Vec<ManifestItem>,
);

/// The `include` set for the provider plus the selected and withheld items.
/// Public so a caller that captures elsewhere (the Cua Keyvault broker,
/// which wants the exact `rel_path`s a [`Selection`] resolves to without
/// itself exporting anything) can resolve a selection identically to
/// [`Teleporter::send`], rather than a second, possibly-drifting copy of
/// this logic.
pub fn resolve_selection(
    manifest: &TransferManifest,
    selection: &Selection,
) -> Result<Resolved, Error> {
    let (include, selected, withheld) = match selection {
        Selection::All => (None, manifest.items.clone(), Vec::new()),
        Selection::Default => {
            let (selected, withheld): (Vec<_>, Vec<_>) = manifest
                .items
                .iter()
                .cloned()
                .partition(|i| i.default_checked);
            let include = selected.iter().map(|i| i.rel_path.clone()).collect();
            (Some(include), selected, withheld)
        }
        Selection::Items(paths) => {
            let offered: HashSet<&str> =
                manifest.items.iter().map(|i| i.rel_path.as_str()).collect();
            let unknown: Vec<&str> = paths
                .iter()
                .map(String::as_str)
                .filter(|p| !offered.contains(p))
                .collect();
            if !unknown.is_empty() {
                let mut offered: Vec<&str> = offered.into_iter().collect();
                offered.sort();
                return Err(Error::InvalidSelection(format!(
                    "{} not offered by {}; available: {}",
                    unknown.join(", "),
                    manifest.provider_id,
                    offered.join(", ")
                )));
            }
            let include: HashSet<String> = paths.iter().cloned().collect();
            let selected = manifest
                .items
                .iter()
                .filter(|i| include.contains(&i.rel_path))
                .cloned()
                .collect();
            (Some(include), selected, Vec::new())
        }
    };
    if selected.is_empty() {
        return Err(Error::InvalidSelection(format!(
            "nothing selected from {} ({} items offered)",
            manifest.provider_id,
            manifest.items.len()
        )));
    }
    Ok((include, selected, withheld))
}

/// Where [`upload_bundle`] reads the bundle from.
pub enum BundleSource<'a> {
    /// A bundle in memory.
    Bytes(&'a [u8]),
    /// A bundle in a (temporary) file, read chunk by chunk so a large bundle
    /// never sits in memory.
    File(&'a mut tokio::fs::File),
}

impl BundleSource<'_> {
    async fn len(&mut self) -> Result<u64, Error> {
        Ok(match self {
            BundleSource::Bytes(b) => b.len() as u64,
            BundleSource::File(f) => f
                .metadata()
                .await
                .map_err(cua_spacesd_client::Error::Io)?
                .len(),
        })
    }

    async fn read_at(&mut self, offset: u64, len: usize) -> Result<Vec<u8>, Error> {
        match self {
            BundleSource::Bytes(b) => Ok(b[offset as usize..offset as usize + len].to_vec()),
            BundleSource::File(f) => {
                use tokio::io::{AsyncReadExt, AsyncSeekExt};
                f.seek(std::io::SeekFrom::Start(offset))
                    .await
                    .map_err(cua_spacesd_client::Error::Io)?;
                let mut buf = vec![0u8; len];
                f.read_exact(&mut buf)
                    .await
                    .map_err(cua_spacesd_client::Error::Io)?;
                Ok(buf)
            }
        }
    }
}

/// Uploads a captured bundle to the spacesd behind `env` with
/// `TeleportService.ImportSession`: unary, offset-addressed chunks (so it
/// works over gRPC-Web too), resumed from the guest's offset after a
/// transient failure, committed with the whole-bundle SHA-256 on the last
/// chunk. `provider_id` names the guest importer (the bundle header's
/// provider). Returns the guest's import result.
pub async fn upload_bundle(
    env: &SpacesdClient,
    import_id: &str,
    provider_id: &str,
    scope: TransferScope,
    mut source: BundleSource<'_>,
    sha256: &str,
    options: &SendOptions,
) -> Result<pb::ImportResult, Error> {
    // Seal to the guest's pinned key first, when one is available (S1): a
    // sealed delivery needs no ack and is never refused by the gate below,
    // since the relay only ever sees its ciphertext either way. Only then
    // does an unsealed relay: delivery still need the gate.
    let sealed = seal_for_relay(env, import_id, &mut source).await?;
    let (mut source, sha256) = match &sealed {
        Some((bytes, sha)) => (BundleSource::Bytes(bytes), sha.as_str()),
        None => {
            require_sealed_or_ack(env, options)?;
            (source, sha256)
        }
    };
    let total = source.len().await?;
    let (preferred, max) = env.chunk_limits().await;
    let chunk = options.chunk_bytes.unwrap_or(preferred).min(max).max(1) as u64;
    let policy = env.retry_policy();
    let progress = |sent: u64| {
        if let Some(p) = &options.progress {
            p(sent, total);
        }
    };
    progress(0);
    let mut offset = 0u64;
    let mut failures = 0u32;
    // Hard bound: every chunk once, plus one extra round trip per allowed
    // failure and per offset correction.
    let max_calls = total / chunk + 2 + 2 * u64::from(policy.max_attempts);
    for _ in 0..max_calls {
        let end = (offset + chunk).min(total);
        let commit = end == total;
        if commit {
            // The last chunk carries the import: everything is on its way,
            // and the Space imports while this call runs.
            progress(end);
        }
        let request = pb::ImportSessionRequest {
            import_id: import_id.to_string(),
            app: provider_id.to_string(),
            scope: proto_scope(scope) as i32,
            offset,
            data: source.read_at(offset, (end - offset) as usize).await?,
            commit,
            sha256: if commit {
                sha256.to_string()
            } else {
                String::new()
            },
            options: commit.then_some(pb::ImportOptions {
                replace_existing: options.replace_existing,
                close_running_app: options.close_running_app,
                launch_after: options.launch_after,
                // The broker's TTL travels to the guest so the receiver
                // wipes on expiry even without a host `WipeImport` (0 = none).
                expires_at_ms: options.expires_at_ms,
                // Set by the Keyvault broker; empty for direct sends.
                broker_grant: options.broker_grant.clone().unwrap_or_default(),
            }),
        };
        match env.teleport().import_session(request).await {
            Ok(response) => {
                let response = response.into_inner();
                failures = 0;
                progress(response.received_bytes.min(total));
                if commit {
                    return response.result.ok_or_else(|| {
                        Error::Env(cua_spacesd_client::Error::Protocol(
                            "the final ImportSession chunk returned no result".into(),
                        ))
                    });
                }
                if response.received_bytes > total {
                    return Err(Error::Env(cua_spacesd_client::Error::Protocol(format!(
                        "guest reports {} of {total} bytes",
                        response.received_bytes
                    ))));
                }
                offset = response.received_bytes;
            }
            Err(status) => match cua_spacesd_client::Error::from(status) {
                cua_spacesd_client::Error::OffsetMismatch {
                    expected_offset: Some(expected),
                    ..
                } if expected <= total => offset = expected,
                e if e.is_retryable() && failures + 1 < policy.max_attempts => {
                    failures += 1;
                    tracing::debug!(offset, error = %e, "teleport upload interrupted; retrying");
                    tokio::time::sleep(policy.backoff(failures)).await;
                }
                e => return Err(e.into()),
            },
        }
    }
    Err(Error::Env(cua_spacesd_client::Error::Protocol(format!(
        "teleport upload made no progress after {max_calls} calls"
    ))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(rel: &str, sensitive: bool, default_checked: bool) -> ManifestItem {
        ManifestItem {
            label: rel.into(),
            rel_path: rel.into(),
            est_bytes: 1,
            count: None,
            count_noun: None,
            sensitive,
            default_checked,
        }
    }

    fn manifest() -> TransferManifest {
        TransferManifest {
            provider_id: "p".into(),
            app_display_name: "P".into(),
            scope: TransferScope::FullProfile,
            items: vec![item("a", false, true), item("b", true, false)],
            total_est_bytes: 2,
            notes: vec![],
        }
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn relay_sealing_is_enforced_now_that_sealing_and_the_app_dialog_ship() {
        // Locks in the "launch" state (S1 step 2): end-to-end sealing, the
        // app's relay-plaintext consent dialog, and this flip landed
        // together, so a plain unsealed relay teleport is refused unless
        // acked.
        assert!(RELAY_SEALING_ENFORCED);
    }

    #[test]
    fn gate_relay_delivery_both_states() {
        let relay = EndpointKind::Relay {
            machine_id: "m".into(),
        };
        let direct = EndpointKind::Direct;

        // Enforced: a relay destination without ack is refused; ack, or a
        // non-relay destination, is not.
        assert!(matches!(
            gate_relay_delivery(&relay, false, true),
            Err(Error::RelayUnsealed)
        ));
        assert!(gate_relay_delivery(&relay, true, true).is_ok());
        assert!(gate_relay_delivery(&direct, false, true).is_ok());

        // Not enforced (today's default): a relay destination is let
        // through either way.
        assert!(gate_relay_delivery(&relay, false, false).is_ok());
        assert!(gate_relay_delivery(&relay, true, false).is_ok());
        assert!(gate_relay_delivery(&direct, false, false).is_ok());
    }

    #[test]
    fn default_selection_withholds_unchecked_items() {
        let (include, selected, withheld) =
            resolve_selection(&manifest(), &Selection::Default).unwrap();
        assert_eq!(include.unwrap().into_iter().collect::<Vec<_>>(), ["a"]);
        assert_eq!(selected.len(), 1);
        assert_eq!(withheld[0].rel_path, "b");
    }

    #[test]
    fn all_and_explicit_selections() {
        let (include, selected, _) = resolve_selection(&manifest(), &Selection::All).unwrap();
        assert!(include.is_none());
        assert_eq!(selected.len(), 2);
        let (include, selected, _) =
            resolve_selection(&manifest(), &Selection::Items(vec!["b".into()])).unwrap();
        assert!(include.unwrap().contains("b"));
        assert!(selected[0].sensitive);
    }

    #[test]
    fn unknown_or_empty_selection_is_rejected() {
        let err = resolve_selection(&manifest(), &Selection::Items(vec!["zzz".into()]))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("zzz") && err.contains("available: a, b"),
            "{err}"
        );
        assert!(matches!(
            resolve_selection(&manifest(), &Selection::Items(vec![])),
            Err(Error::InvalidSelection(_))
        ));
    }

    #[test]
    fn scopes_map_to_the_contract() {
        assert_eq!(
            proto_scope(TransferScope::TabsOnly),
            pb::TeleportScope::Session
        );
        assert_eq!(
            proto_scope(TransferScope::FullProfile),
            pb::TeleportScope::Profile
        );
        assert!(random_import_id().starts_with("tp-"));
        assert_ne!(random_import_id(), random_import_id());
    }
}
