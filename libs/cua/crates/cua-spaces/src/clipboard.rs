//! The clipboard setting of a Space: when the viewers (native and HTML5)
//! may move text between this computer and the Space.
//!
//! * `viewing` (the default): only while a viewer window is focused and
//!   shown. Paste sends this computer's clipboard once, copy brings the
//!   Space's once, and a change the Space makes on its own asks first.
//! * `ask`: every transfer asks first.
//! * `off`: nothing moves.
//!
//! The setting is kept with the Space ([`crate::registry::SpaceMeta`]) and
//! handed to the viewers when they open. The agent tools
//! `computer_clipboard_*` act inside the Space and are not affected.

use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::{Error, Result, Spaces};

/// See the module docs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClipboardSetting {
    /// Only while you're viewing. The default.
    #[default]
    Viewing,
    /// Ask each time.
    Ask,
    /// Never.
    Off,
}

impl ClipboardSetting {
    /// The word the CLI, the SDKs and the viewers use.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Viewing => "viewing",
            Self::Ask => "ask",
            Self::Off => "off",
        }
    }

    /// Whether this is the default, which is not stored.
    pub fn is_default(&self) -> bool {
        *self == Self::Viewing
    }
}

impl FromStr for ClipboardSetting {
    type Err = Error;

    fn from_str(word: &str) -> Result<Self> {
        match word.trim().to_ascii_lowercase().as_str() {
            "viewing" => Ok(Self::Viewing),
            "ask" => Ok(Self::Ask),
            "off" => Ok(Self::Off),
            other => Err(Error::invalid(format!(
                "clipboard {other:?}: choose viewing (only while you're viewing), ask (ask each \
                 time) or off"
            ))),
        }
    }
}

/// Refuses `what` while the operator override `CUA_SECURE_DEFAULT=0` has the secure default off.
pub(crate) fn require_secure(what: &str) -> Result<()> {
    if crate::network::Mode::from_env() == crate::network::Mode::Secure {
        return Ok(());
    }
    Err(Error::invalid(format!(
        "{what} is part of the secure default. {}",
        cua_vmm::network::secure_default_off_message()
    )))
}

impl Spaces {
    /// Sets when this Space's viewers may move the clipboard. It applies to
    /// viewers opened after the change.
    pub fn set_clipboard(&self, space: &str, setting: ClipboardSetting) -> Result<()> {
        // Part of the secure default: without it the viewers keep the clipboard in
        // sync both ways and nothing would enforce the word.
        require_secure("The clipboard setting")?;
        let id = self.resolve(space)?;
        self.inner
            .registry
            .update_meta(&id.to_string(), |m| m.clipboard = setting)
    }

    /// The clipboard setting of a Space ([`ClipboardSetting::Viewing`] unless
    /// one was set).
    pub fn clipboard(&self, space: &str) -> ClipboardSetting {
        self.resolve(space)
            .ok()
            .and_then(|id| self.inner.registry.meta(&id.to_string()))
            .map(|m| m.clipboard)
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_only_while_viewing() {
        assert_eq!(ClipboardSetting::default(), ClipboardSetting::Viewing);
        assert!(ClipboardSetting::default().is_default());
        assert!(!ClipboardSetting::Off.is_default());
    }

    #[test]
    fn words_round_trip_and_unknown_words_are_refused() {
        for s in [
            ClipboardSetting::Viewing,
            ClipboardSetting::Ask,
            ClipboardSetting::Off,
        ] {
            assert_eq!(s.as_str().parse::<ClipboardSetting>().unwrap(), s);
            assert_eq!(
                serde_json::to_string(&s).unwrap(),
                format!("\"{}\"", s.as_str())
            );
        }
        assert_eq!(
            " ASK ".parse::<ClipboardSetting>().unwrap(),
            ClipboardSetting::Ask
        );
        let e = "always".parse::<ClipboardSetting>().unwrap_err();
        assert_eq!(e.tag(), "invalid_argument");
        assert!(e.to_string().contains("only while you're viewing"), "{e}");
    }
}
