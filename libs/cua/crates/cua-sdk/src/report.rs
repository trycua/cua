//! What a script or a person should see for a [`CuaError`]: a stable
//! `code`, one plain sentence (`message`) and one next step (`hint`).
//!
//! The raw text of an error is a chain of layers (`local runtime: [local]
//! container engine: pull x: Docker responded with status code 404: ...`)
//! that helps whoever wrote the code and nobody else. [`report_of`] keeps the
//! raw text in `detail` (the CLI shows it with `-v`) and builds the rest:
//!
//! - the variant's own prefix and the layers below it are dropped;
//! - Rust debug structs and source-tree paths never reach `message` or
//!   `hint`;
//! - the common causes (an unknown image, a registry that refuses, no
//!   network, no AWS credentials, a sandbox name that is taken, a sandbox
//!   whose services never answered) get a sentence of their own and a
//!   concrete next step;
//! - every other error gets the `Fix:` line from the variant's doc comment
//!   in `lib.rs` (a test keeps the two in step).
//!
//! Bindings call [`error_report`]; the `cua` CLI prints `error:` and `fix:`
//! lines, or the `--json` object, from the same report.

use crate::CuaError;

/// The shape of an error for people and scripts. Not an exception: see
/// [`CuaError`] for that.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ErrorReport {
    /// A stable lower-snake-case id (`not_found`, `already_exists`,
    /// `image_not_found`). The `cua` CLI documents the full list with its
    /// exit codes.
    pub code: String,
    /// One plain sentence, without internal layers, structs or paths.
    pub message: String,
    /// The next step, one sentence (`None` when there is nothing to add).
    pub hint: Option<String>,
    /// The raw text of the error, layers and all, when it differs from
    /// `message`.
    pub detail: Option<String>,
}

/// Every variant, with its code, the prefix its `Display` adds and the
/// first sentence of its `Fix:` doc.
pub(crate) const VARIANTS: &[Variant] = &[
    Variant::new(
        "InvalidArgument",
        "invalid_argument",
        "invalid argument: ",
        "Run the command with `--help` (or check the reference) to see what each option accepts",
    ),
    Variant::new(
        "InvalidPlacement",
        "invalid_placement",
        "invalid placement: ",
        "Use one of the values the message lists, or leave `kind` and `runtime` unset (`auto`)",
    ),
    Variant::new(
        "NotFound",
        "not_found",
        "not found: ",
        "Run `cua sb ls` to see the sandboxes you can use, and check the ref, path or id",
    ),
    Variant::new(
        "ProviderNotConfigured",
        "provider_not_configured",
        "provider not configured: ",
        "Set the credentials the message names (for a contrib provider, its environment variable or `cua auth provider set <name>`)",
    ),
    Variant::new(
        "Unsupported",
        "unsupported",
        "unsupported: ",
        "Use a provider, image or runtime that supports the call; the message names what is missing",
    ),
    Variant::new(
        "SpacesdNotAvailable",
        "spacesd_not_available",
        "cua-spacesd is not available: ",
        "Use an image that runs cua-spacesd (the canonical Linux image does), or reach the sandbox through the services it declares",
    ),
    Variant::new(
        "Timeout",
        "timeout",
        "timed out: ",
        "Raise the timeout (for example `ready_timeout_ms`), and check that the sandbox or service actually starts",
    ),
    Variant::new(
        "Runtime",
        "runtime",
        "local runtime: ",
        "Run `cua runtime doctor` and follow the steps it prints",
    ),
    Variant::new(
        "Env",
        "env",
        "env: ",
        "Read the status in the message; `SpacesdClient.capabilities` tells whether this spacesd supports the call",
    ),
    Variant::new(
        "Http",
        "http",
        "http: ",
        "Check that the service listens on its declared port and answers HTTP",
    ),
    Variant::new(
        "Unauthenticated",
        "unauthenticated",
        "unauthenticated: ",
        "Pass the sandbox's env token, or run `cua auth login` again",
    ),
    Variant::new(
        "PermissionDenied",
        "permission_denied",
        "permission denied: ",
        "Run the call as a guest user allowed to do it, or change the permissions of the path",
    ),
    Variant::new(
        "Transport",
        "transport",
        "transport: ",
        "Check the address; for a daemon client, start the daemon with `cua daemon start`",
    ),
    Variant::new(
        "DaemonNotRunning",
        "daemon_not_running",
        "",
        "Start Cua (the Spaces app starts the daemon), or run `cua daemon start`",
    ),
    Variant::new(
        "Closed",
        "closed",
        "closed: ",
        "Open a new handle or session; a closed one stays closed",
    ),
    Variant::new(
        "CapabilityMissing",
        "capability_missing",
        "capability missing: ",
        "Use a Space whose spacesd supports the feature (`Space.supports`), or update cua-spacesd on it",
    ),
    Variant::new(
        "HostCapabilityMissing",
        "host_capability_missing",
        "host capability missing: ",
        "Install or start the prerequisite the message names on this computer",
    ),
    Variant::new(
        "TeleportRefused",
        "teleport_refused",
        "teleport refused: ",
        "Approve the manifest in the consent callback, or teleport a smaller scope",
    ),
    Variant::new(
        "Internal",
        "internal",
        "internal: ",
        "Report it with the message at https://github.com/trycua/cua/issues",
    ),
    Variant::new(
        "AmbiguousSandbox",
        "ambiguous_sandbox",
        "ambiguous sandbox name: ",
        "Use one of the qualified refs the message lists (`local:<name>`, `cloud:<name>`)",
    ),
    Variant::new(
        "InsufficientDisk",
        "insufficient_disk",
        "insufficient disk: ",
        "Free space with `cua cache prune`, or lower `CUA_DISK_MIN_FREE`",
    ),
    Variant::new(
        "ImageNotPublished",
        "image_not_published",
        "image not published: ",
        "Pass the reference explicitly (`Image.from_registry(\"<ref>\")`, `cua sb create <ref>`) to use it anyway, or pick a published image (`cua images ls`)",
    ),
    Variant::new(
        "Cancelled",
        "cancelled",
        "cancelled: ",
        "Nothing to clean up; create it again when you want it",
    ),
    Variant::new(
        "Cloud",
        "cloud",
        "your cloud: ",
        "Fix what the message names in that account (`cua cloud test <provider>` checks it without creating anything), or connect another region or project",
    ),
    Variant::new(
        "AlreadyExists",
        "already_exists",
        "",
        "Pick another name, or delete the existing sandbox first with `cua sb rm <name>`",
    ),
];

/// One row of [`VARIANTS`].
pub(crate) struct Variant {
    pub name: &'static str,
    pub code: &'static str,
    pub prefix: &'static str,
    pub fix: &'static str,
}

impl Variant {
    const fn new(
        name: &'static str,
        code: &'static str,
        prefix: &'static str,
        fix: &'static str,
    ) -> Self {
        Self {
            name,
            code,
            prefix,
            fix,
        }
    }
}

/// Codes a cause can carry instead of its variant's: the image the runtime
/// or registry does not have, a registry that refuses, no network, and
/// cloud credentials that are missing or rejected.
pub const REFINED_CODES: &[&str] = &[
    "image_not_found",
    "registry_auth",
    "network",
    "cloud_credentials",
];

/// Every code an [`ErrorReport`] can carry: one per variant, then the refined
/// ones.
pub fn all_codes() -> Vec<&'static str> {
    VARIANTS
        .iter()
        .map(|v| v.code)
        .chain(REFINED_CODES.iter().copied())
        .collect()
}

fn variant(name: &str) -> Option<&'static Variant> {
    VARIANTS.iter().find(|v| v.name.eq_ignore_ascii_case(name))
}

impl CuaError {
    /// The error's stable code (`not_found`): the same id the CLI prints in
    /// `--json` errors and the bindings expose as `.code`. See
    /// [`CuaError::report`] for the refined codes some causes get.
    pub fn code(&self) -> &'static str {
        variant(self.variant()).map_or("internal", |v| v.code)
    }

    /// The plain-words report: code, one sentence, one next step and the
    /// raw text.
    pub fn report(&self) -> ErrorReport {
        report_of(self.variant(), &self.to_string())
    }
}

/// The report for an error of variant `variant` whose text is `display`
/// (what `str(e)` / `e.message` / `e.to_string()` gives). Unknown variant
/// names are reported as `internal`.
pub fn report_of(variant_name: &str, display: &str) -> ErrorReport {
    let v = variant(variant_name);
    let code = v.map_or("internal", |v| v.code);
    let prefix = v.map_or("", |v| v.prefix);
    let body = display
        .trim()
        .strip_prefix(prefix)
        .unwrap_or(display.trim())
        .trim();
    let default_fix = v.map(|v| v.fix.to_string());
    let (code, message, hint) = match classify(code, body) {
        Some(c) => (c.code.to_string(), c.message, c.hint.or(default_fix)),
        None => {
            let (message, parenthetical) = split_parenthetical_hint(&tidy(body));
            (code.to_string(), message, parenthetical.or(default_fix))
        }
    };
    let message = sentence(&message);
    let detail = (display.trim() != message).then(|| display.trim().to_string());
    ErrorReport {
        code,
        message,
        hint: hint.map(|h| sentence(&h)),
        detail,
    }
}

/// [`report_of`] for bindings: the variant name and the error's text in,
/// the report out. A hint is always one sentence; a message never carries a
/// source path or a Rust debug struct.
#[uniffi::export]
pub fn error_report(variant: String, message: String) -> ErrorReport {
    report_of(&variant, &message)
}

/// The refined code and wording of a cause that has its own sentence.
struct Cause {
    code: &'static str,
    message: String,
    hint: Option<String>,
}

fn cause(code: &'static str, message: impl Into<String>, hint: impl Into<String>) -> Option<Cause> {
    Some(Cause {
        code,
        message: message.into(),
        hint: Some(hint.into()),
    })
}

/// The variant's code as a `'static` str.
fn code_static(code: &str) -> &'static str {
    VARIANTS
        .iter()
        .find(|v| v.code == code)
        .map_or("internal", |v| v.code)
}

fn classify(code: &str, body: &str) -> Option<Cause> {
    let lower = body.to_ascii_lowercase();
    // "sandbox nope" reads as an error only with what is wrong with it.
    if code == "not_found" && !lower.contains("not found") && !lower.contains("no such") {
        return Some(Cause {
            code: "not_found",
            message: format!("{body} not found"),
            hint: None,
        });
    }
    // Cua Cloud, which has ended.
    if body.starts_with("Cua Cloud has ended") {
        return cause(
            code_static(code),
            "Cua Cloud has ended",
            "use a local sandbox or your own computer; see https://cua.ai/docs/cua-sdk/guides/cua-cloud-ended",
        );
    }
    // A sandbox name that is taken.
    if code == "already_exists" {
        let name = between(body, "named '", "'").unwrap_or_default();
        return cause(
            "already_exists",
            body,
            if name.is_empty() {
                "pick another name, or remove the existing sandbox with `cua sb rm NAME`"
                    .to_string()
            } else {
                format!("pick another name, or remove it with `cua sb rm {name}`")
            },
        );
    }
    // The sandbox runs but its services never answered.
    if lower.contains("did not answer within") {
        let name = between(body, "sandbox ", ": ").unwrap_or_default();
        let secs = between(body, "answer within ", "s").unwrap_or_default();
        let which = if name.is_empty() {
            "the sandbox".to_string()
        } else {
            format!("sandbox '{}'", name.trim_start_matches("local:"))
        };
        let message = if secs.is_empty() {
            format!("{which} started, but its services did not answer")
        } else {
            format!("{which} started, but its services did not answer within {secs} s")
        };
        let hint = if name.is_empty() {
            "run `cua doctor`, and check that this machine can reach the container's published ports (Colima, Docker Desktop)".to_string()
        } else {
            format!(
                "run `cua sb logs {}`, and check that this machine can reach the container's published ports (Colima, Docker Desktop)",
                name.trim_start_matches("local:")
            )
        };
        return cause("timeout", message, hint);
    }
    // An image the runtime or the registry does not have.
    if lower.contains("pull access denied")
        || lower.contains("repository does not exist")
        || (lower.contains("status code 404") && lower.contains("pull "))
        || lower.contains("manifest unknown")
    {
        let image = between(body, "pull ", ": ")
            .or_else(|| between(body, "pull ", " "))
            .unwrap_or_default();
        let message = if image.is_empty() {
            "no such image".to_string()
        } else {
            format!("no image named \"{image}\"")
        };
        return cause(
            "image_not_found",
            message,
            "use an alias (linux, windows, macos) or a full image reference, and list the aliases with `cua images ls`",
        );
    }
    // A registry that wants a login.
    if (lower.contains("not authorized")
        || lower.contains("unauthorized")
        || lower.contains("denied: "))
        && (lower.contains("registry") || lower.contains("ghcr.io") || lower.contains("docker"))
    {
        let image = between(body, "cannot read ", ": ")
            .or_else(|| between(body, "pull ", ": "))
            .unwrap_or_default();
        let message = if image.is_empty() {
            "the registry refused the pull".to_string()
        } else {
            format!("the registry refused to serve {image}")
        };
        return cause(
            "registry_auth",
            message,
            "log in with `docker login` (ghcr.io for Cua images), or set CUA_REGISTRY_USERNAME and CUA_REGISTRY_PASSWORD",
        );
    }
    // AWS: no credentials at all.
    if lower.contains("credentialsnotloaded")
        || lower.contains("no providers in chain")
        || lower.contains("credentials not loaded")
        || lower.contains("unable to load credentials")
        || lower.contains("no aws credentials")
    {
        return cause(
            "cloud_credentials",
            "no AWS credentials found",
            "sign in with `aws sso login` or `aws configure`, or pick a profile with AWS_PROFILE (`cua cloud connect aws --profile NAME`)",
        );
    }
    // AWS: credentials the account rejected.
    for rejected in [
        "InvalidClientTokenId",
        "ExpiredToken",
        "SignatureDoesNotMatch",
        "AuthFailure",
        "UnrecognizedClientException",
    ] {
        if body.contains(rejected) {
            let after = body
                .split_once(&format!("{rejected}: "))
                .map(|(_, r)| r)
                .unwrap_or_default();
            let (reason, parenthetical) = split_parenthetical_hint(after);
            let reason = reason.trim().trim_end_matches('.');
            return cause(
                "cloud_credentials",
                if reason.is_empty() {
                    "AWS rejected these credentials".to_string()
                } else {
                    format!("AWS rejected these credentials: {}", lower_first(reason))
                },
                parenthetical.unwrap_or_else(|| {
                    "sign in again with `aws sso login` or `aws configure`".to_string()
                }),
            );
        }
    }
    // The Modal helper is not next to this install.
    if lower.contains("modal-helper") || lower.contains("find the helper") {
        return cause(
            "provider_not_configured",
            "the Modal helper is missing from this install",
            "reinstall Cua with `curl -fsSL https://cua.ai/install.sh | sh`",
        );
    }
    // Modal: no token.
    if lower.contains("modal")
        && (lower.contains("token")
            && (lower.contains("not found")
                || lower.contains("missing")
                || lower.contains("no modal")))
    {
        return cause(
            "cloud_credentials",
            "no Modal token found",
            "run `modal token new`, or pick a profile with `cua cloud connect modal --profile NAME`",
        );
    }
    // The network.
    if lower.contains("dial tcp")
        || lower.contains("error sending request")
        || lower.contains("failed to resolve reference")
        || lower.contains("no such host")
        || lower.contains("network is unreachable")
        || lower.contains("temporary failure in name resolution")
        || lower.contains("dns error")
    {
        let host = host_of(body);
        let message = match host {
            Some(h) => format!("cannot reach {h}"),
            None => "cannot reach the network".to_string(),
        };
        return cause(
            "network",
            message,
            "check your network connection and proxy settings, then try again",
        );
    }
    // Sign-in for a sandbox or a service.
    if code == "unauthenticated"
        && (lower.contains("bearer token") || lower.contains("missing or invalid"))
    {
        return cause(
            "unauthenticated",
            "the request was not authorised",
            "run `cua auth login` again, or pass the sandbox's token",
        );
    }
    None
}

/// `text` between the first `from` and the next `to` after it.
fn between<'a>(text: &'a str, from: &str, to: &str) -> Option<&'a str> {
    let start = text.find(from)? + from.len();
    let rest = &text[start..];
    let end = rest.find(to)?;
    let found = rest[..end].trim();
    (!found.is_empty()).then_some(found)
}

/// The host of the first `http(s)://host/...` URL in `text`, else the
/// address of a `dial tcp host:port`.
fn host_of(text: &str) -> Option<String> {
    for scheme in ["https://", "http://"] {
        if let Some(i) = text.find(scheme) {
            let rest = &text[i + scheme.len()..];
            let end = rest
                .find(|c: char| {
                    c == '/' || c == ')' || c == ' ' || c == ':' || c == '\'' || c == '"'
                })
                .unwrap_or(rest.len());
            if end > 0 {
                return Some(rest[..end].to_string());
            }
        }
    }
    let i = text.find("dial tcp ")? + "dial tcp ".len();
    let rest = &text[i..];
    let end = rest.find(':').unwrap_or(rest.len());
    let addr = rest[..end].trim();
    (!addr.is_empty()).then(|| addr.to_string())
}

fn lower_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_uppercase() && !s.chars().nth(1).is_some_and(char::is_uppercase) => {
            c.to_lowercase().chain(chars).collect()
        }
        _ => s.to_string(),
    }
}

/// Inner layers: tags in square brackets and the prefixes the layers below
/// a variant add.
const LAYERS: &[&str] = &[
    "local runtime: ",
    "container engine: ",
    "your cloud: ",
    "unauthenticated: ",
    "provider not configured: ",
    "invalid argument: ",
    "transport: ",
    "env: ",
    "http: ",
    "timed out: ",
];

/// Strips the inner layers, debug structs, source paths and gRPC status
/// names from `body`, leaving the first line.
pub(crate) fn tidy(body: &str) -> String {
    let mut s = body.lines().next().unwrap_or_default().trim().to_string();
    // Layers can sit at the start and between the colons of the chain.
    loop {
        let before = s.clone();
        s = s.trim_start().to_string();
        if s.starts_with('[')
            && let Some(end) = s.find("] ")
            && end < 24
        {
            s = s[end + 2..].to_string();
        }
        for layer in LAYERS {
            if let Some(rest) = s.strip_prefix(layer) {
                s = rest.to_string();
            }
        }
        if s == before {
            break;
        }
    }
    for layer in LAYERS {
        s = s.replace(&format!(": {layer}"), ": ");
        s = s.replace(&format!(": [local] {layer}"), ": ");
    }
    s = s.replace(": [local] ", ": ");
    s = s.replace(": your cloud: aws: ", ": ");
    s = s.replace(": your cloud: modal: ", ": ");
    s = s.replace(": your cloud: gcp: ", ": ");
    s = strip_trailing_status(&s);
    s = cut_debug(&s);
    s = scrub_paths(&s);
    s.trim().trim_end_matches([':', '(', ' ']).to_string()
}

/// ` (Unauthenticated)`: a gRPC status name the message ends with.
fn strip_trailing_status(s: &str) -> String {
    if let Some(open) = s.rfind(" (")
        && s.ends_with(')')
    {
        let inner = &s[open + 2..s.len() - 1];
        if !inner.is_empty()
            && inner.chars().all(|c| c.is_ascii_alphabetic())
            && inner.chars().next().is_some_and(char::is_uppercase)
        {
            return s[..open].to_string();
        }
    }
    s.to_string()
}

/// Cuts at the first Rust `Debug` struct (`DispatchFailure { source: ...`).
fn cut_debug(s: &str) -> String {
    const MARKERS: &[&str] = &[
        " { source:",
        "{ source:",
        "DispatchFailure(",
        "ProviderChainError",
        "CredentialsNotLoaded(",
        "ConnectorError {",
        "SdkError",
        " { kind:",
        "{ attempts:",
        "ErrorMetadata {",
    ];
    let cut = MARKERS.iter().filter_map(|m| s.find(m)).min();
    match cut {
        Some(i) => {
            let trimmed = s[..i].trim_end();
            // Drop a trailing identifier directly attached to the cut.
            let keep = trimmed.rfind([' ', ':', '(']).map_or(0, |p| p + 1);
            let tail = &trimmed[keep..];
            if tail.chars().all(|c| c.is_alphanumeric() || c == '_')
                && !tail.is_empty()
                && trimmed.len() > tail.len()
            {
                trimmed[..keep].trim_end().to_string()
            } else {
                trimmed.to_string()
            }
        }
        None => s.to_string(),
    }
}

/// Drops words that are source-tree paths: `libs/cua/crates/...` and
/// `file.rs:123`.
fn scrub_paths(s: &str) -> String {
    let is_path = |w: &str| {
        let w = w.trim_matches(|c: char| matches!(c, '(' | ')' | ',' | ';' | '`' | '\'' | '"'));
        w.contains("/crates/")
            || w.starts_with("libs/")
            || w.contains(".rs:")
            || w.ends_with(".rs")
            || w.contains("/src/") && w.contains(".rs")
    };
    if !s.split_whitespace().any(is_path) {
        return s.to_string();
    }
    let mut out: Vec<&str> = Vec::new();
    let words: Vec<&str> = s.split_whitespace().collect();
    let mut i = 0;
    while i < words.len() {
        if is_path(words[i]) {
            // "in <path>" reads badly without the path.
            if matches!(out.last(), Some(&"in") | Some(&"at") | Some(&"from")) {
                out.pop();
            }
        } else {
            out.push(words[i]);
        }
        i += 1;
    }
    out.join(" ")
}

/// `text (sign in with `aws sso login`)` into the text and the
/// parenthetical, when the parenthetical is a next step.
fn split_parenthetical_hint(text: &str) -> (String, Option<String>) {
    let t = text.trim();
    if let Some(open) = t.rfind(" (")
        && t.ends_with(')')
    {
        let inner = &t[open + 2..t.len() - 1];
        let starts_like_step = [
            "sign in", "run ", "try ", "use ", "see ", "set ", "build ", "pass ", "retry",
        ]
        .iter()
        .any(|p| inner.starts_with(p));
        if starts_like_step {
            return (t[..open].trim().to_string(), Some(inner.to_string()));
        }
    }
    (t.to_string(), None)
}

/// One sentence: the first line, trimmed, with no trailing full stop.
fn sentence(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    let line = line.trim_end_matches([':', ' ']);
    // Keep it readable: a run-on message is cut at a clause boundary.
    const MAX: usize = 300;
    if line.chars().count() <= MAX {
        return line.to_string();
    }
    let cut: String = line.chars().take(MAX).collect();
    let end = cut
        .rfind(". ")
        .or_else(|| cut.rfind("; "))
        .or_else(|| cut.rfind(", "))
        .unwrap_or(cut.len());
    cut[..end]
        .trim_end_matches(['.', ',', ';', ' '])
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(variant: &str, display: &str) -> ErrorReport {
        report_of(variant, display)
    }

    /// Text that must never reach a person (a layer of our own code, a
    /// source path, a Rust debug struct).
    pub(crate) const INTERNAL: &[&str] = &[
        "local runtime:",
        "[local]",
        "container engine:",
        "libs/cua",
        "crates/",
        ".rs:",
        "DispatchFailure",
        "ProviderChainError",
        "CredentialsNotLoaded",
        "ConnectorError",
        "{ source",
        "(Unauthenticated)",
    ];

    fn assert_clean(r: &ErrorReport) {
        for bad in INTERNAL {
            assert!(!r.message.contains(bad), "{bad} in message: {r:?}");
            if let Some(h) = &r.hint {
                assert!(!h.contains(bad), "{bad} in hint: {r:?}");
            }
        }
        assert!(!r.message.contains('\n'));
    }

    /// The recorded failures of the output-UX report (E1 to E11).
    #[test]
    fn the_known_ugly_errors_read_as_one_sentence_and_a_fix() {
        let cases: &[(&str, &str, &str, &str, &str)] = &[
            (
                "Timeout",
                "timed out: [local] sandbox logs-a: cua-spacesd did not answer within 120s (nothing answered at http://127.0.0.1:54945/ (it may still be starting; retry in a few seconds): gRPC: tcp connect error (Connection refused (os error 61))); see `cua sb logs logs-a`",
                "timeout",
                "sandbox 'logs-a' started, but its services did not answer within 120 s",
                "cua sb logs logs-a",
            ),
            (
                "Runtime",
                "local runtime: [local] container engine: pull no-such-image-xyz: Docker responded with status code 404: pull access denied for no-such-image-xyz, repository does not exist or may require 'docker login'",
                "image_not_found",
                "no image named \"no-such-image-xyz\"",
                "cua images ls",
            ),
            (
                "ProviderNotConfigured",
                "provider not configured: cannot read registry.example.com/team/missing:9: registry: Not authorized: Not authorized: url https://registry.example.com/v2/team/missing/manifests/9. Log in to ghcr.io (`docker login`, ...), or set CUA_REGISTRY_USERNAME/PASSWORD",
                "registry_auth",
                "the registry refused to serve registry.example.com/team/missing:9",
                "docker login",
            ),
            (
                "Runtime",
                "local runtime: [local] container engine: pull ghcr.io/trycua/linux:24.04-slim: Docker responded with status code 500: failed to resolve reference \"ghcr.io/trycua/linux:24.04-slim\": dial tcp 20.29.134.18:443: connect: connection refused",
                "network",
                "cannot reach 20.29.134.18",
                "network connection",
            ),
            (
                "Http",
                "http: error sending request for url (https://auth.cua.ai/realms/cyclops-cs/.well-known/openid-configuration)",
                "network",
                "cannot reach auth.cua.ai",
                "network connection",
            ),
            (
                "Cloud",
                "your cloud: AWS is not ready, nothing was connected: credentials: your cloud: aws: identify the account: InvalidClientTokenId: The security token included in the request is invalid. (sign in with `aws sso login --profile bogus` or `aws configure`)",
                "cloud_credentials",
                "AWS rejected these credentials: the security token included in the request is invalid",
                "aws sso login --profile bogus",
            ),
            (
                "Cloud",
                "your cloud: AWS is not ready, nothing was connected: credentials: your cloud: aws: identify the account: dispatch failure: Ec2InstanceMetadata: ConnectorError { kind: Other(None), source: CredentialsNotLoaded(CredentialsNotLoaded { source: Some(ProviderChainError { attempts: [ProviderAttempt { name: \"Environment\" } ] }) }) } (DispatchFailure(DispatchFailure { source: ConnectorError { kind: Other(None) } }))",
                "cloud_credentials",
                "no AWS credentials found",
                "aws sso login",
            ),
            (
                "Cloud",
                "your cloud: Modal is not ready, nothing was connected: credentials: your cloud: modal: find the helper: Modal runs through its official Go SDK in cua-modal-helper; build it with `go build` in libs/cua/crates/cua-contrib/modal-helper and put it next to cua, on PATH, or in CUA_MODAL_HELPER",
                "provider_not_configured",
                "the Modal helper is missing from this install",
                "install.sh",
            ),
            (
                "Unauthenticated",
                "unauthenticated: [local] unauthenticated: missing or invalid bearer token (Unauthenticated)",
                "unauthenticated",
                "the request was not authorised",
                "cua auth login",
            ),
            (
                "AlreadyExists",
                "a sandbox named 'dup2' already exists",
                "already_exists",
                "a sandbox named 'dup2' already exists",
                "cua sb rm dup2",
            ),
            (
                "NotFound",
                "not found: sandbox nope",
                "not_found",
                "sandbox nope not found",
                "cua sb ls",
            ),
        ];
        for (variant, display, code, message, hint) in cases {
            let r = report(variant, display);
            assert_eq!(r.code, *code, "{display}");
            assert_eq!(r.message, *message, "{display}");
            assert!(
                r.hint.as_deref().is_some_and(|h| h.contains(hint)),
                "hint {:?} lacks {hint}",
                r.hint
            );
            assert_clean(&r);
            assert_eq!(
                r.detail.as_deref(),
                (display != &r.message.as_str()).then_some(*display)
            );
        }
    }

    #[test]
    fn an_unknown_error_keeps_its_first_line_and_gets_the_variants_fix() {
        let r = report("Internal", "internal: something odd\nsecond line");
        assert_eq!(r.code, "internal");
        assert_eq!(r.message, "something odd");
        assert!(r.hint.unwrap().contains("github.com/trycua/cua/issues"));
        let r = report("NoSuchVariant", "x");
        assert_eq!(r.code, "internal");
    }

    #[test]
    fn source_paths_and_structs_are_scrubbed_from_any_message() {
        let r = report(
            "Runtime",
            "local runtime: failed in libs/cua/crates/cua-vmm/src/lib.rs:12 with Foo { source: Bar }",
        );
        assert_clean(&r);
        let r = report("Internal", "internal: panicked at src/main.rs:10:5");
        assert_clean(&r);
    }

    #[test]
    fn a_long_message_is_cut_at_a_clause() {
        let long = format!("{}. {}", "a".repeat(200), "b".repeat(200));
        let r = report("Internal", &format!("internal: {long}"));
        assert!(r.message.chars().count() <= 300);
        assert!(!r.message.ends_with('.'));
    }

    /// The variant table has one row per `CuaError` variant, and every
    /// row's code, prefix and fix match the enum: the `Display` prefix, and
    /// the first sentence of the `Fix:` paragraph in the doc comment.
    #[test]
    fn the_table_matches_the_enum_and_its_fix_docs() {
        let source = include_str!("lib.rs");
        let enum_text = source
            .split("pub enum CuaError {")
            .nth(1)
            .and_then(|r| r.split("\n}\n").next())
            .expect("the CuaError enum");
        let mut docs: Vec<String> = Vec::new();
        let mut names: Vec<(String, String, String)> = Vec::new(); // name, error attr, fix
        for line in enum_text.lines() {
            let t = line.trim();
            if let Some(doc) = t.strip_prefix("///") {
                docs.push(doc.trim().to_string());
            } else if t.starts_with("#[error(") {
                let attr = t
                    .trim_start_matches("#[error(\"")
                    .trim_end_matches("\")]")
                    .to_string();
                names.push((String::new(), attr, fix_of(&docs)));
            } else if let Some(name) = t
                .split('(')
                .next()
                .filter(|_| t.ends_with("),") || t.ends_with(')'))
                && let Some(last) = names.last_mut()
                && last.0.is_empty()
            {
                last.0 = name.to_string();
                docs.clear();
            }
        }
        assert_eq!(names.len(), VARIANTS.len(), "{names:?}");
        for (name, attr, fix) in &names {
            let row = variant(name).unwrap_or_else(|| panic!("no row for {name}"));
            assert_eq!(&row.name, name);
            assert_eq!(
                attr.replace("{0}", ""),
                row.prefix,
                "{name}: the Display prefix"
            );
            assert!(
                fix.starts_with(row.fix),
                "{name}: the table's fix {:?} is not the start of the Fix: doc {:?}",
                row.fix,
                fix
            );
            assert!(
                !row.code.is_empty()
                    && row.code.chars().all(|c| c.is_ascii_lowercase() || c == '_')
            );
        }
    }

    /// The `Fix:` paragraph of a variant's doc lines, joined.
    fn fix_of(docs: &[String]) -> String {
        let start = docs.iter().position(|l| l.starts_with("Fix:"));
        match start {
            Some(i) => docs[i..]
                .join(" ")
                .trim_start_matches("Fix:")
                .trim()
                .to_string(),
            None => String::new(),
        }
    }

    #[test]
    fn every_variant_reports_its_own_code() {
        for (e, code) in [
            (CuaError::NotFound("x".into()), "not_found"),
            (CuaError::AlreadyExists("x".into()), "already_exists"),
            (CuaError::Cloud("x".into()), "cloud"),
        ] {
            assert_eq!(e.code(), code);
            assert_eq!(e.report().code, code);
        }
    }
}
