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

    /// Adds one variable.
    pub fn with(mut self, name: &str, value: impl ToString) -> Self {
        self.vars.insert(name.into(), value.to_string());
        self
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
