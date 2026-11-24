//! Whether a guest's cua-spacesd can become a new machine.
//!
//! A memory snapshot that several Spaces restore (a golden warm start, a
//! warm-pool member made from one) carries the per-machine secrets the
//! guest's cua-spacesd held when it was taken: its machine-seal keypair, its
//! media certificate, its random state. Every restore is told it is a new
//! machine (`Init{new_machine}`), but a cua-spacesd older than protocol
//! revision [`NEW_MACHINE_REVISION`] ignores that, and its restores would
//! share those secrets. Backends therefore record a shared snapshot only
//! for a guest whose cua-spacesd regenerates them; older images start cold,
//! each with secrets of its own.

/// The first cua-spacesd protocol revision that implements
/// `Init{new_machine, entropy}`.
pub const NEW_MACHINE_REVISION: u32 = 10;

/// cua-spacesd's port.
pub const SPACESD_PORT: u16 = 3211;

/// The protocol revision in the JSON of `cua-spacesd build-info` (top
/// level) or of an image manifest (`/etc/cua-image/manifest.json`, under
/// `spacesd`).
pub fn revision_in(json: &str) -> Option<u32> {
    let v: serde_json::Value = serde_json::from_str(json.trim()).ok()?;
    let r = v
        .get("protocol_revision")
        .or_else(|| v.get("spacesd").and_then(|s| s.get("protocol_revision")))?;
    r.as_u64().and_then(|r| u32::try_from(r).ok())
}

/// Whether a cua-spacesd of protocol `revision` (unknown: `None`) renews
/// its per-machine secrets on `Init{new_machine}`.
pub fn renews_identity(revision: Option<u32>) -> bool {
    revision.is_some_and(|r| r >= NEW_MACHINE_REVISION)
}

/// The shell command that prints the guest's cua-spacesd build info (or,
/// failing that, the image manifest that recorded it).
pub const BUILD_INFO_SCRIPT: &str = "cua-spacesd build-info 2>/dev/null \
    || /usr/local/bin/cua-spacesd build-info 2>/dev/null \
    || cat /etc/cua-image/manifest.json 2>/dev/null";

/// The protocol revision of the cua-spacesd answering at `url` (its
/// `GetCapabilities`, which a guest awaiting its token also answers).
#[cfg(any(feature = "container", feature = "microvm", feature = "qemu"))]
pub async fn revision_at(url: &str, token: Option<&str>) -> Option<u32> {
    let c = cua_spacesd_client::SpacesdClient::connect_url(url, token.map(str::to_string))
        .await
        .ok()?;
    let caps = tokio::time::timeout(std::time::Duration::from_secs(10), c.refresh_capabilities())
        .await
        .ok()?
        .ok()?;
    Some(caps.protocol_revision)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_revision_from_build_info_or_a_manifest() {
        assert_eq!(
            revision_in(r#"{"version":"0.6.0","protocol_revision":10}"#),
            Some(10)
        );
        assert_eq!(
            revision_in(r#"{"spacesd":{"present":true,"protocol_revision":9}}"#),
            Some(9)
        );
        assert_eq!(revision_in("not json"), None);
        assert_eq!(revision_in(r#"{"version":"0.5.3"}"#), None);
    }

    #[test]
    fn only_a_spacesd_that_knows_new_machine_may_share_a_snapshot() {
        assert!(renews_identity(Some(NEW_MACHINE_REVISION)));
        assert!(renews_identity(Some(NEW_MACHINE_REVISION + 3)));
        assert!(!renews_identity(Some(NEW_MACHINE_REVISION - 1)));
        assert!(!renews_identity(None));
    }
}
