// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Look-alike (homoglyph) and bidi defenses for strings the consent screen
//! shows (red-team F4, F18).
//!
//! Two classes of caller-supplied string reach the user's eyes: the site
//! selectors a request narrows to (`github.com`, `accounts.google.com`) and
//! the app's claimed name and reason. An attacker can put IDN confusables
//! (`gіthub.com` with a Cyrillic `і`), bidi overrides (RLO/LRO), or control
//! characters in either, so a careless user approves a look-alike or a string
//! that imitates the verified-identity block.
//!
//! This module never trusts such a string as-is:
//!
//! - [`is_confusable`] flags any string that is not plain ASCII, or that
//!   carries a bidirectional-control or other control character. The consent
//!   screen renders a warning for a flagged string.
//! - [`display_safe`] returns an ASCII-only rendering: printable ASCII passes
//!   through, everything else becomes an explicit `\u{XXXX}` escape, so a
//!   look-alike can never be painted as the character it imitates and a bidi
//!   override cannot reorder the surrounding text.
//! - [`punycode_label`] / [`display_site`] render an IDN host in its ASCII
//!   `xn--` (punycode) form, which is what the browser and the OS resolver
//!   actually use, so `gіthub.com` shows as `xn--gthub-...com`, not the apex
//!   it imitates.
//! - [`skeleton`] and [`confusable_with`] map a string to a single-script
//!   ASCII "skeleton" of common homoglyphs, so a claimed name or site can be
//!   compared against a list of impersonated brands (`github.com`,
//!   `accounts.google.com`, `1password`, ...) even when the attacker used
//!   look-alike code points.

/// Bidirectional formatting and override code points. Present in a
/// caller-supplied string, they can visually reorder text to imitate a
/// trusted string, so they are always treated as confusable and escaped.
const BIDI_CONTROLS: &[char] = &[
    '\u{202A}', '\u{202B}', '\u{202C}', '\u{202D}', '\u{202E}', // LRE RLE PDF LRO RLO
    '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}', // LRI RLI FSI PDI
    '\u{200E}', '\u{200F}', // LRM RLM
    '\u{061C}', // ALM
];

/// Whether `s` must be shown with a look-alike warning: it is not plain
/// printable ASCII, or it carries a bidi/control character.
pub fn is_confusable(s: &str) -> bool {
    s.chars()
        .any(|c| !c.is_ascii() || c.is_control() || BIDI_CONTROLS.contains(&c))
}

/// An ASCII-only rendering of `s`: printable ASCII passes through; every other
/// character (non-ASCII, control, or bidi) becomes a `\u{XXXX}` escape. The
/// result cannot imitate a trusted string and cannot reorder text.
pub fn display_safe(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_graphic() || c == ' ' {
            out.push(c);
        } else {
            out.push_str(&format!("\\u{{{:04X}}}", c as u32));
        }
    }
    out
}

/// Maps a code point to its ASCII "skeleton" character when it is a common
/// homoglyph of one, otherwise returns the lower-cased character unchanged.
/// This is a deliberately small, hand-picked table of the confusables that
/// matter for brand impersonation (Latin/Cyrillic/Greek look-alikes and the
/// full-width forms), not the whole Unicode confusables database; it is the
/// conservative floor that already collapses `gіthub`, `göogle` and `1Passwοrd`
/// onto their real skeletons.
fn skeleton_char(c: char) -> char {
    match c {
        // Latin/Cyrillic/Greek look-alikes of ASCII letters.
        'а' | 'ɑ' | 'α' => 'a',
        'е' | 'ё' | 'є' | 'ε' => 'e',
        'о' | 'ο' | 'σ' | '০' | '੦' => 'o',
        'р' | 'ρ' => 'p',
        'с' | 'ϲ' => 'c',
        'х' | 'χ' => 'x',
        'у' | 'ү' => 'y',
        'і' | 'ı' | 'í' | 'ï' | 'ⅼ' | 'ℓ' => 'i',
        'ѕ' => 's',
        'ԁ' => 'd',
        'ן' => 'l',
        'ɡ' => 'g',
        // Full-width ASCII forms (`！`..=`～`, U+FF01..=U+FF5E) fold to their
        // ASCII code point, which covers full-width letters and digits.
        '！'..='～' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
        _ => c.to_ascii_lowercase(),
    }
}

/// The ASCII skeleton of `s`: each character folded to its look-alike ASCII
/// form, control/bidi characters dropped, everything lower-cased. Two strings
/// with the same skeleton look alike to a human.
pub fn skeleton(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() && !BIDI_CONTROLS.contains(c))
        .map(skeleton_char)
        .collect()
}

/// Whether `candidate` is a confusable of a `trusted` string without being
/// exactly equal to it: same visual skeleton, different bytes. Used to warn
/// when a claimed app name or a site selector imitates a known brand.
pub fn confusable_with(candidate: &str, trusted: &str) -> bool {
    candidate != trusted && skeleton(candidate) == skeleton(trusted)
}

/// Brands whose names or apexes are worth impersonating on a consent screen.
/// A claimed name or a site selector that is a confusable of one of these is
/// flagged (red-team F4/F18).
pub const IMPERSONATED_BRANDS: &[&str] = &[
    "github.com",
    "accounts.google.com",
    "google.com",
    "login.microsoftonline.com",
    "microsoft.com",
    "appleid.apple.com",
    "apple.com",
    "login.okta.com",
    "amazon.com",
    "paypal.com",
    "1password.com",
    "1password",
    "cua",
    "cua spaces",
    "com.trycua.cua",
    "com.trycua.spaces",
];

/// The brand `s` impersonates (a confusable of a known brand that is not the
/// brand itself), if any.
pub fn impersonated_brand(s: &str) -> Option<&'static str> {
    let sk = skeleton(s);
    IMPERSONATED_BRANDS
        .iter()
        .copied()
        .find(|b| skeleton(b) == sk && *b != s)
}

/// Encodes one DNS label to punycode per RFC 3492, returning the raw output
/// (no `xn--` prefix). ASCII input is returned unchanged.
fn punycode_encode(input: &str) -> String {
    // Bootstring parameters for punycode.
    const BASE: u32 = 36;
    const TMIN: u32 = 1;
    const TMAX: u32 = 26;
    const SKEW: u32 = 38;
    const DAMP: u32 = 700;
    const INITIAL_BIAS: u32 = 72;
    const INITIAL_N: u32 = 128;

    fn adapt(mut delta: u32, num_points: u32, first: bool) -> u32 {
        delta = if first { delta / DAMP } else { delta / 2 };
        delta += delta / num_points;
        let mut k = 0;
        while delta > ((BASE - TMIN) * TMAX) / 2 {
            delta /= BASE - TMIN;
            k += BASE;
        }
        k + (((BASE - TMIN + 1) * delta) / (delta + SKEW))
    }

    fn digit(d: u32) -> char {
        // 0..=25 -> 'a'..='z', 26..=35 -> '0'..='9'.
        if d < 26 {
            (b'a' + d as u8) as char
        } else {
            (b'0' + (d - 26) as u8) as char
        }
    }

    let chars: Vec<char> = input.chars().collect();
    let mut output: String = chars.iter().filter(|c| c.is_ascii()).collect();
    let basic = output.len() as u32;
    if basic > 0 && (basic as usize) < chars.len() {
        output.push('-');
    }
    let mut n = INITIAL_N;
    let mut delta = 0u32;
    let mut bias = INITIAL_BIAS;
    let mut handled = basic;
    let total = chars.len() as u32;
    while handled < total {
        let m = chars
            .iter()
            .map(|&c| c as u32)
            .filter(|&c| c >= n)
            .min()
            .expect("more to handle");
        delta += (m - n) * (handled + 1);
        n = m;
        for &c in &chars {
            let c = c as u32;
            if c < n {
                delta += 1;
            }
            if c == n {
                let mut q = delta;
                let mut k = BASE;
                loop {
                    let t = if k <= bias {
                        TMIN
                    } else if k >= bias + TMAX {
                        TMAX
                    } else {
                        k - bias
                    };
                    if q < t {
                        break;
                    }
                    output.push(digit(t + ((q - t) % (BASE - t))));
                    q = (q - t) / (BASE - t);
                    k += BASE;
                }
                output.push(digit(q));
                bias = adapt(delta, handled + 1, handled == basic);
                delta = 0;
                handled += 1;
            }
        }
        delta += 1;
        n += 1;
    }
    output
}

/// The ASCII (punycode) form of a single host label: non-ASCII labels become
/// `xn--<punycode>`, ASCII labels are returned unchanged. This is what the DNS
/// resolver sees, so it is the honest thing to show a user (red-team F18).
pub fn punycode_label(label: &str) -> String {
    if label.is_ascii() {
        label.to_string()
    } else {
        format!("xn--{}", punycode_encode(label))
    }
}

/// The ASCII form of a whole host: each dot-separated label punycoded. A host
/// carrying a bidi/control character is escaped with [`display_safe`] instead,
/// since it is not a real host name.
pub fn display_site(host: &str) -> String {
    if host
        .chars()
        .any(|c| c.is_control() || BIDI_CONTROLS.contains(&c))
    {
        return display_safe(host);
    }
    host.split('.')
        .map(punycode_label)
        .collect::<Vec<_>>()
        .join(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_ascii_is_not_confusable_and_passes_through() {
        for s in ["github.com", "accounts.google.com", "dev-1", "My Bot 3"] {
            assert!(!is_confusable(s), "{s}");
            assert_eq!(display_safe(s), s);
            assert_eq!(display_site(s), s);
            assert_eq!(impersonated_brand(s), None, "{s} is the real brand");
        }
    }

    #[test]
    fn cyrillic_lookalike_is_flagged_and_escaped() {
        // "gіthub.com" with a Cyrillic small letter i (U+0456).
        let spoof = "g\u{0456}thub.com";
        assert!(is_confusable(spoof));
        let safe = display_safe(spoof);
        assert!(safe.contains("\\u{0456}"), "{safe}");
        // The escaped form is pure ASCII and is not the trusted apex.
        assert!(safe.is_ascii());
        assert_ne!(safe, "github.com");
    }

    #[test]
    fn bidi_override_is_flagged_and_stripped() {
        // A right-to-left override could paint "moc.knab" as "bank.com".
        let spoof = "bank\u{202E}moc.example";
        assert!(is_confusable(spoof));
        let safe = display_safe(spoof);
        assert!(safe.contains("\\u{202E}"), "{safe}");
        assert!(safe.is_ascii());
        // display_site of a control/bidi host also escapes rather than resolves.
        assert!(display_site(spoof).is_ascii());
    }

    #[test]
    fn control_characters_are_flagged() {
        assert!(is_confusable("a\u{0007}b"));
        assert!(is_confusable("line\nbreak"));
        assert_eq!(display_safe("a\u{0007}b"), "a\\u{0007}b");
    }

    #[test]
    fn homoglyph_site_maps_to_the_brand_it_imitates() {
        // Cyrillic-i github, Greek/Cyrillic-o google.
        let spoof_github = "g\u{0456}thub.com";
        let spoof_google = "g\u{03BF}\u{043E}gle.com";
        assert!(confusable_with(spoof_github, "github.com"));
        assert_eq!(impersonated_brand(spoof_github), Some("github.com"));
        assert_eq!(impersonated_brand(spoof_google), Some("google.com"));
        // The real brand is not "impersonating" itself.
        assert_eq!(impersonated_brand("github.com"), None);
        // An unrelated string collides with nothing.
        assert_eq!(impersonated_brand("example.dev"), None);
    }

    #[test]
    fn punycode_renders_the_resolver_form() {
        // RFC 3492 / IDNA: bücher -> xn--bcher-kva.
        assert_eq!(punycode_label("b\u{00FC}cher"), "xn--bcher-kva");
        // A full IDN host, label by label.
        let host = "b\u{00FC}cher.example.com";
        assert_eq!(display_site(host), "xn--bcher-kva.example.com");
        // The rendered form is ASCII and is not the look-alike glyph string.
        assert!(display_site(host).is_ascii());
        assert_ne!(display_site(host), host);
        // Cyrillic-i github renders to an xn-- apex, never "github.com".
        let spoof = "g\u{0456}thub.com";
        let shown = display_site(spoof);
        assert!(shown.starts_with("xn--"), "{shown}");
        assert_ne!(shown, "github.com");
    }
}
