//! Moving a vault item from the legacy service name to the current one.
//!
//! Builds before CUA-1877 kept the session, the device key and the other
//! named secrets under a service named after the retired Cloud API host
//! (`run.cua.ai`, `staging.run.cua.ai`). The current builds use `ai.cua.session`
//! (`ai.cua.session.staging`); the account names stay as they were
//! (`cua-cli`, `cua-cli.<name>`). The first read that finds the current item
//! missing moves the legacy one over, so nobody has to sign in again.
//!
//! The move never loses a secret and never leaves two live copies behind:
//!
//! 1. read the legacy item (a read that needs a prompt this process may not
//!    show fails here, and nothing has changed);
//! 2. write the current item (a failed write leaves the legacy item as it
//!    was);
//! 3. read the current item back and compare (a copy that cannot be read
//!    back is deleted again, and the legacy item stays);
//! 4. only then delete the legacy item. If that fails the current item
//!    wins from now on and the legacy one is ignored.
//!
//! So a crash, a refusal or a locked keychain at any step leaves the user
//! with at least the session they had, and running the move again is safe: a
//! missing legacy item is "nothing to do", and the callers only start a move
//! while the current item is missing.
//!
//! The logic is generic over a [`Vault`] so it is tested on every platform;
//! macOS plugs in the keychain, Windows the Credential Manager.
//!
//! **Lifetime.** This code and `CuaEnv::legacy_keyring_service` are for the
//! releases around the rename. Remove them two releases or 60 days after the
//! first release that carries the move, whichever is later.

/// A vault that holds text items per `(service, account)`.
pub(crate) trait Vault {
    type Err;
    /// The item, or `None` when there is none.
    fn read(&self, service: &str, account: &str) -> Result<Option<String>, Self::Err>;
    /// Creates or replaces the item.
    fn write(&self, service: &str, account: &str, text: &str) -> Result<(), Self::Err>;
    /// Deletes the item; a missing one is not an error.
    fn delete(&self, service: &str, account: &str) -> Result<(), Self::Err>;
}

/// Why a move did not complete. The legacy item is intact in every case.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Failure<E> {
    /// The legacy item could not be read (typically: it needs a prompt).
    Read(E),
    /// The current item could not be written.
    Write(E),
    /// The current item was written but did not read back the same, so it
    /// was deleted again.
    Unverified,
}

/// Moves `account` from `legacy` to `current`. `Ok(None)`: there is no legacy
/// item. `Ok(Some(text))`: the item is now under `current` (and, unless its
/// deletion was refused, gone from `legacy`).
///
/// The caller has checked that `current` has no item for `account`: this
/// never overwrites one.
pub(crate) fn move_item<V: Vault>(
    vault: &V,
    legacy: &str,
    current: &str,
    account: &str,
) -> Result<Option<String>, Failure<V::Err>> {
    let Some(text) = vault.read(legacy, account).map_err(Failure::Read)? else {
        return Ok(None);
    };
    vault
        .write(current, account, &text)
        .map_err(Failure::Write)?;
    if !matches!(vault.read(current, account), Ok(Some(back)) if back == text) {
        let _ = vault.delete(current, account);
        return Err(Failure::Unverified);
    }
    // Best effort: a legacy item that cannot be deleted (not ours to delete,
    // or a prompt this process may not show) is simply not used any more.
    let _ = vault.delete(legacy, account);
    Ok(Some(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    const LEGACY: &str = "run.cua.ai";
    const CURRENT: &str = "ai.cua.session";

    /// An in-memory vault whose operations can be made to fail.
    #[derive(Default)]
    struct Fake {
        items: RefCell<BTreeMap<(String, String), String>>,
        fail_read: RefCell<Vec<(String, String)>>,
        fail_write: RefCell<bool>,
        fail_delete: RefCell<Vec<(String, String)>>,
        /// Writes to `CURRENT` store this instead of the text.
        garble_write: RefCell<bool>,
        log: RefCell<Vec<String>>,
    }

    impl Fake {
        fn with(items: &[(&str, &str, &str)]) -> Self {
            let f = Self::default();
            for (s, a, t) in items {
                f.items
                    .borrow_mut()
                    .insert((s.to_string(), a.to_string()), t.to_string());
            }
            f
        }
        fn get(&self, s: &str, a: &str) -> Option<String> {
            self.items
                .borrow()
                .get(&(s.to_string(), a.to_string()))
                .cloned()
        }
        fn log(&self) -> Vec<String> {
            self.log.borrow().clone()
        }
    }

    impl Vault for Fake {
        type Err = String;
        fn read(&self, s: &str, a: &str) -> Result<Option<String>, String> {
            self.log.borrow_mut().push(format!("read {s} {a}"));
            if self.fail_read.borrow().contains(&(s.into(), a.into())) {
                return Err("needs a prompt".into());
            }
            Ok(self.get(s, a))
        }
        fn write(&self, s: &str, a: &str, t: &str) -> Result<(), String> {
            self.log.borrow_mut().push(format!("write {s} {a}"));
            if *self.fail_write.borrow() {
                return Err("write refused".into());
            }
            let stored = if *self.garble_write.borrow() {
                format!("{t}?")
            } else {
                t.to_string()
            };
            self.items
                .borrow_mut()
                .insert((s.to_string(), a.to_string()), stored);
            Ok(())
        }
        fn delete(&self, s: &str, a: &str) -> Result<(), String> {
            self.log.borrow_mut().push(format!("delete {s} {a}"));
            if self.fail_delete.borrow().contains(&(s.into(), a.into())) {
                return Err("delete refused".into());
            }
            self.items
                .borrow_mut()
                .remove(&(s.to_string(), a.to_string()));
            Ok(())
        }
    }

    fn run(f: &Fake, account: &str) -> Result<Option<String>, Failure<String>> {
        move_item(f, LEGACY, CURRENT, account)
    }

    #[test]
    fn a_legacy_item_moves_and_the_old_one_is_deleted() {
        let f = Fake::with(&[(LEGACY, "cua-cli", "session")]);
        assert_eq!(run(&f, "cua-cli"), Ok(Some("session".into())));
        assert_eq!(f.get(CURRENT, "cua-cli").as_deref(), Some("session"));
        assert_eq!(f.get(LEGACY, "cua-cli"), None);
        // The new item is written and verified before the old one goes.
        assert_eq!(
            f.log(),
            [
                format!("read {LEGACY} cua-cli"),
                format!("write {CURRENT} cua-cli"),
                format!("read {CURRENT} cua-cli"),
                format!("delete {LEGACY} cua-cli"),
            ]
        );
    }

    #[test]
    fn without_a_legacy_item_nothing_happens() {
        let f = Fake::default();
        assert_eq!(run(&f, "cua-cli"), Ok(None));
        assert_eq!(f.get(CURRENT, "cua-cli"), None);
        assert_eq!(f.log(), [format!("read {LEGACY} cua-cli")]);
    }

    #[test]
    fn a_failed_read_leaves_the_old_item_intact_and_creates_nothing() {
        let f = Fake::with(&[(LEGACY, "cua-cli", "session")]);
        f.fail_read
            .borrow_mut()
            .push((LEGACY.into(), "cua-cli".into()));
        assert_eq!(
            run(&f, "cua-cli"),
            Err(Failure::Read("needs a prompt".into()))
        );
        assert_eq!(f.get(LEGACY, "cua-cli").as_deref(), Some("session"));
        assert_eq!(f.get(CURRENT, "cua-cli"), None);
        // It never wrote or deleted anything.
        assert_eq!(f.log(), [format!("read {LEGACY} cua-cli")]);
    }

    #[test]
    fn a_failed_write_leaves_the_old_item_intact() {
        let f = Fake::with(&[(LEGACY, "cua-cli", "session")]);
        *f.fail_write.borrow_mut() = true;
        assert_eq!(
            run(&f, "cua-cli"),
            Err(Failure::Write("write refused".into()))
        );
        assert_eq!(f.get(LEGACY, "cua-cli").as_deref(), Some("session"));
        assert_eq!(f.get(CURRENT, "cua-cli"), None);
    }

    #[test]
    fn a_copy_that_does_not_read_back_is_dropped_and_the_old_item_stays() {
        let f = Fake::with(&[(LEGACY, "cua-cli", "session")]);
        *f.garble_write.borrow_mut() = true;
        assert_eq!(run(&f, "cua-cli"), Err(Failure::Unverified));
        assert_eq!(f.get(LEGACY, "cua-cli").as_deref(), Some("session"));
        assert_eq!(f.get(CURRENT, "cua-cli"), None);
        // A copy that cannot be read at all is the same.
        let f = Fake::with(&[(LEGACY, "cua-cli", "session")]);
        f.fail_read
            .borrow_mut()
            .push((CURRENT.into(), "cua-cli".into()));
        assert_eq!(run(&f, "cua-cli"), Err(Failure::Unverified));
        assert_eq!(f.get(LEGACY, "cua-cli").as_deref(), Some("session"));
        assert_eq!(f.get(CURRENT, "cua-cli"), None);
    }

    #[test]
    fn a_refused_delete_keeps_the_move() {
        let f = Fake::with(&[(LEGACY, "cua-cli", "session")]);
        f.fail_delete
            .borrow_mut()
            .push((LEGACY.into(), "cua-cli".into()));
        assert_eq!(run(&f, "cua-cli"), Ok(Some("session".into())));
        assert_eq!(f.get(CURRENT, "cua-cli").as_deref(), Some("session"));
        assert_eq!(f.get(LEGACY, "cua-cli").as_deref(), Some("session"));
    }

    #[test]
    fn running_it_again_is_a_no_op() {
        let f = Fake::with(&[
            (LEGACY, "cua-cli", "session"),
            (LEGACY, "cua-cli.device-key", "key"),
        ]);
        for account in ["cua-cli", "cua-cli.device-key"] {
            assert!(run(&f, account).unwrap().is_some());
        }
        let after = f.items.borrow().clone();
        // The legacy items are gone, so a second run finds nothing to move
        // and changes nothing.
        for account in ["cua-cli", "cua-cli.device-key"] {
            assert_eq!(run(&f, account), Ok(None));
        }
        assert_eq!(*f.items.borrow(), after);
        assert_eq!(f.get(CURRENT, "cua-cli.device-key").as_deref(), Some("key"));
    }

    #[test]
    fn each_account_moves_on_its_own() {
        let f = Fake::with(&[
            (LEGACY, "cua-cli", "session"),
            (LEGACY, "cua-cli.device-key", "key"),
        ]);
        // The device key needs a prompt; the session does not.
        f.fail_read
            .borrow_mut()
            .push((LEGACY.into(), "cua-cli.device-key".into()));
        assert!(run(&f, "cua-cli").is_ok());
        assert!(run(&f, "cua-cli.device-key").is_err());
        assert_eq!(f.get(CURRENT, "cua-cli").as_deref(), Some("session"));
        assert_eq!(f.get(CURRENT, "cua-cli.device-key"), None);
        assert_eq!(f.get(LEGACY, "cua-cli.device-key").as_deref(), Some("key"));
    }
}
