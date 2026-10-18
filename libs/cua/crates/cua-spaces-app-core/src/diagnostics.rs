// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The opt-in diagnostic report for a failed Space create.
//!
//! Standard crash-report pattern: on a failed create the app builds the
//! report here ([`create_report`]), redacted ([`redact`]), and shows it in a
//! review sheet. The person can edit or leave out each part, and nothing
//! leaves the machine unless they press Send. Send opens a prefilled email
//! to Cua support ([`report_mailto`], a `mailto:` link) in their mail app with
//! the kept parts. Reports are private: they go only to support@trycua.com,
//! and only when the person sends that email themselves. No new service
//! receives anything.
//!
//! Redaction runs twice: when the sheet is built, and again on the text the
//! person sends (their edits included).

use serde::{Deserialize, Serialize};

use crate::about::AboutInput;

/// What the app knows about one failed create.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CreateReportInput {
    /// The error text the create failed with (the SDK's words, before the
    /// app rewrote them).
    pub error: String,
    /// What the failed row showed, when different.
    pub shown: Option<String>,
    /// The phase it failed in (`pulling`, `booting`, ...).
    pub phase: String,
    /// `linux`, `windows` or `macos`.
    pub os: String,
    /// `local`, `cloud` or `relay`.
    pub provider: String,
    /// `container` or `vm`, when known.
    pub kind: Option<String>,
    /// The image asked for.
    pub image: Option<String>,
    /// GPU acceleration was asked for.
    pub gpu: bool,
    /// Milliseconds from Create to the failure.
    pub elapsed_ms: Option<u64>,
    /// The app and this machine (Settings > About's input).
    pub about: Option<AboutInput>,
    /// Names to hide wherever they appear: this machine's and the account's
    /// other machines' names (Bonjour names have no dots, so [`redact`]
    /// cannot find them on its own).
    pub names: Vec<String>,
}

/// One part of the report the person can edit or leave out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportPart {
    /// `details`, `error` or `environment`.
    pub id: String,
    /// "What was created".
    pub label: String,
    /// The redacted text.
    pub text: String,
}

/// The review sheet for a failed create.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateReport {
    /// "Send a diagnostic report".
    pub title: String,
    /// What Send does, in plain words.
    pub intro: String,
    /// The email subject.
    pub subject: String,
    /// The parts, in order.
    pub parts: Vec<ReportPart>,
    /// "Send".
    pub send_label: String,
    /// The fixed failure word (`image_pull_auth`), as telemetry records it.
    pub error_kind: String,
}

/// The parts the person kept, as they edited them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReportSend {
    pub subject: String,
    pub parts: Vec<ReportPart>,
}

fn word(s: &str) -> &str {
    match s {
        "linux" => "Linux",
        "windows" => "Windows",
        "macos" => "macOS",
        "local" => "this machine",
        "cloud" => "Cua Cloud",
        "relay" => "another of my machines",
        "container" => "container",
        "vm" => "VM",
        _ => s,
    }
}

fn phase_words(phase: &str) -> &str {
    match phase {
        "preparing" => "preparing",
        "pulling" => "downloading the image",
        "creating" => "creating",
        "booting" => "starting up",
        "waiting_for_services" => "starting its services",
        "connecting" => "connecting",
        "" => "an unknown step",
        p => p,
    }
}

fn platform_name(platform: &str) -> &'static str {
    match platform {
        "windows" => "Windows",
        "linux" => "Linux",
        _ => "macOS",
    }
}

/// The report for a failed create, redacted.
pub fn create_report(input: &CreateReportInput) -> CreateReport {
    let kind = cua_create_failure::create_failure_kind(
        &crate::errors::plain_error(&input.error),
        &input.phase,
        "",
        input.provider != "cloud",
    );
    let error_kind = if kind.is_empty() { "other" } else { kind }.to_string();
    let r = |s: &str| redact_with(s, &input.names);
    let mut details = vec![
        format!(
            "Space: {} {}",
            word(&input.os),
            input.kind.as_deref().map(word).unwrap_or("Space")
        ),
        format!("Where: {}", word(&input.provider)),
    ];
    if let Some(image) = input.image.as_deref().filter(|i| !i.trim().is_empty()) {
        details.push(format!("Image: {}", r(image.trim())));
    }
    details.push(format!("Failed while: {}", phase_words(&input.phase)));
    details.push(format!("Failure kind: {error_kind}"));
    if input.gpu {
        details.push("GPU acceleration: on".into());
    }
    if let Some(ms) = input.elapsed_ms {
        let secs = ms / 1000;
        details.push(if secs >= 120 {
            format!("Time to failure: {} min", secs / 60)
        } else {
            format!("Time to failure: {secs} s")
        });
    }
    let mut error = r(input.error.trim());
    if error.is_empty() {
        error = "No error text was reported.".into();
    }
    if let Some(shown) = input.shown.as_deref().map(str::trim)
        && !shown.is_empty()
        && shown != input.error.trim()
    {
        error = format!("{error}\n\nThe app showed: {}", r(shown));
    }
    let environment = match &input.about {
        Some(a) => {
            let mut e = format!(
                "Cua Spaces for {} {}",
                platform_name(&a.platform),
                crate::about::version_line(&a.version, &a.build).trim_start_matches("Version ")
            );
            if !a.os.is_empty() {
                e.push('\n');
                e.push_str(&r(&a.os));
            }
            e
        }
        None => "Cua Spaces".into(),
    };
    CreateReport {
        title: "Send a diagnostic report".into(),
        intro: format!(
            "This is what the report says. We removed tokens, email addresses, \
             hostnames and your home folder. Edit it or leave parts out. Send opens \
             an email to Cua support ({SUPPORT_EMAIL}) in your mail app, with the \
             parts you kept. It goes nowhere else, and nothing is sent until you \
             press Send in your mail app."
        ),
        subject: format!(
            "Spaces: {} {} create failed ({error_kind})",
            word(&input.os),
            input.kind.as_deref().map(word).unwrap_or("Space")
        ),
        parts: vec![
            ReportPart {
                id: "details".into(),
                label: "What was created".into(),
                text: details.join("\n"),
            },
            ReportPart {
                id: "error".into(),
                label: "Error".into(),
                text: error,
            },
            ReportPart {
                id: "environment".into(),
                label: "App and system".into(),
                text: environment,
            },
        ],
        send_label: "Send".into(),
        error_kind,
    }
}

/// Where reports go: Cua's support address, as on the public site.
pub const SUPPORT_EMAIL: &str = "support@trycua.com";

/// Longest body the email link carries, counted as written in the link
/// (percent-encoded). Mail apps and the OS cut or drop much longer links
/// (Windows' limit is about 2,000 characters for the whole link).
const BODY_MAX: usize = 1800;

/// Longest subject, in characters.
const SUBJECT_MAX: usize = 150;

/// Added where a part is cut to fit the link.
const CUT_NOTE: &str = "[cut: too long for an email link]";

/// Whether `b` stays as it is in a URL query value (RFC 3986 unreserved).
fn is_unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~')
}

/// `s` for a URL query value. Line breaks become `%0D%0A`, as RFC 6068 asks
/// for a mail body.
fn query_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b == b'\n' {
            out.push_str("%0D%0A");
        } else if is_unreserved(b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// What `c` adds to a query value, as [`query_value`] writes it.
fn encoded_cost(c: char) -> usize {
    match c {
        '\n' => 6,
        c if c.is_ascii() => {
            if is_unreserved(c as u8) {
                1
            } else {
                3
            }
        }
        c => 3 * c.len_utf8(),
    }
}

fn encoded_len(s: &str) -> usize {
    s.chars().map(encoded_cost).sum()
}

/// The longest start of `s` that takes at most `max` in a query value.
fn encoded_prefix(s: &str, max: usize) -> &str {
    let mut used = 0;
    for (i, c) in s.char_indices() {
        used += encoded_cost(c);
        if used > max {
            return &s[..i];
        }
    }
    s
}

/// `s` with line breaks as `\n` and no leading or trailing blank space.
fn tidy(s: &str) -> String {
    s.replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim()
        .to_string()
}

/// The email body from the parts the person kept. Over [`BODY_MAX`], the
/// error is cut first (it is the part that grows), then the whole body, each
/// with a note. Redaction has already run, so a cut never shows half a secret.
fn mail_body(details: Option<&str>, error: Option<&str>, environment: Option<&str>) -> String {
    let build = |error: Option<&str>| {
        let mut out = String::from(
            "Sent from the diagnostic report in Cua Spaces after a Space failed to create.",
        );
        for (heading, text) in [
            ("What was created", details),
            ("Error", error),
            ("App and system", environment),
        ] {
            if let Some(text) = text {
                out.push_str(&format!("\n\n{heading}\n{text}"));
            }
        }
        out
    };
    let note = format!("\n{CUT_NOTE}");
    let body = build(error);
    if encoded_len(&body) <= BODY_MAX {
        return body;
    }
    if let Some(error) = error {
        // The error gets the room the other parts leave.
        let room = BODY_MAX.saturating_sub(encoded_len(&build(Some(&note))));
        let kept = encoded_prefix(error, room).trim_end();
        let body = build(Some(&format!("{kept}{note}")));
        if encoded_len(&body) <= BODY_MAX {
            return body;
        }
    }
    let room = BODY_MAX.saturating_sub(encoded_len(&note));
    format!("{}{note}", encoded_prefix(&body, room).trim_end())
}

/// The prefilled email to Cua support with the parts the person kept,
/// redacted again (their edits included), as a `mailto:` link. Nothing is
/// sent by building it: the shell opens it in the person's mail app, where
/// they press Send themselves.
pub fn report_mailto(send: &ReportSend, names: &[String]) -> String {
    let r = |s: &str| redact_with(&tidy(s), names);
    let part = |id: &str| {
        send.parts
            .iter()
            .find(|p| p.id == id)
            .map(|p| r(&p.text))
            .filter(|t| !t.is_empty())
    };
    let subject = r(&send.subject)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let subject: String = subject.chars().take(SUBJECT_MAX).collect();
    let subject = if subject.is_empty() {
        "Spaces: create failed".to_string()
    } else {
        subject
    };
    let body = mail_body(
        part("details").as_deref(),
        part("error").as_deref(),
        part("environment").as_deref(),
    );
    format!(
        "mailto:{SUPPORT_EMAIL}?subject={}&body={}",
        query_value(&subject),
        query_value(&body)
    )
}

// ---- Redaction --------------------------------------------------------------

/// `text` with tokens, email addresses, home folders, hostnames and public
/// IP addresses replaced.
pub fn redact(text: &str) -> String {
    redact_with(text, &[])
}

/// [`redact`], also hiding each of `names` (machine names) wherever it
/// appears.
pub fn redact_with(text: &str, names: &[String]) -> String {
    let mut s = home_paths(text);
    for n in names {
        let n = n.trim();
        if n.chars().count() >= 3 {
            s = replace_ci(&s, n, "<machine>");
        }
    }
    let mut out = String::with_capacity(s.len());
    let mut secret_next = false;
    for (piece, is_space) in split_keep(&s) {
        if is_space {
            out.push_str(piece);
            continue;
        }
        let (word, next_secret) = redact_word(piece, secret_next);
        secret_next = next_secret;
        out.push_str(&word);
    }
    out
}

/// Whitespace and non-whitespace runs, in order.
fn split_keep(s: &str) -> Vec<(&str, bool)> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut space = None;
    for (i, c) in s.char_indices() {
        let w = c.is_whitespace();
        match space {
            Some(prev) if prev != w => {
                out.push((&s[start..i], prev));
                start = i;
            }
            _ => {}
        }
        space = Some(w);
    }
    if start < s.len() {
        out.push((&s[start..], space.unwrap_or(false)));
    }
    out
}

/// ASCII case-insensitive replace of every `needle`.
fn replace_ci(hay: &str, needle: &str, with: &str) -> String {
    let lower = hay.to_ascii_lowercase();
    let n = needle.to_ascii_lowercase();
    if n.is_empty() || !lower.contains(&n) {
        return hay.to_string();
    }
    let mut out = String::with_capacity(hay.len());
    let mut i = 0;
    while let Some(off) = lower[i..].find(&n) {
        let at = i + off;
        out.push_str(&hay[i..at]);
        out.push_str(with);
        i = at + n.len();
    }
    out.push_str(&hay[i..]);
    out
}

/// `/Users/<name>`, `/home/<name>`, `C:\Users\<name>` (any case, either
/// slash) become `~`.
fn home_paths(s: &str) -> String {
    let mut out = s.to_string();
    for prefix in [
        "/Users/",
        "/home/",
        "C:\\Users\\",
        "C:/Users/",
        "/var/home/",
    ] {
        let mut result = String::with_capacity(out.len());
        let lower = out.to_ascii_lowercase();
        let p = prefix.to_ascii_lowercase();
        let mut i = 0;
        while let Some(off) = lower[i..].find(&p) {
            let at = i + off;
            let name_start = at + p.len();
            let name_end = out[name_start..]
                .find(['/', '\\', ' ', '\'', '"', ')', ':', ',', '\n'])
                .map_or(out.len(), |e| name_start + e);
            let name = &out[name_start..name_end];
            result.push_str(&out[i..at]);
            if name.is_empty() || name.eq_ignore_ascii_case("shared") {
                result.push_str(&out[at..name_end]);
            } else {
                result.push('~');
            }
            i = name_end;
        }
        result.push_str(&out[i..]);
        out = result;
    }
    out
}

/// Keys whose value is a secret (`token=`, `password:`, `Authorization`).
const SECRET_KEYS: &[&str] = &[
    "token",
    "secret",
    "password",
    "passwd",
    "api_key",
    "apikey",
    "api-key",
    "authorization",
    "bearer",
    "credential",
    "private_key",
    "session",
    "cookie",
    "signature",
    "x-amz-signature",
    "sig",
];

/// Token prefixes of well-known credentials.
const TOKEN_PREFIXES: &[&str] = &[
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "glpat-",
    "sk-",
    "sk_live_",
    "sk_test_",
    "rk_live_",
    "pk_live_",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "akia",
    "eyj",
    "cua_",
    "cuak_",
    "hf_",
    "npm_",
];

/// Registries and services whose names say nothing about the person.
const PUBLIC_HOSTS: &[&str] = &[
    "ghcr.io",
    "github.com",
    "docker.io",
    "registry-1.docker.io",
    "index.docker.io",
    "quay.io",
    "cua.ai",
    "trycua.com",
    "apple.com",
    "microsoft.com",
    "ubuntu.com",
    "debian.org",
    "localhost",
];

/// Top-level domains a hostname ends with (filenames like `config.json`
/// do not).
const TLDS: &[&str] = &[
    "com",
    "net",
    "org",
    "io",
    "ai",
    "dev",
    "co",
    "cloud",
    "sh",
    "me",
    "xyz",
    "tech",
    "us",
    "uk",
    "de",
    "fr",
    "eu",
    "it",
    "es",
    "nl",
    "ch",
    "ca",
    "au",
    "jp",
    "in",
    "br",
    "info",
    "biz",
    "local",
    "lan",
    "internal",
    "home",
    "corp",
    "localdomain",
    "arpa",
];

fn is_secret_key(k: &str) -> bool {
    let k = k
        .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-')
        .to_ascii_lowercase();
    !k.is_empty()
        && SECRET_KEYS
            .iter()
            .any(|s| k == *s || k.ends_with(&format!("_{s}")) || k.ends_with(&format!("-{s}")))
}

fn looks_like_token(w: &str) -> bool {
    let l = w.to_ascii_lowercase();
    if TOKEN_PREFIXES.iter().any(|p| l.starts_with(p))
        && w.len() >= 12
        && w.chars().any(|c| c.is_ascii_digit())
        && !w.contains('=')
    {
        return true;
    }
    // Digests and ids stay: they help and are not secrets.
    if l.starts_with("sha256:") || l.starts_with("sha512:") || is_uuid(w) {
        return false;
    }
    if w.len() < 32 {
        return false;
    }
    let allowed = w
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '/' | '=' | '.'));
    let digits = w.chars().any(|c| c.is_ascii_digit());
    let letters = w.chars().any(|c| c.is_ascii_alphabetic());
    // A plain hex string (a digest without its prefix, a container id)
    // stays too.
    let hex = w.chars().all(|c| c.is_ascii_hexdigit());
    allowed && digits && letters && !hex && !w.contains("..") && !w.starts_with('/')
}

fn is_uuid(w: &str) -> bool {
    let parts: Vec<&str> = w.split('-').collect();
    parts.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&parts)
            .all(|(n, p)| p.len() == *n && p.chars().all(|c| c.is_ascii_hexdigit()))
}

fn is_email(w: &str) -> bool {
    let Some((user, domain)) = w.split_once('@') else {
        return false;
    };
    !user.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
        && user
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+' | '%'))
}

/// A hostname to hide: dotted labels ending in a known TLD, not a public
/// registry or service.
fn is_private_host(h: &str) -> bool {
    let h = h.trim_end_matches('.').to_ascii_lowercase();
    let labels: Vec<&str> = h.split('.').collect();
    if labels.len() < 2
        || labels
            .iter()
            .any(|l| l.is_empty() || !l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
    {
        return false;
    }
    let tld = labels[labels.len() - 1];
    if !TLDS.contains(&tld) {
        return false;
    }
    !PUBLIC_HOSTS
        .iter()
        .any(|p| h == *p || h.ends_with(&format!(".{p}")))
}

fn is_public_ipv4(h: &str) -> bool {
    let parts: Vec<u8> = h.split('.').filter_map(|p| p.parse().ok()).collect();
    if parts.len() != 4 || h.split('.').count() != 4 {
        return false;
    }
    let private = parts[0] == 10
        || parts[0] == 127
        || (parts[0] == 192 && parts[1] == 168)
        || (parts[0] == 172 && (16..=31).contains(&parts[1]))
        || (parts[0] == 169 && parts[1] == 254)
        || (parts[0] == 100 && (64..=127).contains(&parts[1]))
        || parts[0] == 0;
    !private
}

/// `a=1&token=x`: secret values and token-like values hidden.
fn redact_query(q: &str) -> String {
    q.split('&')
        .map(|pair| match pair.split_once('=') {
            Some((k, _)) if is_secret_key(k) => format!("{k}=<redacted>"),
            Some((k, v)) if is_email(v) || looks_like_token(v) => format!("{k}=<redacted>"),
            _ => pair.to_string(),
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// One whitespace-free piece, redacted; also whether the next piece is a
/// secret (it followed `Bearer` or `password:`).
fn redact_word(piece: &str, secret: bool) -> (String, bool) {
    // Keep surrounding punctuation as it is.
    let start = piece
        .find(|c: char| !matches!(c, '"' | '\'' | '(' | '[' | '<' | '{' | '`'))
        .unwrap_or(piece.len());
    let end = piece
        .rfind(|c: char| {
            !matches!(
                c,
                '"' | '\'' | ')' | ']' | '>' | '}' | ',' | ';' | '.' | '`' | ':'
            )
        })
        .map_or(start, |e| {
            e + piece[e..].chars().next().map_or(1, char::len_utf8)
        });
    let (lead, core, tail) = if start < end {
        (&piece[..start], &piece[start..end], &piece[end..])
    } else {
        return (piece.to_string(), secret);
    };
    if secret {
        // `Authorization: Bearer x`: the scheme stays, the next word goes.
        if core.eq_ignore_ascii_case("bearer") || core.eq_ignore_ascii_case("basic") {
            return (piece.to_string(), true);
        }
        return (format!("{lead}<redacted>{tail}"), false);
    }
    // `Bearer x`, `password: x`, `token x`.
    let key_alone =
        is_secret_key(core) && (tail.contains(':') || core.eq_ignore_ascii_case("bearer"));
    if key_alone || core.eq_ignore_ascii_case("bearer") {
        return (piece.to_string(), true);
    }
    (format!("{lead}{}{tail}", redact_core(core)), false)
}

fn redact_core(core: &str) -> String {
    // A query string: each pair on its own (`?token=..&page=2`).
    if let Some((before, query)) = core.split_once('?') {
        return format!("{}?{}", redact_core(before), redact_query(query));
    }
    // key=value and key:value pairs.
    for sep in ['=', ':'] {
        if let Some((k, v)) = core.split_once(sep)
            && !v.is_empty()
            && !v.starts_with("//")
            && is_secret_key(k)
        {
            return format!("{k}{sep}<redacted>");
        }
    }
    if is_email(core) {
        return "<email>".into();
    }
    if looks_like_token(core) {
        return "<redacted>".into();
    }
    // URLs and host[:port][/path]: hide the host, and any user:pass@.
    let (scheme, rest) = match core.find("://") {
        Some(i) => (&core[..i + 3], &core[i + 3..]),
        None => ("", core),
    };
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, path) = rest.split_at(host_end);
    let (userinfo, hostport) = match authority.rsplit_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, authority),
    };
    let host = if hostport.starts_with('[') {
        hostport
    } else {
        hostport.rsplit_once(':').map_or(hostport, |(h, p)| {
            if p.chars().all(|c| c.is_ascii_digit()) {
                h
            } else {
                hostport
            }
        })
    };
    let port = &hostport[host.len()..];
    let hide_host = is_private_host(host) || is_public_ipv4(host);
    if userinfo.is_none() && !hide_host {
        return core.to_string();
    }
    let host_out = if is_public_ipv4(host) {
        "<ip>".to_string()
    } else if hide_host {
        "<host>".to_string()
    } else {
        host.to_string()
    };
    let user_out = if userinfo.is_some() {
        "<redacted>@"
    } else {
        ""
    };
    format!("{scheme}{user_out}{host_out}{port}{path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hides_tokens() {
        for (raw, want) in [
            (
                concat!(
                    "token gh",
                    "p_abcdefghijklmnopqrstuvwxyz0123456789 rejected"
                ),
                "token <redacted> rejected",
            ),
            (
                // A JWT-shaped fake, joined at build time so no scanner reads
                // a token in the source.
                concat!(
                    "Authorization: Bearer ey",
                    "JfaGVhZGVy.ey",
                    "JfcGF5bG9hZA.c2lnbmF0dXJl"
                ),
                "Authorization: Bearer <redacted>",
            ),
            (
                concat!("api_key=sk", "-live-1234567890abcdef"),
                "api_key=<redacted>",
            ),
            ("password: hunter2!", "password: <redacted>"),
            (
                "GET /v1/x?token=abc123&page=2",
                "GET /v1/x?token=<redacted>&page=2",
            ),
            (
                concat!("key AKI", "AIOSFODNN7EXAMPLE leaked"),
                "key <redacted> leaked",
            ),
            (concat!("xox", "b-1234-5678-abcdefghijkl"), "<redacted>"),
            ("CUA_API_KEY=cua_0123456789abcdef", "CUA_API_KEY=<redacted>"),
            (
                "blob Zm9vYmFyYmF6cXV4MTIzNDU2Nzg5MGFiY2RlZmdoaWo= ok",
                "blob <redacted> ok",
            ),
        ] {
            assert_eq!(redact(raw), want, "{raw}");
        }
    }

    #[test]
    fn keeps_digests_ids_and_ordinary_words() {
        for raw in [
            "pull ghcr.io/trycua/linux:24.04@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "sandbox 3f2504e0-4f89-11d3-9a0c-0305e82c3301 was not ready after 300s",
            "container 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef exited",
            "Not enough disk space: this Space needs about 30.0 GB.",
            "QEMU (qemu-system-x86_64, qemu-img) is not available: install it",
            "cannot reach the VM at 192.168.64.45:3211 (No route to host (os error 65))",
            "read config.json and Cua Spaces.app",
        ] {
            assert_eq!(redact(raw), raw, "{raw}");
        }
    }

    #[test]
    fn hides_emails() {
        assert_eq!(
            redact("signed in as maya.lee+cua@example.com."),
            "signed in as <email>."
        );
        assert_eq!(redact("<maya@corp.example.org>"), "<<email>>");
    }

    #[test]
    fn hides_home_folders() {
        assert_eq!(
            redact("only 12.0 GB is available on /Users/maya/.lume (APFS)"),
            "only 12.0 GB is available on ~/.lume (APFS)"
        );
        assert_eq!(redact("open /home/sam/.cua/x.sock"), "open ~/.cua/x.sock");
        assert_eq!(redact(r"C:\Users\sam\AppData\Local"), r"~\AppData\Local");
        assert_eq!(redact("/Users/Shared/cua"), "/Users/Shared/cua");
    }

    #[test]
    fn hides_hostnames_and_public_ips() {
        assert_eq!(
            redact("connect to build-box.corp.example.net:8443 failed"),
            "connect to <host>:8443 failed"
        );
        assert_eq!(redact("Mac-mini.local refused"), "<host> refused");
        assert_eq!(
            redact("https://build-7.internal.acme.com/v2/x?sig=abc"),
            "https://<host>/v2/x?sig=<redacted>"
        );
        assert_eq!(
            redact("https://user:pw@ghcr.io/v2/"),
            "https://<redacted>@ghcr.io/v2/"
        );
        assert_eq!(redact("dial 203.0.113.9:443"), "dial <ip>:443");
        // Public registries and services stay: they say what failed.
        assert_eq!(
            redact("registry: Not authorized: ghcr.io/trycua/macos:26 from api.cua.ai"),
            "registry: Not authorized: ghcr.io/trycua/macos:26 from api.cua.ai"
        );
    }

    #[test]
    fn hides_machine_names_it_is_told() {
        let names = vec!["Francescos-MacBook-Pro".to_string(), "x".to_string()];
        assert_eq!(
            redact_with("host francescos-macbook-pro said no; x stays", &names),
            "host <machine> said no; x stays"
        );
    }

    fn input() -> CreateReportInput {
        CreateReportInput {
            error: concat!(
                "container engine: pull ghcr.io/me/private:1: pull access denied for \
                    ghcr.io/me/private, token gh",
                "p_abcdefghijklmnopqrstuvwxyz0123456789 \
                    (maya@example.com, /Users/maya/.cua)"
            )
            .into(),
            shown: Some("Could not download the image.".into()),
            phase: "pulling".into(),
            os: "linux".into(),
            provider: "local".into(),
            kind: Some("container".into()),
            image: Some("ghcr.io/me/private:1".into()),
            gpu: false,
            elapsed_ms: Some(42_000),
            about: Some(AboutInput {
                platform: "macos".into(),
                version: "0.4.0".into(),
                build: "0.4.0.200".into(),
                os: "macOS 26.0 (arm64)".into(),
                ..AboutInput::default()
            }),
            names: vec!["Studio".into()],
        }
    }

    #[test]
    fn the_report_is_redacted_and_names_the_failure() {
        let r = create_report(&input());
        assert_eq!(r.error_kind, "image_pull_auth");
        assert_eq!(
            r.subject,
            "Spaces: Linux container create failed (image_pull_auth)"
        );
        let ids: Vec<&str> = r.parts.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["details", "error", "environment"]);
        let all: String = r.parts.iter().map(|p| p.text.as_str()).collect();
        for leak in ["ghp_", "maya", "/Users/"] {
            assert!(!all.contains(leak), "{leak} in {all}");
        }
        assert!(
            r.parts[0]
                .text
                .contains("Failed while: downloading the image")
        );
        assert!(r.parts[0].text.contains("Time to failure: 42 s"));
        assert!(
            r.parts[1]
                .text
                .contains("The app showed: Could not download the image.")
        );
        assert_eq!(
            r.parts[2].text,
            "Cua Spaces for macOS 0.4.0 (0.4.0.200)\nmacOS 26.0 (arm64)"
        );
        assert!(!r.intro.contains('\u{2014}'), "no em dashes in UI copy");
        // The sheet says where it goes: an email to Cua support.
        assert!(
            r.intro
                .contains("email to Cua support (support@trycua.com)")
        );
        for public in ["public", "GitHub", "issue"] {
            assert!(!r.intro.contains(public), "{public} in {}", r.intro);
        }
    }

    /// The query value `name` of a `mailto:` link, decoded (line breaks as `\n`).
    fn mail_field(url: &str, name: &str) -> String {
        let query = url.split_once('?').expect("a query").1;
        let raw = query
            .split('&')
            .find_map(|kv| kv.strip_prefix(&format!("{name}=")))
            .unwrap_or_else(|| panic!("no {name} in {url}"));
        let mut bytes = Vec::new();
        let mut it = raw.bytes();
        while let Some(b) = it.next() {
            if b == b'%' {
                let hex: Vec<u8> = it.by_ref().take(2).collect();
                bytes.push(u8::from_str_radix(std::str::from_utf8(&hex).unwrap(), 16).unwrap());
            } else {
                bytes.push(b);
            }
        }
        String::from_utf8(bytes).unwrap().replace("\r\n", "\n")
    }

    fn body_len(url: &str) -> usize {
        url.split_once("&body=").expect("a body").1.len()
    }

    fn send_of(r: &CreateReport) -> ReportSend {
        ReportSend {
            subject: r.subject.clone(),
            parts: r.parts.clone(),
        }
    }

    #[test]
    fn the_link_is_a_private_email_with_only_the_kept_parts_redacted_again() {
        let r = create_report(&input());
        let mut send = send_of(&r);
        // The person left the environment out and pasted a secret in.
        send.parts.retain(|p| p.id != "environment");
        send.parts[1].text.push_str(concat!(
            "\nmy token=gh",
            "p_zzzzzzzzzzzzzzzzzzzzzzzzzzzzzz9 at sam@example.com"
        ));
        let url = report_mailto(&send, &[]);
        assert!(url.starts_with(
            "mailto:support@trycua.com?subject=Spaces%3A%20Linux%20container%20create%20failed"
        ));
        assert!(!url.contains("github"), "no GitHub address in {url}");
        for leak in ["ghp_", "sam%40example.com", "maya"] {
            assert!(!url.contains(leak), "{leak} in {url}");
        }
        assert!(url.contains("token%3D%3Credacted%3E"));
        assert!(url.contains("%0D%0A"), "line breaks follow RFC 6068");
        let body = mail_field(&url, "body");
        assert!(body.contains("What was created\nSpace: Linux container"));
        assert!(body.contains("Error\n"));
        assert!(body.contains("token=<redacted> at <email>"));
        assert!(
            !body.contains("App and system"),
            "a part left out stays out"
        );
        assert_eq!(
            mail_field(&url, "subject"),
            "Spaces: Linux container create failed (image_pull_auth)"
        );
    }

    #[test]
    fn a_long_error_is_cut_and_the_rest_of_the_report_stays() {
        let r = create_report(&input());
        let mut send = send_of(&r);
        send.parts[1].text = "word ".repeat(2000);
        let url = report_mailto(&send, &[]);
        assert!(body_len(&url) <= BODY_MAX, "{} chars", body_len(&url));
        let body = mail_field(&url, "body");
        assert!(body.contains("[cut: too long for an email link]"));
        assert!(body.contains("Space: Linux container"), "details stay");
        assert!(body.contains("Cua Spaces for macOS"), "environment stays");
        assert!(body.contains("word word"), "the start of the error stays");
    }

    #[test]
    fn a_report_that_fits_is_not_cut() {
        let url = report_mailto(&send_of(&create_report(&input())), &[]);
        assert!(body_len(&url) <= BODY_MAX);
        assert!(!mail_field(&url, "body").contains("[cut"));
    }

    #[test]
    fn the_whole_body_is_cut_when_the_other_parts_are_too_long() {
        let r = create_report(&input());
        let mut send = send_of(&r);
        send.parts[0].text = "é ".repeat(3000);
        let url = report_mailto(&send, &[]);
        assert!(body_len(&url) <= BODY_MAX, "{} chars", body_len(&url));
        assert!(mail_field(&url, "body").ends_with("[cut: too long for an email link]"));
    }

    #[test]
    fn the_subject_is_one_short_line() {
        let mut send = send_of(&create_report(&input()));
        send.subject = format!("first line\r\nBcc: x@example.com {}", "long ".repeat(100));
        let url = report_mailto(&send, &[]);
        let subject = mail_field(&url, "subject");
        assert!(!subject.contains('\n') && !subject.contains('\r'));
        assert!(subject.chars().count() <= SUBJECT_MAX);
        assert!(subject.starts_with("first line Bcc: <email>"), "{subject}");
        send.subject = "  ".into();
        assert_eq!(
            mail_field(&report_mailto(&send, &[]), "subject"),
            "Spaces: create failed"
        );
    }
}
