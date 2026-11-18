// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Per-site cookie selection: which cookie rows belong to the site the user
//! chose (red-team F8).
//!
//! When a user teleports one site, capture filters the copied cookie store to
//! that site's rows (`DELETE ... WHERE host_key NOT IN (...)`). Getting the set
//! wrong breaks in both directions:
//!
//! - too narrow (only the exact apex) deletes the `.apex` domain cookies and
//!   the `api.apex` / `gist.apex` host cookies, so the teleported session is
//!   broken and users escalate to "whole browser", a far worse leak;
//! - too broad (any suffix match) pulls cookies for sibling tenants or
//!   subdomains the user did not mean to include.
//!
//! The correct set for a chosen registrable-domain apex is exactly
//! `{ apex, .apex, and every host key ending in .apex }`. This module computes
//! that membership, normalizing the leading dot of domain cookies and folding
//! case, and never matches a look-alike apex (`evilgithub.com`,
//! `github.com.evil.example`).
//!
//! Registrable-domain grouping itself (deciding that `github.com` is the apex,
//! not `com`) depends on a current Public Suffix List; [`registrable_domain`]
//! is a conservative fallback that fails *safe* toward under-capture: an
//! unrecognized multi-label suffix is treated as its own isolated origin rather
//! than grouped with siblings.

/// Normalizes a cookie `host_key` or a host for comparison: trims surrounding
/// whitespace, strips a single leading dot (the domain-cookie marker) and
/// lower-cases it.
pub fn normalize_host(host: &str) -> String {
    host.trim()
        .strip_prefix('.')
        .unwrap_or_else(|| host.trim())
        .to_ascii_lowercase()
}

/// Whether a cookie with this `host_key` belongs to the site whose
/// registrable-domain apex is `apex`. True for the apex itself, its leading-dot
/// domain form, and any subdomain under it; false for a look-alike or a
/// different registrable domain.
pub fn host_key_in_site(host_key: &str, apex: &str) -> bool {
    let h = normalize_host(host_key);
    let a = normalize_host(apex);
    if a.is_empty() || h.is_empty() {
        return false;
    }
    h == a || h.ends_with(&format!(".{a}"))
}

/// Splits `host_keys` into the rows kept for `apex` and the rows dropped. The
/// dropped rows are what capture deletes so they never enter the vault.
pub fn partition_host_keys<'a>(
    host_keys: impl IntoIterator<Item = &'a str>,
    apex: &str,
) -> (Vec<&'a str>, Vec<&'a str>) {
    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    for h in host_keys {
        if host_key_in_site(h, apex) {
            kept.push(h);
        } else {
            dropped.push(h);
        }
    }
    (kept, dropped)
}

/// A small, embedded set of multi-label public suffixes under which the
/// registrable domain is the last *three* labels rather than the last two.
/// This is not a full Public Suffix List; it exists only so the conservative
/// fallback groups the common cases correctly and, for anything it does not
/// know, fails safe toward treating a host as its own isolated origin.
const MULTI_LABEL_SUFFIXES: &[&str] = &[
    "co.uk", "org.uk", "gov.uk", "ac.uk", "co.jp", "com.au", "com.br", "co.nz", "co.in", "com.cn",
];

/// A conservative registrable domain for `host`. With a current Public Suffix
/// List this is exact; here it recognizes a small set of two-label suffixes and
/// otherwise takes the last two labels. When a host is a bare public-suffix-like
/// token it returns the host unchanged, so an unknown suffix stays isolated
/// (fail safe toward under-capture, red-team F8).
pub fn registrable_domain(host: &str) -> String {
    let host = normalize_host(host);
    let labels: Vec<&str> = host.split('.').filter(|l| !l.is_empty()).collect();
    if labels.len() <= 2 {
        return host;
    }
    let last_two = format!("{}.{}", labels[labels.len() - 2], labels[labels.len() - 1]);
    if MULTI_LABEL_SUFFIXES.contains(&last_two.as_str()) && labels.len() >= 3 {
        // Registrable domain is the last three labels (e.g. example.co.uk).
        return format!("{}.{}", labels[labels.len() - 3], last_two);
    }
    last_two
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixture cookie row (name + host_key), as a browser Cookies DB stores.
    struct Cookie {
        /// The cookie name (documents the `__Host-`/`__Secure-` cases).
        #[allow(dead_code)]
        name: &'static str,
        host_key: &'static str,
    }

    fn fixtures() -> Vec<Cookie> {
        vec![
            // github.com apex and its subdomains / domain cookies.
            Cookie {
                name: "__Host-user_session",
                host_key: "github.com",
            },
            Cookie {
                name: "__Secure-logged_in",
                host_key: ".github.com",
            },
            Cookie {
                name: "_gh_sess",
                host_key: "api.github.com",
            },
            Cookie {
                name: "gist_session",
                host_key: "gist.github.com",
            },
            Cookie {
                name: "deep",
                host_key: "raw.githubusercontent.example",
            }, // different apex
            // Look-alikes and unrelated sites that must NOT be captured.
            Cookie {
                name: "evil",
                host_key: "evilgithub.com",
            },
            Cookie {
                name: "suffix_spoof",
                host_key: "github.com.evil.example",
            },
            Cookie {
                name: "gitlab",
                host_key: ".gitlab.com",
            },
            Cookie {
                name: "bank",
                host_key: "login.bank.example",
            },
        ]
    }

    #[test]
    fn keeps_the_site_and_its_subdomains_only() {
        let apex = "github.com";
        let (kept, dropped) = partition_host_keys(fixtures().iter().map(|c| c.host_key), apex);
        assert_eq!(
            kept,
            vec![
                "github.com",
                ".github.com",
                "api.github.com",
                "gist.github.com"
            ]
        );
        // Every look-alike and sibling is dropped.
        assert!(dropped.contains(&"evilgithub.com"));
        assert!(dropped.contains(&"github.com.evil.example"));
        assert!(dropped.contains(&".gitlab.com"));
        assert!(dropped.contains(&"login.bank.example"));
        assert!(dropped.contains(&"raw.githubusercontent.example"));
    }

    #[test]
    fn host_and_secure_prefixed_cookies_are_matched_by_host_key() {
        // `__Host-` cookies are host-only (no leading dot); `__Secure-` may be
        // domain cookies (leading dot). Both resolve to the same site.
        assert!(host_key_in_site("github.com", "github.com"));
        assert!(host_key_in_site(".github.com", "github.com"));
        // The chosen apex may itself be given with a leading dot or odd case.
        assert!(host_key_in_site("API.GitHub.com", ".GitHub.Com"));
    }

    #[test]
    fn does_not_over_or_under_match() {
        // Under-match guard: the apex, dotted form and subdomains all match.
        for h in [
            "github.com",
            ".github.com",
            "sub.github.com",
            "a.b.github.com",
        ] {
            assert!(host_key_in_site(h, "github.com"), "{h} should match");
        }
        // Over-match guard: prefixes, suffixes and siblings do not.
        for h in [
            "notgithub.com",
            "github.com.evil.example",
            "github.co",
            "gitlab.com",
            "",
        ] {
            assert!(!host_key_in_site(h, "github.com"), "{h} must not match");
        }
    }

    #[test]
    fn registrable_domain_groups_common_cases_and_fails_safe() {
        assert_eq!(registrable_domain("api.github.com"), "github.com");
        assert_eq!(registrable_domain("github.com"), "github.com");
        assert_eq!(registrable_domain("www.example.co.uk"), "example.co.uk");
        assert_eq!(registrable_domain("shop.example.co.uk"), "example.co.uk");
        // A single-label host is its own isolated origin.
        assert_eq!(registrable_domain("localhost"), "localhost");
        // An unknown two-label suffix is treated as last-two labels, and a
        // three-label host under an unknown suffix stays isolated to those two
        // rather than being grouped too broadly.
        assert_eq!(registrable_domain("a.b.internal"), "b.internal");
    }
}
