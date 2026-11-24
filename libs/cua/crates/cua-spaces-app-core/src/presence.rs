// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Who this app is on a Space's presence: the name on its cursor's pill for
//! everyone else, and the stable principal id it joins as.
//!
//! Both shells join with these, so a person reads the same on every client:
//! the signed-in account's name, else the local part of its email, else its
//! username, else this computer's account (full name, then short name). A
//! presence name is never empty and never reads as an agent or as "You"
//! (the viewer's own cursor has no pill; "You" would mislabel it for
//! everyone else).

/// Names a person must never show as: they read as an agent, as the viewer
/// itself, or as nobody.
const RESERVED: &[&str] = &["cua agent", "agent", "you", "anonymous"];

/// Shown when nothing names the user at all.
pub const FALLBACK_NAME: &str = "Cua user";

fn usable(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| {
        !value.is_empty() && !RESERVED.contains(&value.to_ascii_lowercase().as_str())
    })
}

/// The local part of an email (`dana` for `dana@example.com`); a value
/// without `@` is returned as is.
fn local_part(value: &str) -> &str {
    value.split('@').next().unwrap_or(value)
}

/// The display name on a presence cursor.
///
/// `name`, `email` and `username` are the signed-in account's claims (all
/// `None` when signed out); `os_full_name` and `os_user` are this computer's
/// account.
pub fn presence_name(
    name: Option<&str>,
    email: Option<&str>,
    username: Option<&str>,
    os_full_name: Option<&str>,
    os_user: Option<&str>,
) -> String {
    let email = usable(email).and_then(|email| usable(Some(local_part(email))));
    let username = usable(username).and_then(|user| usable(Some(local_part(user))));
    usable(name)
        .or(email)
        .or(username)
        .or(usable(os_full_name))
        .or(usable(os_user))
        .unwrap_or(FALLBACK_NAME)
        .to_owned()
}

/// The principal id a user joins presence as, stable across launches and
/// the same in both apps: `user:<email>`, else `user:<subject>`, else
/// `user:<username>` for an account; `user:local:<os user>` signed out.
pub fn presence_principal_id(
    email: Option<&str>,
    subject: Option<&str>,
    username: Option<&str>,
    os_user: Option<&str>,
) -> String {
    fn nonempty(value: Option<&str>) -> Option<&str> {
        value.map(str::trim).filter(|value| !value.is_empty())
    }
    match nonempty(email).or(nonempty(subject)).or(nonempty(username)) {
        Some(account) => format!("user:{account}"),
        None => format!("user:local:{}", nonempty(os_user).unwrap_or("user")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_account_name_wins() {
        assert_eq!(
            presence_name(
                Some("Dillon Dupont"),
                Some("dillon@trycua.com"),
                Some("dillon"),
                Some("Dillon D"),
                Some("dd")
            ),
            "Dillon Dupont"
        );
    }

    #[test]
    fn then_the_email_local_part_then_the_username() {
        assert_eq!(
            presence_name(
                None,
                Some("dana@example.com"),
                Some("dana42"),
                None,
                Some("d")
            ),
            "dana"
        );
        assert_eq!(
            presence_name(Some("  "), None, Some("dana42"), None, None),
            "dana42"
        );
        // A username that is itself an email shows its local part.
        assert_eq!(
            presence_name(None, None, Some("lee@example.com"), None, None),
            "lee"
        );
    }

    #[test]
    fn signed_out_uses_this_computers_account() {
        assert_eq!(
            presence_name(None, None, None, Some("Dillon Dupont"), Some("ada")),
            "Dillon Dupont"
        );
        assert_eq!(
            presence_name(None, None, None, Some(""), Some("ada")),
            "ada"
        );
    }

    #[test]
    fn never_an_agent_you_or_empty() {
        for reserved in [
            "CUA agent",
            "cua agent",
            "You",
            "you",
            "Agent",
            "Anonymous",
            "",
            "  ",
        ] {
            let name = presence_name(Some(reserved), None, None, Some(reserved), Some(reserved));
            assert_eq!(name, FALLBACK_NAME, "{reserved:?}");
        }
        assert_eq!(
            presence_name(
                Some("You"),
                Some("you@example.com"),
                None,
                None,
                Some("sam")
            ),
            "sam"
        );
        assert!(!presence_name(None, None, None, None, None).is_empty());
    }

    #[test]
    fn principal_ids_are_stable_per_account() {
        assert_eq!(
            presence_principal_id(Some("dana@example.com"), Some("sub-1"), None, Some("d")),
            "user:dana@example.com"
        );
        assert_eq!(
            presence_principal_id(None, Some("sub-1"), Some("dana"), None),
            "user:sub-1"
        );
        assert_eq!(
            presence_principal_id(None, None, None, Some("ada")),
            "user:local:ada"
        );
        assert_eq!(
            presence_principal_id(None, None, None, None),
            "user:local:user"
        );
    }
}
