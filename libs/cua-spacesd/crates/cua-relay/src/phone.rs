// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! What a phone may do through the relay (account mode, `CUA_RELAY_PUSH=on`).
//!
//! A phone is a device the account enrolled from the Cua Spaces phone app. It
//! is a *smaller* kind of device than a computer: it reaches a machine only
//! through a short list of calls (read and post in a thread, start a task,
//! look at the screen, ask for a viewer ticket), never a shell, the
//! filesystem, a tunnel or any plain HTTP route. A desktop a computer shares as a
//! Space is a Space: its picture, and its pointer and keyboard when the
//! account's role may touch it, are a phone's like any app's; the relay forwards
//! those calls and the machine decides by the same rule as for every app.
//!
//! The list is enforced twice: here, before a call is forwarded, and again by
//! the machine (`cua-spacesd-server`), which reads the same constants.
//!
//! A device is a phone when it says so (`platform` `ios` or `android`) or when
//! its sign-in token was issued to one of the phone client ids
//! ([`DEFAULT_PHONE_CLIENT`]); the flag is sticky, so a phone cannot register
//! again as a computer.

/// The `CUA_RELAY_TRUSTED_PROXIES` value that says, on purpose, there is no proxy
/// (clients reach the relay directly): an empty list is a mistake, this is not.
pub const NO_PROXIES: &str = "none";

/// The proxy list a setting means (`none` is the empty list).
pub fn proxies_setting(value: &str) -> &str {
    if value.trim().eq_ignore_ascii_case(NO_PROXIES) {
        ""
    } else {
        value
    }
}

/// The settings a public relay with phone support must have. `public_url` is the
/// relay's own address: unset or https it is public. Local runs (plain http) are not held to it.
pub fn check_secure_config(
    public_url: Option<&str>,
    authorized_parties: &str,
    trusted_proxies: &str,
) -> Result<(), String> {
    // Only a relay that says it is plain http (a local run) is let off; one that says
    // https, or says nothing (TLS ends at an ingress that was not told to the relay),
    // is public.
    if public_url.is_some_and(|u| u.trim().to_ascii_lowercase().starts_with("http://")) {
        return Ok(());
    }
    if authorized_parties.split(',').all(|p| p.trim().is_empty()) {
        return Err("CUA_RELAY_PUSH=on on a public relay needs CUA_RELAY_OIDC_AUTHORIZED_PARTIES: the OAuth clients allowed to sign in (a token for any other client of the issuer must be refused)".into());
    }
    if trusted_proxies.trim().is_empty() {
        return Err("CUA_RELAY_PUSH=on on a public relay needs CUA_RELAY_TRUSTED_PROXIES: the ingress addresses (the ingress must overwrite X-Forwarded-For), or `none` when clients reach the relay directly".into());
    }
    Ok(())
}

/// The OAuth client id of the phone app.
pub const DEFAULT_PHONE_CLIENT: &str = "cua-spaces-mobile";

/// The `dcls` claim of an assertion for a phone: the device class.
pub const CLASS_MOBILE: &str = "mobile";

/// Whether a reported platform is a phone's.
pub fn platform_is_phone(platform: &str) -> bool {
    matches!(
        platform.trim().to_ascii_lowercase().as_str(),
        "ios" | "ipados" | "android"
    )
}

/// The calls a phone may make without touching the desktop. Everything else
/// is refused, whatever the account is allowed to do from a computer.
pub const PHONE_GRPC_METHODS: &[&str] = &[
    "/cua.env.v1.SystemService/GetCapabilities",
    "/cua.env.v1.SystemService/Health",
    // A view-only viewer ticket for the live desktop (see the machine: a
    // phone without control gets a short, view-only ticket).
    "/cua.env.v1.SystemService/CreateViewerTicket",
    "/cua.env.v1.ComputerService/ListDisplays",
    "/cua.env.v1.ComputerService/Screenshot",
    // The Space's thread: read, post, start a task, answer approvals.
    "/cua.env.v1.HostThreadService/CallThreadTool",
];

/// The calls that touch a desktop shared as a Space. A phone makes them on a
/// computer that shares its desktop, with a role that may touch it.
pub const PHONE_CONTROL_GRPC_METHODS: &[&str] = &[
    "/cua.env.v1.ComputerService/Pointer",
    "/cua.env.v1.ComputerService/Keyboard",
];

/// The gRPC methods a viewer ticket may call (the machine's
/// `cua_proto::metadata::VIEWER_GRPC_METHODS`, which a test keeps equal). The
/// relay forwards a header-ticket call only for these.
pub const VIEWER_GRPC_METHODS: &[&str] = &[
    "/cua.env.v1.SystemService/GetCapabilities",
    "/cua.env.v1.SystemService/Health",
    "/cua.env.v1.StreamService/ListTargets",
    "/cua.env.v1.StreamService/OpenMedia",
    "/cua.env.v1.StreamService/SetPreferences",
    "/cua.env.v1.StreamService/RequestKeyframe",
    "/cua.env.v1.StreamService/CloseMedia",
    "/cua.env.v1.ComputerService/ListDisplays",
    "/cua.env.v1.ComputerService/GetClipboard",
    "/cua.env.v1.ComputerService/SetClipboard",
    "/cua.env.v1.PresenceService/Join",
    "/cua.env.v1.PresenceService/UpdateCursor",
    "/cua.env.v1.PresenceService/Leave",
    "/cua.env.v1.FilesystemService/Stat",
    "/cua.env.v1.FilesystemService/ListDir",
    "/cua.env.v1.FilesystemService/MakeDir",
    "/cua.env.v1.FilesystemService/Move",
    "/cua.env.v1.FilesystemService/Remove",
    "/cua.env.v1.FilesystemService/CreateWatcher",
    "/cua.env.v1.FilesystemService/GetWatcherEvents",
    "/cua.env.v1.FilesystemService/RemoveWatcher",
    "/cua.env.v1.FilesystemService/ReadFile",
    "/cua.env.v1.FilesystemService/BeginUpload",
    "/cua.env.v1.FilesystemService/UploadChunk",
    "/cua.env.v1.FilesystemService/CommitUpload",
    "/cua.env.v1.FilesystemService/AbortUpload",
];

/// What a pool member says it can host (the `capabilities` of an enrollment,
/// versioned and additive; the apps read it, the relay only keeps it).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Capabilities {
    /// Format version (1).
    #[serde(default)]
    pub v: u32,
    /// It runs Spaces for other members.
    #[serde(default)]
    pub can_host_spaces: bool,
    /// The images it will run (`agent-home`), or `*`.
    #[serde(default)]
    pub images: Vec<String>,
    /// The kinds of Space it runs (`container`, `vm`).
    #[serde(default)]
    pub kinds: Vec<String>,
    /// It has a screen to run desktop Spaces on.
    #[serde(default)]
    pub desktop: bool,
}

impl Capabilities {
    /// Checks what an enrollment states: a small, well-formed list, nothing
    /// to store that is not a short word.
    pub fn check(&self) -> Result<(), String> {
        let word = |s: &str, max: usize| {
            !s.is_empty()
                && s.len() <= max
                && s.chars().all(|c| {
                    c.is_ascii_alphanumeric()
                        || matches!(c, '.' | '_' | ':' | '/' | '@' | '-' | '*')
                })
        };
        if self.v == 0 || self.v > 8 {
            return Err("capabilities: v must be a small number".into());
        }
        if self.images.len() > 16 || !self.images.iter().all(|i| word(i, 64)) {
            return Err("capabilities: images are at most 16 short names".into());
        }
        if self.kinds.len() > 8 || !self.kinds.iter().all(|k| word(k, 16)) {
            return Err("capabilities: kinds are at most 8 short words".into());
        }
        Ok(())
    }
}

/// How the relay sees a path a phone asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhonePath {
    /// A call every phone may make.
    Allowed,
    /// A call that needs the owner's control approval on the machine.
    Control,
    /// Not for a phone at all.
    Refused,
}

/// Classifies `forwarded` (the path on the machine, with or without a query)
/// requested with `method`. Only exact gRPC method paths, only POST.
pub fn classify(method: &http::Method, forwarded: &str) -> PhonePath {
    if *method != http::Method::POST {
        return PhonePath::Refused;
    }
    let path = forwarded.split('?').next().unwrap_or(forwarded);
    if PHONE_GRPC_METHODS.contains(&path) {
        PhonePath::Allowed
    } else if PHONE_CONTROL_GRPC_METHODS.contains(&path) {
        PhonePath::Control
    } else {
        PhonePath::Refused
    }
}

/// The call that mints a viewer ticket.
pub const CREATE_VIEWER_TICKET: &str = "/cua.env.v1.SystemService/CreateViewerTicket";

/// The longest a phone's viewer ticket lives, whatever it asks.
pub const PHONE_TICKET_MAX_SECS: u64 = 300;

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8 & 0x7f) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn get_varint(b: &[u8], i: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = *b.get(*i)?;
        *i += 1;
        v |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

/// The seconds a `CreateViewerTicketRequest` payload asks for (field 1, a
/// `Duration`), if it says.
fn asked_seconds(payload: &[u8]) -> Option<u64> {
    let mut i = 0;
    while i < payload.len() {
        let tag = get_varint(payload, &mut i)?;
        match tag & 7 {
            0 => {
                get_varint(payload, &mut i)?;
            }
            1 => i = i.checked_add(8)?,
            5 => i = i.checked_add(4)?,
            2 => {
                let len = get_varint(payload, &mut i)? as usize;
                let end = i.checked_add(len)?;
                let inner = payload.get(i..end)?;
                if tag >> 3 == 1 {
                    let mut j = 0;
                    while j < inner.len() {
                        let t = get_varint(inner, &mut j)?;
                        if t == 0x08 {
                            return get_varint(inner, &mut j);
                        }
                        match t & 7 {
                            0 => {
                                get_varint(inner, &mut j)?;
                            }
                            _ => return None,
                        }
                    }
                }
                i = end;
            }
            _ => return None,
        }
    }
    None
}

/// What a phone's viewer ticket request is made into, whatever it said: view only,
/// at most [`PHONE_TICKET_MAX_SECS`], and nothing else (no clipboard, no files, no
/// microphone, no chosen principal). Only the lifetime it asked for (when shorter)
/// is kept.
pub fn clamped_ticket_payload(request: &[u8]) -> Vec<u8> {
    let secs = asked_seconds(request)
        .filter(|s| *s > 0)
        .unwrap_or(PHONE_TICKET_MAX_SECS)
        .min(PHONE_TICKET_MAX_SECS);
    let mut duration = vec![0x08];
    put_varint(&mut duration, secs);
    let mut out = vec![0x0a, duration.len() as u8];
    out.extend(duration);
    // policy = SESSION_POLICY_VIEW_ONLY
    out.extend([0x10, 0x01]);
    out
}

/// Rewrites the body of a phone's `CreateViewerTicket` call to
/// [`clamped_ticket_payload`], before it is forwarded: a computer that predates
/// phones, or one that has a bug, never sees what the phone asked for. gRPC and
/// gRPC-Web, binary and text; anything else is refused.
pub async fn clamp_ticket_request(
    request: &mut axum::extract::Request,
) -> Result<(), &'static str> {
    use base64::Engine as _;
    use http_body_util::BodyExt as _;
    let content_type = request
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !content_type.starts_with("application/grpc") {
        return Err("a viewer ticket is asked for with gRPC");
    }
    let text = content_type.starts_with("application/grpc-web-text");
    let body = std::mem::take(request.body_mut());
    let bytes = http_body_util::Limited::new(body, 16 * 1024)
        .collect()
        .await
        .map_err(|_| "the ticket request is too large")?
        .to_bytes();
    let engine = base64::engine::general_purpose::STANDARD;
    let raw = if text {
        engine
            .decode(&bytes)
            .map_err(|_| "the ticket request is not base64")?
    } else {
        bytes.to_vec()
    };
    let payload = if raw.len() >= 5 && raw[0] == 0 {
        let len = u32::from_be_bytes([raw[1], raw[2], raw[3], raw[4]]) as usize;
        raw.get(5..5 + len)
            .ok_or("the ticket request is cut short")?
            .to_vec()
    } else if raw.is_empty() {
        Vec::new()
    } else {
        return Err("the ticket request is not a gRPC message");
    };
    let clamped = clamped_ticket_payload(&payload);
    let mut framed = vec![0u8];
    framed.extend((clamped.len() as u32).to_be_bytes());
    framed.extend(clamped);
    let out = if text {
        engine.encode(&framed).into_bytes()
    } else {
        framed
    };
    request.headers_mut().remove(http::header::CONTENT_LENGTH);
    *request.body_mut() = axum::body::Body::from(out);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phone_reaches_the_thread_and_the_screen_and_nothing_that_runs_code() {
        let post = http::Method::POST;
        assert_eq!(
            classify(&post, "/cua.env.v1.HostThreadService/CallThreadTool"),
            PhonePath::Allowed
        );
        assert_eq!(
            classify(&post, "/cua.env.v1.ComputerService/Pointer"),
            PhonePath::Control
        );
        for refused in [
            "/cua.env.v1.ComputerService/SetClipboard",
            "/cua.env.v1.ComputerService/GetClipboard",
            "/cua.env.v1.ProcessService/Run",
            "/cua.env.v1.FilesystemService/WriteFile",
            "/cua.env.v1.TunnelService/Forward",
            "/cua.env.v1.HostSpacesService/CreateHostSpace",
            "/files?path=/etc/passwd",
            "/viewer/",
            "/cua.env.v1.HostThreadService/CallThreadTool/../ProcessService/Run",
            "/cua.env.v1.HostThreadService/CallThreadToolX",
        ] {
            assert_eq!(classify(&post, refused), PhonePath::Refused, "{refused}");
        }
        // Only POST: a gRPC call is never a GET.
        assert_eq!(
            classify(
                &http::Method::GET,
                "/cua.env.v1.HostThreadService/CallThreadTool"
            ),
            PhonePath::Refused
        );
    }

    #[test]
    fn the_viewer_list_is_the_machines() {
        assert_eq!(
            VIEWER_GRPC_METHODS,
            cua_proto::metadata::VIEWER_GRPC_METHODS
        );
    }

    #[test]
    fn phones_are_recognized_by_platform() {
        assert!(platform_is_phone("iOS"));
        assert!(platform_is_phone(" android "));
        assert!(!platform_is_phone("macos"));
        assert!(!platform_is_phone(""));
    }

    #[test]
    fn a_public_relay_with_phones_starts_only_with_its_clients_and_its_proxies_said() {
        let https = Some("https://relay.example.com");
        let party = "cua-cli,cua-spaces-mobile";
        assert!(check_secure_config(https, party, "10.0.0.0/8").is_ok());
        assert!(
            check_secure_config(https, party, "none").is_ok(),
            "no proxy, said on purpose"
        );
        let e = check_secure_config(https, "", "10.0.0.0/8").unwrap_err();
        assert!(e.contains("CUA_RELAY_OIDC_AUTHORIZED_PARTIES"), "{e}");
        assert!(check_secure_config(https, " , ", "none").is_err());
        let e = check_secure_config(https, party, "").unwrap_err();
        assert!(
            e.contains("CUA_RELAY_TRUSTED_PROXIES") && e.contains("`none`"),
            "{e}"
        );
        // A local run (plain http) is not held to it; a relay that names no address is.
        assert!(check_secure_config(Some("http://127.0.0.1:8080"), "", "").is_ok());
        let e = check_secure_config(None, "", "").unwrap_err();
        assert!(e.contains("CUA_RELAY_OIDC_AUTHORIZED_PARTIES"), "{e}");
        assert!(check_secure_config(None, "cua-cli", "").is_err());
        assert!(check_secure_config(None, "cua-cli", "none").is_ok());
        assert!(check_secure_config(Some(""), "", "").is_err());
        assert_eq!(proxies_setting("none"), "");
        assert_eq!(proxies_setting(" None "), "");
        assert_eq!(proxies_setting("10.0.0.0/8"), "10.0.0.0/8");
    }

    #[test]
    fn a_phones_ticket_request_becomes_view_only_and_short_whatever_it_asked() {
        // policy 3, clipboard, files_root, microphone, 24 h: the S9 request.
        let mut asked = vec![0x0a, 0x04, 0x08, 0x80, 0xa3, 0x05]; // ttl 86400 s
        asked.extend([0x10, 0x03, 0x18, 0x01]);
        asked.extend([0x22, 0x0b]);
        asked.extend(b"/Users/ada/");
        asked.extend([0x28, 0x01]);
        assert_eq!(
            clamped_ticket_payload(&asked),
            vec![0x0a, 0x03, 0x08, 0xac, 0x02, 0x10, 0x01],
            "300 s, view only, nothing else"
        );
        // A shorter wish stays; none, zero or a damaged request is the maximum.
        let short = [0x0a, 0x02, 0x08, 0x1e];
        assert_eq!(
            clamped_ticket_payload(&short),
            vec![0x0a, 0x02, 0x08, 0x1e, 0x10, 0x01]
        );
        for odd in [
            &[][..],
            &[0x0a, 0x00][..],
            &[0xff, 0xff][..],
            &[0x0a, 0x09, 0x08][..],
        ] {
            assert_eq!(
                clamped_ticket_payload(odd),
                vec![0x0a, 0x03, 0x08, 0xac, 0x02, 0x10, 0x01],
                "{odd:?}"
            );
        }
    }
}
