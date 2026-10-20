// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A user-visible message as the core hands it to a shell: a stable code
//! plus variables, never English. The shell looks the code up in its own
//! catalog (the web UI's `en` namespaces, or the SwiftUI app's
//! `.xcstrings`) and fills the variables, so translations never touch Rust.
//! The code is the web UI's i18n key, for example `teleport.sensitive.history`.

use serde::{Deserialize, Serialize};

/// A message code and the variables its text uses (`{name}`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Msg {
    /// The catalog key, `<area>.<screen_or_component>.<element>`.
    pub code: String,
    /// Values for the text's variables.
    pub vars: std::collections::HashMap<String, String>,
}

impl Msg {
    /// A message without variables.
    pub fn new(code: &str) -> Self {
        Self {
            code: code.into(),
            vars: std::collections::HashMap::new(),
        }
    }

    /// Text that is data, not copy (a name, an address, an error from
    /// elsewhere): `common.literal`, whose text is `{text}`.
    pub fn literal(text: impl ToString) -> Self {
        Self::new("common.literal").with("text", text)
    }

    /// Adds one variable.
    pub fn with(mut self, name: &str, value: impl ToString) -> Self {
        self.vars.insert(name.into(), value.to_string());
        self
    }
}

#[cfg(test)]
impl Msg {
    /// The message as the web UI's `en` catalog words it
    /// (`apps/cua-spaces/src/i18n/locales/en`), the variables filled in.
    /// Panics on a code the catalog does not have.
    pub(crate) fn english(&self) -> String {
        use std::sync::OnceLock;
        static CATALOG: OnceLock<std::collections::HashMap<String, String>> = OnceLock::new();
        let catalog = CATALOG.get_or_init(|| {
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../../apps/cua-spaces/src/i18n/locales/en");
            let mut all = std::collections::HashMap::new();
            for entry in std::fs::read_dir(&dir).expect("the en catalog") {
                let path = entry.unwrap().path();
                if path.extension().is_some_and(|e| e == "json") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    let map: std::collections::HashMap<String, serde_json::Value> =
                        serde_json::from_str(&text).unwrap();
                    all.extend(
                        map.into_iter()
                            .filter_map(|(k, v)| v.as_str().map(|v| (k, v.to_string()))),
                    );
                }
            }
            all
        });
        let mut text = catalog
            .get(&self.code)
            .unwrap_or_else(|| panic!("`{}` is not in the en catalog", self.code))
            .clone();
        for (name, value) in &self.vars {
            text = text.replace(&format!("{{{name}}}"), value);
        }
        text
    }
}

/// [`Msg::english`] for an optional message.
#[cfg(test)]
pub(crate) trait OptionMsgEnglish {
    fn english(&self) -> Option<String>;
}

#[cfg(test)]
impl OptionMsgEnglish for Option<Msg> {
    fn english(&self) -> Option<String> {
        self.as_ref().map(Msg::english)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_is_a_code_and_variables() {
        let m = Msg::new("x.y").with("count", 3);
        assert_eq!(m.code, "x.y");
        assert_eq!(m.vars["count"], "3");
        assert_eq!(
            serde_json::to_string(&Msg::new("x.y")).unwrap(),
            r#"{"code":"x.y","vars":{}}"#
        );
    }
}
