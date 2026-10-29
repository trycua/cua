//! Plain-words limits of a share link, shown next to the link by every
//! surface (CLI, SDK, MCP, app).

use crate::model::Share;

/// Shown when a browser cannot open the link: any path-mode link, and a
/// private or team link (the hosted relay has no browser sign-in yet).
pub const PATH_MODE: &str =
    "Browser viewing of private links is coming; this link opens for tools and signed-in devices.";

/// Whether `url` is a relay path-mode link (`<relay>/m/<machine>/share/...`).
pub fn is_path_mode(url: &str) -> bool {
    url::Url::parse(url)
        .map(|u| u.path().starts_with("/m/"))
        .unwrap_or_else(|_| url.contains("/m/"))
}

/// The notices for `share`.
pub fn notices(share: &Share) -> Vec<String> {
    if is_path_mode(&share.url) || share.access != crate::model::Access::Public {
        vec![PATH_MODE.to_string()]
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Access, Owner, Target};

    fn share(url: &str, access: Access, target: Target) -> Share {
        Share {
            handle: "share-1".into(),
            key: "k".into(),
            name: None,
            target,
            access,
            owner: Owner::default(),
            teams: vec![],
            created_ms: 0,
            expires_ms: None,
            url: url.into(),
        }
    }

    #[test]
    fn host_mode_has_no_notices() {
        let s = share(
            "https://pabc-1234.m.cua.sh/",
            Access::Public,
            Target::Port { port: 1 },
        );
        assert!(notices(&s).is_empty());
    }

    #[test]
    fn private_host_mode_links_still_say_browsers_cannot_open_them() {
        let s = share(
            "https://sabcd-0123456789ab.m.cua.sh/",
            Access::Private,
            Target::Port { port: 1 },
        );
        assert_eq!(notices(&s), vec![PATH_MODE.to_string()]);
    }

    #[test]
    fn path_mode_says_browsers_need_the_host_domain() {
        let s = share(
            "https://relay.cua.ai/m/0123/share/abcd/",
            Access::Private,
            Target::Port { port: 1 },
        );
        assert_eq!(notices(&s), vec![PATH_MODE.to_string()]);
    }
}
