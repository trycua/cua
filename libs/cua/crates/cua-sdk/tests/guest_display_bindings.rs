//! `GuestDisplay` carries a VNC password. It is exported as an object, not a
//! record, so no generated binding gets a field-dumping string form
//! (Python `__repr__`/`__str__`, Kotlin `toString`/data class, Swift
//! `description`/`CustomStringConvertible`, TypeScript `toString`/`toJSON`).
//! This reads the checked-in bindings and fails if one ever does, or if the
//! password stops being reachable only through `url_with_password`.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The source of the declaration starting at `start` up to the next
/// top-level declaration (a line starting with one of `next`).
fn body(file: &str, start: &str, next: &[&str]) -> String {
    let src = std::fs::read_to_string(root().join(file)).unwrap();
    let i = src
        .find(start)
        .unwrap_or_else(|| panic!("{file}: no `{start}`"));
    let rest = &src[i + start.len()..];
    let end = rest
        .lines()
        .scan(0usize, |off, l| {
            let at = *off;
            *off += l.len() + 1;
            Some((at, l))
        })
        .skip(1)
        .find(|(_, l)| next.iter().any(|n| l.starts_with(n)))
        .map(|(at, _)| at)
        .unwrap_or(rest.len());
    rest[..end].to_string()
}

fn assert_opaque(lang: &str, body: &str, forbidden: &[&str]) {
    for f in forbidden {
        assert!(
            !body.contains(f),
            "{lang}: GuestDisplay defines `{f}`; its string form could show the VNC password"
        );
    }
    assert!(
        body.contains("url_with_password") || body.contains("urlWithPassword"),
        "{lang}: GuestDisplay lost its explicit url_with_password accessor"
    );
}

#[test]
fn python_guest_display_has_no_repr_or_fields() {
    let b = body(
        "python/src/cua/_native.py",
        "\nclass GuestDisplay(",
        &["class ", "def ", "_Uniffi"],
    );
    assert_opaque(
        "python",
        &b,
        &["__repr__", "__str__", "@dataclass", "self.url ="],
    );
}

#[test]
fn kotlin_guest_display_is_not_a_data_class() {
    let src = std::fs::read_to_string(root().join("kotlin/src/main/kotlin/ai/cua/sdk/cua_sdk.kt"))
        .unwrap();
    assert!(!src.contains("data class GuestDisplay"));
    let b = body(
        "kotlin/src/main/kotlin/ai/cua/sdk/cua_sdk.kt",
        "open class GuestDisplay",
        &["open class ", "public object ", "data class ", "interface "],
    );
    assert_opaque("kotlin", &b, &["toString"]);
}

#[test]
fn swift_guest_display_is_not_a_struct_or_printable() {
    let src = std::fs::read_to_string(root().join("swift/Sources/CuaSDK/CuaSDK.swift")).unwrap();
    assert!(!src.contains("struct GuestDisplay"));
    assert!(!src.contains("extension GuestDisplay: CustomStringConvertible"));
    assert!(!src.contains("extension GuestDisplay: CustomDebugStringConvertible"));
    let b = body(
        "swift/Sources/CuaSDK/CuaSDK.swift",
        "open class GuestDisplay",
        &[
            "open class ",
            "public struct ",
            "public enum ",
            "public protocol ",
            "extension ",
        ],
    );
    assert_opaque("swift", &b, &["var description", "var debugDescription"]);
}

#[test]
fn typescript_guest_display_is_an_opaque_object() {
    let src = std::fs::read_to_string(root().join("typescript/src/native/cua_sdk.ts")).unwrap();
    assert!(!src.contains("export type GuestDisplay ="));
    let b = body(
        "typescript/src/native/cua_sdk.ts",
        "export class GuestDisplay ",
        &["export ", "const "],
    );
    assert_opaque("typescript", &b, &["toString(", "toJSON("]);
}
