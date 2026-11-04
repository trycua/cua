//! Pure selection rules for attributing an X11 toplevel to a local process
//! when the client does not publish `_NET_WM_PID`.
//!
//! Tk, many Java/AWT builds, Wine, and legacy Xlib/Xt clients never set
//! `_NET_WM_PID`. The X-Resource extension (`XResQueryClientIds` with
//! `XRES_CLIENT_ID_PID_MASK`, XRes >= 1.2) lets the X server report the PID
//! of the connection that created a resource, which it learns from the Unix
//! socket peer credentials. That answer is only trustworthy when all of the
//! following hold, and these rules fail closed otherwise:
//!
//! - the server reports our own connection's PID as our real PID, which proves
//!   the driver is a local client in the same PID namespace as the server (a
//!   remote `DISPLAY`, a TCP connection, or a container boundary fails this);
//! - the server reports exactly one PID for the client that created the
//!   window, never the server itself (client 0 owns the root window);
//! - the window does not name a different host in `WM_CLIENT_MACHINE`, which
//!   is how an SSH-forwarded client (whose socket peer is the local `ssh`
//!   process, not the application) identifies itself.
//!
//! The X11 I/O lives in `crate::x11`; everything here is platform-independent
//! so it is unit-tested on every host.

/// `XRES_CLIENT_ID_PID_MASK` (X-Resource 1.2, `LocalClientPID`).
pub(crate) const LOCAL_CLIENT_PID_MASK: u32 = 1 << 1;

/// One `ClientIdValue` of an `XResQueryClientIds` reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct XresClientValue<'a> {
    /// The client's resource-id base (`clientAsMask`), as echoed by the server.
    pub client: u32,
    pub mask: u32,
    pub value: &'a [u32],
}

/// The XRes version that introduced `QueryClientIds`.
pub(crate) fn xres_supports_client_ids(major: u16, minor: u16) -> bool {
    (major, minor) >= (1, 2)
}

/// Resource-id base of the client that created `xid`: the server allocates
/// every client a disjoint id range, so clearing the client-local bits of
/// `resource_id_mask` (from the connection setup) names the creator.
pub(crate) fn client_base(xid: u32, resource_id_mask: u32) -> u32 {
    xid & !resource_id_mask
}

/// The PID the server reported for `client`, if it reported exactly one.
fn reported_pid(values: &[XresClientValue<'_>], client: u32) -> Option<u32> {
    let mut pids = values
        .iter()
        .filter(|v| v.client == client && v.mask == LOCAL_CLIENT_PID_MASK)
        .map(|v| match v.value {
            [pid] if *pid != 0 && i32::try_from(*pid).is_ok() => Some(*pid),
            _ => None,
        });
    let first = pids.next()??;
    pids.next().is_none().then_some(first)
}

/// Select the owner PID of the window created by the client at `target_base`
/// from a reply that also covered our own client at `own_base`.
///
/// Returns `None` unless the server reported `own_pid` for our own
/// connection (so its PIDs are local and in our namespace) and exactly one
/// valid PID for the target client. The server's own resources (client base
/// 0, e.g. the root window) are never attributed.
pub(crate) fn select_owner_pid(
    values: &[XresClientValue<'_>],
    own_base: u32,
    own_pid: u32,
    target_base: u32,
) -> Option<u32> {
    if target_base == 0 || reported_pid(values, own_base) != Some(own_pid) {
        return None;
    }
    reported_pid(values, target_base)
}

/// Whether `WM_CLIENT_MACHINE` allows a local-socket PID to stand for the
/// application. An absent or empty property is accepted (most clients that
/// omit `_NET_WM_PID` omit it too); a present one must name this host, and an
/// unknown local hostname fails closed.
pub(crate) fn client_machine_is_local(machine: Option<&str>, local_hostname: Option<&str>) -> bool {
    let Some(machine) = machine.map(str::trim).filter(|m| !m.is_empty()) else {
        return true;
    };
    let Some(local) = local_hostname.map(str::trim).filter(|h| !h.is_empty()) else {
        return false;
    };
    let short = |name: &str| name.split('.').next().unwrap_or(name).to_ascii_lowercase();
    machine.eq_ignore_ascii_case(local) || short(machine) == short(local)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWN_BASE: u32 = 0x0060_0000;
    const OWN_PID: u32 = 4242;
    const TK_BASE: u32 = 0x0120_0000;
    const MASK: u32 = 0x001f_ffff;

    #[test]
    fn a_local_client_without_net_wm_pid_is_attributed_to_its_creator() {
        let own = [OWN_PID];
        let tk = [777];
        let values = [
            XresClientValue {
                client: OWN_BASE,
                mask: LOCAL_CLIENT_PID_MASK,
                value: &own,
            },
            XresClientValue {
                client: TK_BASE,
                mask: LOCAL_CLIENT_PID_MASK,
                value: &tk,
            },
        ];
        let window = TK_BASE | 0x0000_0017;
        assert_eq!(client_base(window, MASK), TK_BASE);
        assert_eq!(
            select_owner_pid(&values, OWN_BASE, OWN_PID, client_base(window, MASK)),
            Some(777)
        );
    }

    #[test]
    fn a_remote_or_forwarded_target_without_a_pid_value_stays_unattributed() {
        // The server returns no LocalClientPID value for TCP clients.
        let own = [OWN_PID];
        let values = [XresClientValue {
            client: OWN_BASE,
            mask: LOCAL_CLIENT_PID_MASK,
            value: &own,
        }];
        assert_eq!(select_owner_pid(&values, OWN_BASE, OWN_PID, TK_BASE), None);
    }

    #[test]
    fn a_server_in_another_pid_namespace_or_host_is_never_trusted() {
        // Our own connection reports a different PID (container boundary) or
        // none at all (the driver itself reached the server over TCP).
        let own = [1];
        let tk = [777];
        let foreign = [
            XresClientValue {
                client: OWN_BASE,
                mask: LOCAL_CLIENT_PID_MASK,
                value: &own,
            },
            XresClientValue {
                client: TK_BASE,
                mask: LOCAL_CLIENT_PID_MASK,
                value: &tk,
            },
        ];
        assert_eq!(select_owner_pid(&foreign, OWN_BASE, OWN_PID, TK_BASE), None);
        let remote = [XresClientValue {
            client: TK_BASE,
            mask: LOCAL_CLIENT_PID_MASK,
            value: &tk,
        }];
        assert_eq!(select_owner_pid(&remote, OWN_BASE, OWN_PID, TK_BASE), None);
    }

    #[test]
    fn ambiguous_malformed_or_invalid_pid_values_fail_closed() {
        let own = [OWN_PID];
        let a = [777];
        let b = [778];
        let two = [777, 778];
        let zero = [0];
        let negative = [u32::MAX];
        let base = XresClientValue {
            client: OWN_BASE,
            mask: LOCAL_CLIENT_PID_MASK,
            value: &own,
        };
        for target in [&two[..], &zero[..], &negative[..], &[][..]] {
            let values = [
                base,
                XresClientValue {
                    client: TK_BASE,
                    mask: LOCAL_CLIENT_PID_MASK,
                    value: target,
                },
            ];
            assert_eq!(
                select_owner_pid(&values, OWN_BASE, OWN_PID, TK_BASE),
                None,
                "{target:?}"
            );
        }
        let duplicated = [
            base,
            XresClientValue {
                client: TK_BASE,
                mask: LOCAL_CLIENT_PID_MASK,
                value: &a,
            },
            XresClientValue {
                client: TK_BASE,
                mask: LOCAL_CLIENT_PID_MASK,
                value: &b,
            },
        ];
        assert_eq!(
            select_owner_pid(&duplicated, OWN_BASE, OWN_PID, TK_BASE),
            None
        );
        // A CLIENT_XID value is not a PID.
        let wrong_mask = [
            base,
            XresClientValue {
                client: TK_BASE,
                mask: 1,
                value: &a,
            },
        ];
        assert_eq!(
            select_owner_pid(&wrong_mask, OWN_BASE, OWN_PID, TK_BASE),
            None
        );
    }

    #[test]
    fn server_owned_resources_are_never_attributed() {
        let own = [OWN_PID];
        let server = [1];
        let values = [
            XresClientValue {
                client: OWN_BASE,
                mask: LOCAL_CLIENT_PID_MASK,
                value: &own,
            },
            XresClientValue {
                client: 0,
                mask: LOCAL_CLIENT_PID_MASK,
                value: &server,
            },
        ];
        assert_eq!(select_owner_pid(&values, OWN_BASE, OWN_PID, 0), None);
    }

    #[test]
    fn our_own_window_resolves_to_our_pid() {
        let own = [OWN_PID];
        let values = [XresClientValue {
            client: OWN_BASE,
            mask: LOCAL_CLIENT_PID_MASK,
            value: &own,
        }];
        assert_eq!(
            select_owner_pid(&values, OWN_BASE, OWN_PID, OWN_BASE),
            Some(OWN_PID)
        );
    }

    #[test]
    fn query_client_ids_requires_xres_1_2() {
        assert!(!xres_supports_client_ids(1, 0));
        assert!(!xres_supports_client_ids(1, 1));
        assert!(xres_supports_client_ids(1, 2));
        assert!(xres_supports_client_ids(2, 0));
    }

    #[test]
    fn wm_client_machine_must_be_absent_or_this_host() {
        assert!(client_machine_is_local(None, Some("ci-runner")));
        assert!(client_machine_is_local(Some(""), None));
        assert!(client_machine_is_local(
            Some("ci-runner"),
            Some("ci-runner")
        ));
        assert!(client_machine_is_local(
            Some("CI-Runner.example.com"),
            Some("ci-runner")
        ));
        assert!(client_machine_is_local(
            Some("ci-runner"),
            Some("ci-runner.local\n")
        ));
        // An SSH-forwarded client names its remote host.
        assert!(!client_machine_is_local(
            Some("build-box"),
            Some("ci-runner")
        ));
        // Unknown local hostname: cannot prove a named machine is local.
        assert!(!client_machine_is_local(Some("ci-runner"), None));
    }
}
