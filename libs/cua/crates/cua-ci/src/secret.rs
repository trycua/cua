//! Where a pool's GitHub credential is kept.
//!
//! On macOS: the login keychain, as a generic password with service
//! [`APP_KEY_SERVICE`] (or [`TOKEN_SERVICE`]) and the pool name as the
//! account; the value is the App's PEM private key, base64-encoded. Read it
//! by hand with `security find-generic-password -s cua-ci-github-app
//! -a <pool> -w`. Elsewhere, or with `secret_store = "file"` (a Mac set up
//! over SSH, whose keychain is locked): a file under
//! `$CUA_HOME/ci/secrets/`, readable by its owner only (0600), with the
//! same base64 value.
//!
//! A GitHub App key can also stay in a file of its own
//! ([`APP_KEY_FILE_ENV`], or `private_key_file` in the pool's `[github]`
//! table): the way for a headless host or one set up over SSH, whose login
//! keychain cannot be used without someone at the screen. The file must be
//! the user's own and readable by them only (0600); [`resolve_app_key`]
//! prefers it over the store.
//!
//! The credential is read by `cua ci serve` on this computer only. It is
//! never logged ([`Secret`] prints as `[redacted]`), never written to the
//! pool's configuration and never passed into a Space.

use std::path::{Path, PathBuf};

use base64::Engine as _;

use crate::{Error, Result};

/// The keychain service of a GitHub App private key.
pub const APP_KEY_SERVICE: &str = "cua-ci-github-app";

/// The keychain service of a GitHub token.
pub const TOKEN_SERVICE: &str = "cua-ci-github-token";

/// The environment variable naming a GitHub App private key file; it wins
/// over the pool's `private_key_file`.
pub const APP_KEY_FILE_ENV: &str = "CUA_CI_GITHUB_APP_KEY_FILE";

/// The App key file to use: [`APP_KEY_FILE_ENV`] when set (and not empty),
/// else `configured` (the pool's `private_key_file`).
pub fn app_key_file(configured: Option<&Path>) -> Option<PathBuf> {
    match std::env::var_os(APP_KEY_FILE_ENV) {
        Some(v) if !v.is_empty() => Some(PathBuf::from(v)),
        _ => configured.map(Path::to_path_buf),
    }
}

/// Reads a private key (or token) file into memory. Refused, with the
/// reason, when it is not a regular file, not owned by this user, or
/// readable or writable by anyone else (anything but 0600 or 0400). The
/// value is never logged or printed.
pub fn read_private_file(path: &Path) -> Result<Secret> {
    use std::io::Read as _;
    let mut f =
        std::fs::File::open(path).map_err(|e| Error::Secret(format!("{}: {e}", path.display())))?;
    // Checked on the open file, so the file checked is the file read.
    let meta = f
        .metadata()
        .map_err(|e| Error::Secret(format!("{}: {e}", path.display())))?;
    check_private(path, &meta)?;
    let mut text = String::new();
    f.read_to_string(&mut text)
        .map_err(|e| Error::Secret(format!("{}: {e}", path.display())))?;
    if text.trim().is_empty() {
        return Err(Error::Secret(format!("{} is empty", path.display())));
    }
    Ok(Secret::new(text))
}

/// The ownership and permission rules of [`read_private_file`].
pub fn check_private(path: &Path, meta: &std::fs::Metadata) -> Result<()> {
    if !meta.is_file() {
        return Err(Error::Secret(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        // SAFETY: geteuid has no preconditions and cannot fail.
        let me = unsafe { libc::geteuid() };
        if meta.uid() != me {
            return Err(Error::Secret(format!(
                "{} belongs to another user (uid {}); the key file must be yours",
                path.display(),
                meta.uid()
            )));
        }
        let mode = meta.mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(Error::Secret(format!(
                "{} is readable or writable by others (mode {mode:03o}); make it yours only: \
                 chmod 600 {}",
                path.display(),
                path.display()
            )));
        }
    }
    Ok(())
}

/// Where a pool's GitHub App key came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeySource {
    /// A key file (its path).
    File(PathBuf),
    /// The credential store (its description).
    Store(String),
}

impl std::fmt::Display for KeySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeySource::File(p) => write!(f, "the key file {}", p.display()),
            KeySource::Store(d) => f.write_str(d),
        }
    }
}

/// The credential, in this order: the key `file` (from [`app_key_file`]),
/// then `store` at `service`/`account`. A key file that is set but cannot
/// be used is an error, never a silent fall back to the store. `None` when
/// neither has one.
pub fn resolve_app_key(
    file: Option<&Path>,
    store: &dyn SecretStore,
    service: &str,
    account: &str,
) -> Result<Option<(Secret, KeySource)>> {
    if let Some(p) = file {
        return read_private_file(p).map(|s| Some((s, KeySource::File(p.to_path_buf()))));
    }
    Ok(store
        .get(service, account)?
        .map(|s| (s, KeySource::Store(store.describe(service, account)))))
}

/// A secret value. Its `Debug` and `Display` print `[redacted]`; only
/// [`Secret::expose`] gives the value.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wraps a value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The value. Never log it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[redacted]")
    }
}

impl std::fmt::Display for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[redacted]")
    }
}

/// A credential store.
pub trait SecretStore: Send + Sync {
    /// The value at `service`/`account`, or `None`.
    fn get(&self, service: &str, account: &str) -> Result<Option<Secret>>;
    /// Stores (or replaces) the value.
    fn set(&self, service: &str, account: &str, value: &Secret) -> Result<()>;
    /// Removes it; `false` when there was none.
    fn delete(&self, service: &str, account: &str) -> Result<bool>;
    /// Where it is, for people ("the login keychain", a path).
    fn describe(&self, service: &str, account: &str) -> String;
}

/// The values are stored base64-encoded (a PEM key keeps its newlines).
fn encode(v: &Secret) -> String {
    base64::engine::general_purpose::STANDARD.encode(v.expose().as_bytes())
}

fn decode(stored: &[u8]) -> Result<Secret> {
    let text = std::str::from_utf8(stored)
        .map_err(|_| Error::Secret("the stored credential is not text".into()))?;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .map_err(|_| Error::Secret("the stored credential is not base64".into()))?;
    String::from_utf8(raw)
        .map(Secret)
        .map_err(|_| Error::Secret("the stored credential is not UTF-8".into()))
}

/// Which store a pool's credential is in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StoreKind {
    /// The macOS login keychain (a 0600 file elsewhere).
    #[default]
    Keychain,
    /// A 0600 file under `$CUA_HOME/ci/secrets/`.
    File,
}

impl StoreKind {
    /// `keychain` or `file`.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "keychain" => Ok(StoreKind::Keychain),
            "file" => Ok(StoreKind::File),
            other => Err(Error::Config(format!(
                "secret store {other:?}: keychain or file"
            ))),
        }
    }
}

/// The store `kind` names: the login keychain on macOS (a 0600 file under
/// `secrets_dir` on other systems, which have no login keychain), or the
/// file.
pub fn store(kind: StoreKind, secrets_dir: PathBuf) -> Box<dyn SecretStore> {
    match kind {
        #[cfg(target_os = "macos")]
        StoreKind::Keychain => {
            let _ = secrets_dir;
            Box::new(KeychainStore)
        }
        _ => Box::new(FileStore::new(secrets_dir)),
    }
}

/// Files under a directory, one per service and account, mode 0600.
pub struct FileStore {
    dir: PathBuf,
}

impl FileStore {
    /// Under `dir` (created 0700 on first write).
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn path(&self, service: &str, account: &str) -> Result<PathBuf> {
        let ok = |s: &str| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
                && !s.starts_with('.')
        };
        if !ok(service) || !ok(account) {
            return Err(Error::Secret(format!(
                "credential name {service}/{account}: letters, digits, '-', '_' and '.' only"
            )));
        }
        Ok(self.dir.join(format!("{service}.{account}")))
    }
}

impl SecretStore for FileStore {
    fn get(&self, service: &str, account: &str) -> Result<Option<Secret>> {
        let p = self.path(service, account)?;
        match std::fs::read(&p) {
            Ok(b) => decode(&b).map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(Error::Secret(format!("{}: {e}", p.display()))),
        }
    }

    fn set(&self, service: &str, account: &str, value: &Secret) -> Result<()> {
        let p = self.path(service, account)?;
        cua_home::guard_write(&p)?;
        std::fs::create_dir_all(&self.dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o700))?;
        }
        cua_home::write_private(&p, encode(value).as_bytes())?;
        Ok(())
    }

    fn delete(&self, service: &str, account: &str) -> Result<bool> {
        let p = self.path(service, account)?;
        cua_home::guard_write(&p)?;
        match std::fs::remove_file(&p) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(Error::Secret(format!("{}: {e}", p.display()))),
        }
    }

    fn describe(&self, service: &str, account: &str) -> String {
        self.path(service, account)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| self.dir.display().to_string())
    }
}

/// The macOS login keychain (generic passwords). Refused in a test
/// process: tests never touch the user's keychain.
#[cfg(target_os = "macos")]
pub struct KeychainStore;

#[cfg(target_os = "macos")]
impl KeychainStore {
    fn guard() -> Result<()> {
        if cua_home::is_test_process() {
            return Err(Error::Secret(
                "refusing to use the login keychain from a test; use a FileStore".into(),
            ));
        }
        Ok(())
    }

    /// Runs a keychain call with a deadline: a locked keychain without a
    /// screen to ask on (an SSH session) blocks instead of failing.
    fn with_deadline<T: Send + 'static>(
        f: impl FnOnce() -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(std::time::Duration::from_secs(60))
            .unwrap_or_else(|_| {
                Err(Error::Secret(
                    "the login keychain did not answer within 60 s (it is locked, or waits for \
                     a password on the Mac's screen). Unlock it, or keep the credential in a \
                     file with `cua ci connect --own-app ... --secret-store file`"
                        .into(),
                ))
            })
    }
}

/// `errSecItemNotFound`.
#[cfg(target_os = "macos")]
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

#[cfg(target_os = "macos")]
impl SecretStore for KeychainStore {
    fn get(&self, service: &str, account: &str) -> Result<Option<Secret>> {
        Self::guard()?;
        let (service, account) = (service.to_string(), account.to_string());
        Self::with_deadline(move || {
            match security_framework::passwords::get_generic_password(&service, &account) {
                Ok(b) => decode(&b).map(Some),
                Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
                Err(e) => Err(Error::Secret(format!(
                    "the login keychain ({service}/{account}): {e}"
                ))),
            }
        })
    }

    fn set(&self, service: &str, account: &str, value: &Secret) -> Result<()> {
        Self::guard()?;
        let (service, account, value) = (service.to_string(), account.to_string(), encode(value));
        Self::with_deadline(move || {
            security_framework::passwords::set_generic_password(
                &service,
                &account,
                value.as_bytes(),
            )
            .map_err(|e| Error::Secret(format!("the login keychain ({service}/{account}): {e}")))
        })
    }

    fn delete(&self, service: &str, account: &str) -> Result<bool> {
        Self::guard()?;
        let (service, account) = (service.to_string(), account.to_string());
        Self::with_deadline(move || {
            match security_framework::passwords::delete_generic_password(&service, &account) {
                Ok(()) => Ok(true),
                Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(false),
                Err(e) => Err(Error::Secret(format!(
                    "the login keychain ({service}/{account}): {e}"
                ))),
            }
        })
    }

    fn describe(&self, service: &str, account: &str) -> String {
        format!("the login keychain (service {service}, account {account})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_never_print() {
        let s = Secret::new("ghs_supersecret");
        assert_eq!(format!("{s:?} {s}"), "[redacted] [redacted]");
        assert_eq!(s.expose(), "ghs_supersecret");
    }

    #[cfg(unix)]
    #[test]
    fn a_key_file_must_be_yours_alone() {
        use std::os::unix::fs::PermissionsExt as _;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("github-app.pem");
        std::fs::write(
            &p,
            "-----BEGIN TEST KEY-----\nabc\n-----END TEST KEY-----\n",
        )
        .unwrap();
        for mode in [0o644, 0o640, 0o604, 0o660] {
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
            let e = read_private_file(&p).unwrap_err().to_string();
            assert!(
                e.contains("readable or writable by others"),
                "{mode:o}: {e}"
            );
            assert!(e.contains("chmod 600"), "{e}");
            assert!(!e.contains("abc"), "the key never shows: {e}");
        }
        for mode in [0o600, 0o400] {
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
            let k = read_private_file(&p).unwrap();
            assert!(k.expose().contains("abc"));
            assert_eq!(format!("{k:?}"), "[redacted]");
        }
        // A directory, a missing file and an empty one are refused too.
        assert!(read_private_file(d.path()).is_err());
        assert!(read_private_file(&d.path().join("missing.pem")).is_err());
        let empty = d.path().join("empty.pem");
        std::fs::write(&empty, "").unwrap();
        std::fs::set_permissions(&empty, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(
            read_private_file(&empty)
                .unwrap_err()
                .to_string()
                .contains("empty")
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_key_file_comes_first_then_the_store() {
        use std::os::unix::fs::PermissionsExt as _;
        let d = tempfile::tempdir().unwrap();
        let store = FileStore::new(d.path().join("secrets"));
        store
            .set(APP_KEY_SERVICE, "trycua", &Secret::new("from-the-store"))
            .unwrap();
        let p = d.path().join("github-app.pem");
        std::fs::write(&p, "from-the-file").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();

        let (k, src) = resolve_app_key(Some(&p), &store, APP_KEY_SERVICE, "trycua")
            .unwrap()
            .unwrap();
        assert_eq!(k.expose(), "from-the-file");
        assert_eq!(src, KeySource::File(p.clone()));

        let (k, src) = resolve_app_key(None, &store, APP_KEY_SERVICE, "trycua")
            .unwrap()
            .unwrap();
        assert_eq!(k.expose(), "from-the-store");
        assert!(matches!(src, KeySource::Store(_)));

        // A key file that is set but unusable is an error, not the store.
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(resolve_app_key(Some(&p), &store, APP_KEY_SERVICE, "trycua").is_err());
        assert_eq!(
            resolve_app_key(None, &store, APP_KEY_SERVICE, "other").unwrap(),
            None
        );
    }

    #[test]
    fn file_store_round_trip_is_private_and_base64() {
        let d = tempfile::tempdir().unwrap();
        let store = FileStore::new(d.path().join("secrets"));
        let pem = Secret::new("-----BEGIN TEST KEY-----\nabc\n-----END TEST KEY-----\n");
        assert_eq!(store.get(APP_KEY_SERVICE, "trycua").unwrap(), None);
        store.set(APP_KEY_SERVICE, "trycua", &pem).unwrap();
        assert_eq!(
            store.get(APP_KEY_SERVICE, "trycua").unwrap(),
            Some(pem.clone())
        );
        let path = d.path().join("secrets/cua-ci-github-app.trycua");
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("BEGIN"), "stored base64-encoded");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(d.path().join("secrets"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        assert!(store.delete(APP_KEY_SERVICE, "trycua").unwrap());
        assert!(!store.delete(APP_KEY_SERVICE, "trycua").unwrap());
        assert!(store.set("../x", "a", &pem).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_keychain_is_never_touched_from_a_test() {
        let e = KeychainStore.get(APP_KEY_SERVICE, "trycua").unwrap_err();
        assert!(e.to_string().contains("refusing"), "{e}");
    }
}
