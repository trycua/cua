//! Renaming a Space ([`Spaces::rename`]): a new name people see. Its id
//! (`local:<name>`, `relay:<machine>`, ...) never changes, so everything
//! keyed by it (credentials, labels, shares, thumbnails) stays as it is.
//!
//! - A Space of this device keeps the new name in `space-meta.json`
//!   ([`crate::registry::SpaceMeta::display_name`]); listings show it as
//!   [`SpaceInfo::display_name`].
//! - A Space that is a machine on the relay (one of your computers provides
//!   it, or you shared it) is renamed there too, so your other devices and
//!   the people it is shared with see the new name. A relay Space is
//!   renamed only there: its listed `name` is the relay's.
//! - Only the owner renames: a Space shared with you is refused.

use crate::{Error, Result, SpaceId, SpaceInfo, Spaces};

/// The longest Space name, in characters (a DNS label, as create allows).
pub const MAX_NAME: usize = 63;

/// Checks a new Space name by the rule a create's name follows: lowercase
/// letters, digits and dashes, starting and ending with a letter or digit,
/// at most [`MAX_NAME`]. Returns it trimmed.
pub fn check_space_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::invalid("A Space needs a name."));
    }
    if name.chars().count() > MAX_NAME {
        return Err(Error::invalid(format!(
            "Use at most {MAX_NAME} characters for a Space's name."
        )));
    }
    let ok = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    if !name.chars().all(|c| ok(c) || c == '-') || !name.starts_with(ok) || !name.ends_with(ok) {
        return Err(Error::invalid(
            "Use lowercase letters, digits and dashes in a Space's name, starting and ending with \
             a letter or digit.",
        ));
    }
    Ok(name.to_string())
}

impl Spaces {
    /// Gives Space `space` (any accepted spelling) the name `name`; its id
    /// stays the same. Returns the Space as it is listed now.
    ///
    /// The name follows [`check_space_name`] and may not be another of
    /// your Spaces' names. A Space that is a machine on the relay is
    /// renamed there first (your account must own it: a Space shared with
    /// you is refused with `permission_denied`), so nothing changes here
    /// when the relay refuses. Renaming back to the name it was created
    /// with drops the rename.
    pub async fn rename(&self, space: &str, name: &str) -> Result<SpaceInfo> {
        let name = check_space_name(name)?;
        let info = self.find(space).await?;
        if info.shown_name() == name {
            return Ok(info);
        }
        if self
            .list()?
            .iter()
            .any(|i| i.id != info.id && i.shown_name() == name)
        {
            return Err(Error::invalid(format!(
                "Another Space is already called {name}. Pick another name."
            )));
        }
        let id = SpaceId::parse(&info.id)?;
        #[cfg(feature = "spaces-agents")]
        self.rename_on_relay(&info, &id, &name).await?;
        #[cfg(not(feature = "spaces-agents"))]
        if matches!(id, SpaceId::Relay { .. }) {
            return Err(Error::Unsupported(
                "renaming a Space on the relay needs cua-spaces' spaces-agents feature".into(),
            ));
        }
        if !matches!(id, SpaceId::Relay { .. }) {
            let kept = if name == info.name {
                String::new()
            } else {
                name.clone()
            };
            self.inner
                .registry
                .update_meta(&info.id, |m| m.display_name = kept)?;
        }
        self.find(&info.id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_follows_the_create_rule() {
        assert_eq!(check_space_name("  qa-runner-2 ").unwrap(), "qa-runner-2");
        assert_eq!(check_space_name(&"a".repeat(63)).unwrap(), "a".repeat(63));
        for bad in [
            "",
            "   ",
            "QA",
            "qa runner",
            "-qa",
            "qa-",
            "qa_runner",
            "qa.local",
            "café",
        ] {
            let e = check_space_name(bad).unwrap_err();
            assert_eq!(e.tag(), "invalid_argument", "{bad:?}: {e}");
        }
        assert!(
            check_space_name(&"a".repeat(64))
                .unwrap_err()
                .to_string()
                .contains("at most 63")
        );
        assert!(
            check_space_name("")
                .unwrap_err()
                .to_string()
                .contains("needs a name")
        );
    }
}
