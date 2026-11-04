//! Calls to a host whose Cua is older than the RPC.
//!
//! A host that predates an RPC does not answer "unimplemented": its access
//! layer rejects a method it does not know as `unauthenticated`, which users
//! read as a permissions problem. A client therefore checks the host's
//! reported protocol revision before calling an RPC added after it
//! ([`cua_proto::HOST_SERVICES_MIN_REVISION`]), and maps what an old host
//! answers anyway to the same refusal.

use crate::{Error, pb};

/// How a host that is too old for the RPC describes itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OldHost {
    /// The host's cua-spacesd version (empty when unknown).
    pub version: String,
    /// The host's protocol revision (0 when unknown).
    pub revision: u32,
}

/// `Some` when `caps` (a host's `GetCapabilities`) report a revision below
/// [`cua_proto::HOST_SERVICES_MIN_REVISION`]. A host that reports none (0:
/// empty capabilities, no cua-spacesd) is not judged here.
pub fn too_old(caps: &pb::GetCapabilitiesResponse) -> Option<OldHost> {
    (caps.protocol_revision != 0 && caps.protocol_revision < cua_proto::HOST_SERVICES_MIN_REVISION)
        .then(|| OldHost {
            version: caps.version.clone(),
            revision: caps.protocol_revision,
        })
}

/// Whether `e` is what a host answers for an RPC it does not have:
/// `unimplemented`, or `unauthenticated` from an access layer that rejects
/// unknown methods that way.
pub fn is_unknown_rpc(e: &Error) -> bool {
    match e {
        Error::Unauthenticated(_) => true,
        Error::Rpc(d) => d.code == tonic::Code::Unimplemented as i32,
        _ => false,
    }
}

/// The refusal for `what` (for example "checkpoints of Spaces") on `host`,
/// which `old` describes (`None`: only its answer showed it is old).
pub fn message(host: &str, old: Option<&OldHost>, what: &str) -> String {
    let who = match old {
        Some(o) if !o.version.is_empty() => {
            format!(
                "{host} runs an older Cua (cua-spacesd {} / revision {})",
                o.version, o.revision
            )
        }
        Some(o) => format!("{host} runs an older Cua (revision {})", o.revision),
        None => format!("{host} runs an older Cua"),
    };
    format!(
        "{who}; {what} there need Cua 0.8 or newer on that machine. Update it with \
         `curl -fsSL https://cua.ai/install.sh | sh` there."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(rev: u32) -> pb::GetCapabilitiesResponse {
        pb::GetCapabilitiesResponse {
            version: "0.6.0".into(),
            protocol_revision: rev,
            ..Default::default()
        }
    }

    #[test]
    fn revision_8_is_too_old_and_9_is_not() {
        let o = too_old(&caps(8)).expect("0.7 host");
        assert_eq!(o.revision, 8);
        assert_eq!(o.version, "0.6.0");
        assert!(too_old(&caps(cua_proto::HOST_SERVICES_MIN_REVISION)).is_none());
        assert!(too_old(&caps(cua_proto::ENV_PROTOCOL_REVISION)).is_none());
    }

    #[test]
    fn unknown_revision_is_not_judged() {
        assert!(too_old(&caps(0)).is_none());
    }

    #[test]
    fn min_revision_is_within_the_current_one() {
        const { assert!(cua_proto::HOST_SERVICES_MIN_REVISION <= cua_proto::ENV_PROTOCOL_REVISION) };
    }

    #[test]
    fn message_names_host_version_and_the_update() {
        let m = message(
            "ip-172-31-46-222",
            too_old(&caps(8)).as_ref(),
            "checkpoints of Spaces",
        );
        assert_eq!(
            m,
            "ip-172-31-46-222 runs an older Cua (cua-spacesd 0.6.0 / revision 8); \
             checkpoints of Spaces there need Cua 0.8 or newer on that machine. Update it with \
             `curl -fsSL https://cua.ai/install.sh | sh` there."
        );
        assert!(message("h", None, "x").starts_with("h runs an older Cua; x there"));
    }

    #[test]
    fn unknown_rpc_is_unauthenticated_or_unimplemented() {
        assert!(is_unknown_rpc(&Error::from(
            tonic::Status::unauthenticated(
                "cannot call /cua.env.v1.HostSnapshotsService/CreateHostSnapshot"
            )
        )));
        assert!(is_unknown_rpc(&Error::from(tonic::Status::unimplemented(
            "no"
        ))));
        assert!(!is_unknown_rpc(&Error::from(tonic::Status::not_found(
            "no"
        ))));
        assert!(!is_unknown_rpc(&Error::from(
            tonic::Status::permission_denied("no")
        )));
    }
}
