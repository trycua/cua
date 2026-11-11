// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Process-environment reads that unit tests can override per thread.
//!
//! Unit tests share one process and run on parallel threads, so they must
//! never call `std::env::set_var` or `remove_var`. On glibc that can free the
//! `environ` array while another thread walks it, and libc's own readers (the
//! resolver inside `getaddrinfo`, libX11) take no lock, so the test binary
//! crashes with SIGSEGV. Tests use [`Overrides`] instead; it only affects the
//! calling thread and the child processes it configures with
//! [`Overrides::apply`].

use std::ffi::OsString;

/// `std::env::var_os`, seen through the calling test thread's overrides.
pub(crate) fn var_os(name: &str) -> Option<OsString> {
    #[cfg(test)]
    if let Some(value) = overrides::get(name) {
        return value;
    }
    std::env::var_os(name)
}

/// `std::env::var(name).ok()`, seen through the calling test thread's overrides.
pub(crate) fn var(name: &str) -> Option<String> {
    var_os(name).and_then(|value| value.into_string().ok())
}

#[cfg(test)]
pub(crate) use overrides::Overrides;

#[cfg(test)]
mod overrides {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::ffi::{OsStr, OsString};

    thread_local! {
        static OVERRIDES: RefCell<HashMap<String, Option<OsString>>> = RefCell::new(HashMap::new());
    }

    pub(super) fn get(name: &str) -> Option<Option<OsString>> {
        OVERRIDES.with(|map| map.borrow().get(name).cloned())
    }

    /// Thread-local environment overrides, restored when dropped.
    #[must_use]
    #[derive(Default)]
    pub(crate) struct Overrides {
        saved: Vec<(String, Option<Option<OsString>>)>,
    }

    impl Overrides {
        pub(crate) fn new() -> Self {
            Self::default()
        }

        pub(crate) fn set(&mut self, name: &str, value: impl AsRef<OsStr>) -> &mut Self {
            self.put(name, Some(value.as_ref().to_owned()))
        }

        pub(crate) fn unset(&mut self, name: &str) -> &mut Self {
            self.put(name, None)
        }

        fn put(&mut self, name: &str, value: Option<OsString>) -> &mut Self {
            let previous = OVERRIDES.with(|map| map.borrow_mut().insert(name.to_owned(), value));
            if !self.saved.iter().any(|(saved, _)| saved == name) {
                self.saved.push((name.to_owned(), previous));
            }
            self
        }

        /// Give a child process this thread's view of the environment.
        pub(crate) fn apply(command: &mut std::process::Command) {
            OVERRIDES.with(|map| {
                for (name, value) in map.borrow().iter() {
                    match value {
                        Some(value) => command.env(name, value),
                        None => command.env_remove(name),
                    };
                }
            });
        }
    }

    impl Drop for Overrides {
        fn drop(&mut self) {
            OVERRIDES.with(|map| {
                let mut map = map.borrow_mut();
                for (name, previous) in self.saved.drain(..).rev() {
                    match previous {
                        Some(value) => map.insert(name, value),
                        None => map.remove(&name),
                    };
                }
            });
        }
    }

    #[test]
    fn overrides_are_per_thread_and_restored_on_drop() {
        let name = "CUA_DRIVER_ENV_VARS_TEST_ONLY";
        {
            let mut env = Overrides::new();
            env.set(name, "outer");
            {
                let mut inner = Overrides::new();
                inner.set(name, "inner");
                assert_eq!(super::var(name).as_deref(), Some("inner"));
                std::thread::spawn(move || assert_eq!(super::var(name), None))
                    .join()
                    .unwrap();
            }
            assert_eq!(super::var(name).as_deref(), Some("outer"));
            env.unset(name);
            assert_eq!(super::var(name), None);
        }
        assert_eq!(get(name), None);
    }
}
