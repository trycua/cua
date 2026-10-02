// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! macOS Keychain *installs* for session teleport (receiver side).
//!
//! Chromium/Electron apps (Chrome, Slack, Discord, …) encrypt their local
//! cookies and auth tokens with an app-specific **"Safe Storage"** key held in
//! the Keychain, and some keep their auth token there too. The sender reads
//! those items and packs them into the reserved [`KEYCHAIN_ENTRY`];
//! [`install_all`] reinstalls them in the destination's Keychain via
//! `security add-generic-password`, through the injected [`HostEffects`].
//! Non-macOS destinations install nothing (the functions are no-ops that
//! report it), so the crate still builds and runs everywhere.
//!
//! # Nothing here may ever raise a Keychain prompt
//!
//! A Space is unattended: a SecurityAgent dialog ("security wants to use your
//! confidential information stored in 'Chrome Safe Storage'", "enter the
//! 'login' keychain password") has nobody to answer it and blocks whatever
//! asked, here the teleport, forever. The two ways this module used to raise
//! one, both from touching the **login** keychain, are closed by design:
//!
//! * The login keychain of a Lume image does not take the account password
//!   (lume's offline `/etc/kcpassword` padding bug makes loginwindow re-key it
//!   to a passphrase nobody knows), and an item an app created there (Chrome's
//!   own `Chrome Safe Storage`) trusts only that app. Reading it with
//!   `/usr/bin/security` prompts; so does writing its partition list.
//! * So spacesd never reads, writes, deletes or re-authorizes anything in the
//!   login keychain. Every item goes into a keychain spacesd **owns**
//!   (`cua.keychain-db`), created with a password spacesd knows and kept
//!   unlocked, which is put FIRST on the user's search list. Chromium looks its
//!   key up through the search list (`SecKeychainFindGenericPassword` with no
//!   keychain), which returns the first match, so the app finds ours before any
//!   item the login keychain holds, with no prompt: our item carries the app's
//!   code-signing team in its partition list and is readable by any app. A key
//!   the app made itself in the login keychain is shadowed, not read or
//!   deleted, so nothing of the user's is lost.
//! * Every `security` call is bounded ([`SECURITY_TIMEOUT`]); if one still
//!   hangs on a dialog it is killed, the dialog is dismissed and the call fails
//!   with an error that says so, instead of hanging the teleport.
//! * The spacesd process itself turns Keychain user interaction off
//!   ([`forbid_keychain_prompts`]) so an in-process Security API call fails
//!   with `errSecInteractionNotAllowed` rather than showing UI.

pub use cua_teleport_bundle::keychain::{KeychainItem, KEYCHAIN_ENTRY};

use crate::host::HostEffects;
#[cfg(target_os = "macos")]
use crate::host::{EffectKind, HostCommand, HostOutput};
use crate::ledger::ImportRecord;

/// Upper bound on any one `security` invocation. Every call here is local and
/// finishes in well under a second; one that takes this long is stuck behind a
/// Keychain dialog nobody can answer.
#[cfg(target_os = "macos")]
pub const SECURITY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

#[cfg(target_os = "macos")]
#[link(name = "Security", kind = "framework")]
extern "C" {
    fn SecKeychainSetUserInteractionAllowed(state: u8) -> i32;
}

/// Turns Keychain user interaction off for THIS process: a Security API call
/// that would have shown a SecurityAgent dialog fails with
/// `errSecInteractionNotAllowed` instead. A Space is unattended, nothing could
/// answer the dialog. (Child `security` processes are covered separately, by
/// [`SECURITY_TIMEOUT`] and by never touching a keychain whose password this
/// guest does not know.) A no-op off macOS.
pub fn forbid_keychain_prompts() {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: a plain C call taking a Boolean; no pointers.
        let _ = unsafe { SecKeychainSetUserInteractionAllowed(0) };
    }
}

/// Runs one `security` invocation with a deadline. A call that hits it was
/// stuck behind a Keychain dialog: kill the dialog too (it would otherwise sit
/// on the screen forever; SecurityAgent ignores SIGTERM, so SIGKILL, and it is
/// started again on demand) and say so.
#[cfg(target_os = "macos")]
fn security(
    host: &dyn HostEffects,
    kind: EffectKind,
    args: Vec<String>,
    stdin: Option<Vec<u8>>,
) -> std::io::Result<HostOutput> {
    let sub = args
        .iter()
        .find(|a| !a.starts_with('-'))
        .cloned()
        .unwrap_or_default();
    let mut command = HostCommand::new(kind, "security")
        .args(args)
        .timeout(SECURITY_TIMEOUT);
    if let Some(bytes) = stdin {
        command = command.stdin(bytes);
    }
    match host.run(&command) {
        Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
            let _ = host.run(
                &HostCommand::new(EffectKind::ProcessTerminate, "pkill").args([
                    "-KILL",
                    "-x",
                    "SecurityAgent",
                ]),
            );
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "`security {sub}` did not finish within {}s: it was waiting on a Keychain \
                     dialog that nothing in an unattended Space can answer, so it was stopped",
                    SECURITY_TIMEOUT.as_secs()
                ),
            ))
        }
        other => other,
    }
}

/// `security` with plain `&str` arguments, reporting only whether it succeeded.
#[cfg(target_os = "macos")]
fn security_ok(host: &dyn HostEffects, kind: EffectKind, args: &[&str]) -> bool {
    security(
        host,
        kind,
        args.iter().map(|a| a.to_string()).collect(),
        None,
    )
    .map(|o| o.success)
    .unwrap_or(false)
}

/// Reads a generic-password item's secret from spacesd's OWN keychain
/// (`cua.keychain-db`) -- never the login keychain or the search list.
/// That keychain's password is known, so the read never prompts; the login
/// keychain's is not, and an item an app created there trusts only that app, so
/// reading it with `security` raises a dialog nobody can answer. `None` when no
/// such item exists there (spacesd has not installed one yet), when the keychain
/// cannot be opened, or off macOS. Used to reuse the key a previous teleport
/// installed, so a re-teleport re-encrypts under the same key instead of minting
/// a second, conflicting one.
pub fn read_generic(
    host: &dyn HostEffects,
    service: &str,
    account: Option<&str>,
) -> Option<Vec<u8>> {
    #[cfg(target_os = "macos")]
    {
        let keychain = existing_cua_keychain(host)?;
        let mut args = vec![
            "find-generic-password".to_string(),
            "-s".to_string(),
            service.to_string(),
        ];
        if let Some(a) = account {
            args.push("-a".to_string());
            args.push(a.to_string());
        }
        args.push("-w".to_string());
        args.push(keychain);
        let output = security(host, EffectKind::KeychainRead, args, None).ok()?;
        if !output.success {
            return None;
        }
        let mut secret = output.stdout;
        if secret.last() == Some(&b'\n') {
            secret.pop();
        }
        if secret.is_empty() {
            None
        } else {
            Some(secret)
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (host, service, account);
        None
    }
}

/// Remove a generic-password item (by service and account) that spacesd
/// installed, from spacesd's own keychain. Never touches the login keychain or
/// any other: an item an app made there (Chrome's own Safe Storage key) is not
/// ours to delete. Returns whether one was removed. Off macOS nothing was ever
/// installed, so this is `Ok(false)`.
pub fn remove_generic(
    host: &dyn HostEffects,
    service: &str,
    account: &str,
) -> std::io::Result<bool> {
    #[cfg(target_os = "macos")]
    {
        let Some(keychain) = existing_cua_keychain(host) else {
            return Ok(false);
        };
        let output = security(
            host,
            EffectKind::KeychainWrite,
            vec![
                "delete-generic-password".into(),
                "-s".into(),
                service.into(),
                "-a".into(),
                account.into(),
                keychain,
            ],
            None,
        )?;
        Ok(output.success)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (host, service, account);
        Ok(false)
    }
}

/// Reads a generic-password item's secret from the DESTINATION's own
/// Keychain (this machine's, never the source's -- nothing in this crate ever
/// receives a source secret to read). `None` when spacesd has not installed one
/// yet. Used by [`crate::cookies::ensure_safe_storage_secret`] to tell "a
/// previous teleport already gave this app its key, reuse it" from "none exists,
/// create one" -- the destination's re-encryption always uses a key that either
/// already lived here or was generated here, never one carried on the wire.
pub fn read_secret(host: &dyn HostEffects, service: &str, account: &str) -> Option<Vec<u8>> {
    read_generic(host, service, Some(account))
}

/// The `security -i` line that adds `item` to `keychain`: every argument
/// double-quoted with `\` and `"` escaped, the secret hex-encoded (`-X`).
/// A newline or NUL in a name cannot be expressed on one line and is refused.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn add_command_line(item: &KeychainItem, keychain: &str) -> std::io::Result<String> {
    fn quote(value: &str) -> std::io::Result<String> {
        if value.contains(['\n', '\r', '\0']) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "keychain item names may not contain line breaks",
            ));
        }
        Ok(format!(
            "\"{}\"",
            value.replace('\\', "\\\\").replace('"', "\\\"")
        ))
    }
    Ok(format!(
        "add-generic-password -U -s {} -a {} -X {} -A {}\n",
        quote(&item.service)?,
        quote(&item.account)?,
        hex::encode(&item.secret),
        quote(keychain)?,
    ))
}

/// Install one generic-password item into spacesd's keychain. Non-macOS is a
/// no-op reporting success (there is nothing to install). Returns `Ok` on
/// success.
pub fn install_generic(host: &dyn HostEffects, item: &KeychainItem) -> std::io::Result<()> {
    install_generic_noted(host, item, &mut Vec::new())
}

/// [`install_generic`], also telling the caller (in `notes`, user-presentable)
/// about anything the user should know. Fails, fast and with a clear error,
/// rather than leave an item the app would prompt for: a teleport that cannot
/// install silently is a failed teleport, not a hung one.
///
/// The item goes into the keychain spacesd owns (`ensure_cua_keychain`):
/// created with a password spacesd knows, unlocked, never auto-locked, and first
/// on the search list so the app's own lookup finds it before anything the login
/// keychain holds (see the module docs for why the login keychain is never
/// used). The app is pre-authorized with a partition-list write using that known
/// password, which succeeds, silently, every time.
pub fn install_generic_noted(
    host: &dyn HostEffects,
    item: &KeychainItem,
    notes: &mut Vec<String>,
) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let _ = &notes;
        let keychain = ensure_cua_keychain(host)?;
        if !install_into(host, item, &keychain)? {
            let app = item
                .service
                .strip_suffix(" Safe Storage")
                .unwrap_or(&item.service);
            return Err(std::io::Error::other(format!(
                "could not pre-authorize {app} to read its saved key from {keychain}; {app} \
                 would raise a Keychain prompt nobody in this Space can answer, so the \
                 teleport was stopped instead"
            )));
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (host, item, notes);
        Ok(())
    }
}

/// Adds `item` to spacesd's `keychain` and authorizes its app. `Ok(true)` when
/// the app is pre-authorized (the partition list took); `Ok(false)` when the
/// item is installed but the app might still prompt.
#[cfg(target_os = "macos")]
fn install_into(
    host: &dyn HostEffects,
    item: &KeychainItem,
    keychain: &str,
) -> std::io::Result<bool> {
    // Delete this service+account from OUR keychain first, rather than
    // replacing it with `-U`: updating an item the caller does not own needs
    // authorization (an unanswerable dialog in a Space). Creating a fresh item
    // never prompts. Only our own keychain is touched: an item the app made
    // itself in the login keychain trusts only that app, is not ours to delete,
    // and does not need to be -- our keychain is first on the search list, so
    // the app's lookup finds ours and never reaches it.
    let _ = security(
        host,
        EffectKind::KeychainWrite,
        vec![
            "delete-generic-password".into(),
            "-s".into(),
            item.service.clone(),
            "-a".into(),
            item.account.clone(),
            keychain.to_string(),
        ],
        None,
    )?;

    // The secret never goes on argv: argv is readable by every local user
    // (`ps`). `security -i` reads commands from stdin instead, and the value
    // travels hex-encoded (`-X`, see security(1) add-generic-password).
    //
    // `-A`: every application may read this item without an "allow access?"
    // prompt. `-T <app>` looks tighter but is matched against the *reading*
    // process, which for an Electron app is not reliably the bundle we can
    // name. The exposure `-A` adds is bounded: the destination of a teleport is
    // a single-user throwaway VM whose whole purpose is to act as the user, the
    // keychain is spacesd's own, and anything that could read the item could
    // equally read the profile it decrypts.
    let line = add_command_line(item, keychain)?;
    let output = security(
        host,
        EffectKind::KeychainWrite,
        vec!["-q".into(), "-i".into()],
        Some(line.into_bytes()),
    )?;
    // Interactive mode's exit status does not reliably reflect the command's,
    // so confirm the item exists (attributes only: neither `-g` nor `-w`, so no
    // secret is printed).
    let present = output.success
        && security(
            host,
            EffectKind::KeychainRead,
            vec![
                "find-generic-password".into(),
                "-s".into(),
                item.service.clone(),
                "-a".into(),
                item.account.clone(),
                keychain.to_string(),
            ],
            None,
        )
        .map(|o| o.success)
        .unwrap_or(false);
    if !present {
        return Err(std::io::Error::other(format!(
            "security add-generic-password failed for {:?}",
            item.service
        )));
    }
    // macOS also gates reads on the item's PARTITION LIST, independently of the
    // ACL. An item created by `security` gets `apple-tool:`, which excludes
    // every app that is not an Apple command-line tool: without the app's
    // code-signing team in it the app gets "<App> wants to access key ... enter
    // the keychain password". ALWAYS set it, with the team when known.
    let team = item
        .trust_app
        .as_deref()
        .and_then(|app| team_identifier(host, app))
        .or_else(|| known_team_id(&item.service).map(str::to_string));
    let authorized = set_partition_list(
        host,
        &item.service,
        &item.account,
        team.as_deref(),
        std::path::Path::new(keychain),
    );
    // `set-generic-password-partition-list -k` unlocks the keychain for the
    // call and RE-LOCKS it on the way out. A locked keychain on the search list
    // makes any process that reaches for it (Spotlight's mdworker is one)
    // raise "<Process> wants to use the '<keychain>' keychain".
    keep_unlocked(host, keychain);
    Ok(authorized)
}

/// Parse a [`KEYCHAIN_ENTRY`] payload and install every item, returning how
/// many were installed. Best-effort: a single item's failure is skipped so the
/// rest still land (a partly-usable session beats none).
pub fn install_all(host: &dyn HostEffects, bytes: &[u8]) -> usize {
    install_all_recorded(host, bytes, &mut ImportRecord::default())
}

/// [`install_all`], recording every item actually installed in `record` (so
/// a wipe can remove it). Off macOS nothing is installed or recorded.
pub fn install_all_recorded(
    host: &dyn HostEffects,
    bytes: &[u8],
    record: &mut ImportRecord,
) -> usize {
    let items = cua_teleport_bundle::keychain::parse(bytes);
    items
        .iter()
        .filter(|item| {
            let ok = match install_generic_noted(host, item, &mut record.notices) {
                Ok(()) => true,
                Err(e) => {
                    eprintln!(
                        "teleport: keychain item {:?} not installed: {e}",
                        item.service
                    );
                    record.notices.push(format!("{}: {e}", item.service));
                    false
                }
            };
            if ok && cfg!(target_os = "macos") {
                record.keychain_installed(&item.service, &item.account);
            }
            ok
        })
        .count()
}

/// The code-signing Team Identifier of an installed `.app`, e.g. "9QW8UQUTAA"
/// for Unity. `None` for an unsigned or missing bundle.
#[cfg(target_os = "macos")]
fn team_identifier(host: &dyn HostEffects, app_path: &str) -> Option<String> {
    let output = host
        .run(
            &HostCommand::new(EffectKind::CodeSignInspect, "codesign").args([
                "-dv",
                "--verbose=4",
                app_path,
            ]),
        )
        .ok()?;
    // codesign writes its report to stderr.
    let text = String::from_utf8_lossy(&output.stderr);
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix("TeamIdentifier=") {
            let id = rest.trim();
            if !id.is_empty() && id != "not set" {
                return Some(id.to_string());
            }
        }
    }
    None
}

/// Authorize a keychain item for silent reads by code-signed apps, so the
/// trusted app never raises the "wants to access key … enter the login keychain
/// password" dialog. `apple:` covers Apple-signed tooling; the team id covers
/// the destination app itself. Best-effort — a failure here only means the app
/// may prompt, so it must not fail the install.
#[cfg(target_os = "macos")]
fn set_partition_list(
    host: &dyn HostEffects,
    service: &str,
    account: &str,
    team_id: Option<&str>,
    path: &std::path::Path,
) -> bool {
    // Targets the keychain the item actually went into (`path`). Re-verified
    // 2026-09-12 inside a Space, in the aqua session as uid 501: this exact
    // call with `-k lume` against the login keychain of an image with the
    // autologin-password bug returns "SecKeychainItemSetAccessWithPassword:
    // The user name or passphrase you entered is not correct" --
    // `security unlock-keychain -p lume` succeeding does NOT contradict that,
    // unlocking an already-unlocked keychain never checks the password.
    // Every password this guest can know is therefore tried in turn
    // ([`keychain_passwords`]); the caller moves to a keychain with a known
    // password when none opens this one.
    let partitions = match team_id {
        // apple-tool: keeps /usr/bin/security itself able to touch the item
        // (the golden's per-boot verification uses it), apple: covers
        // Apple-signed callers, and the team id covers the destination app.
        Some(id) => format!("teamid:{id},apple:,apple-tool:"),
        None => "apple:,apple-tool:".to_string(),
    };
    let mut last_error = String::new();
    for pw in keychain_passwords(host) {
        let mut args: Vec<String> = vec![
            "set-generic-password-partition-list".into(),
            "-S".into(),
            partitions.clone(),
            "-k".into(),
            pw,
            "-s".into(),
            service.to_string(),
        ];
        if !account.is_empty() {
            args.push("-a".into());
            args.push(account.to_string());
        }
        args.push(path.to_string_lossy().into_owned());
        match security(host, EffectKind::KeychainWrite, args, None) {
            Ok(out) if out.success => return true,
            Ok(out) => last_error = String::from_utf8_lossy(&out.stderr).trim().to_string(),
            Err(e) => last_error = e.to_string(),
        }
    }
    // Reported, not swallowed: the failure mode is a SecurityAgent dialog
    // nothing in an unattended destination can answer.
    eprintln!(
        "warning: could not authorize keychain item {service:?} for silent reads \
         ({last_error}); the destination app will raise a \"wants to access key\" dialog. \
         Keychain: {}",
        path.display()
    );
    false
}

/// Passwords this guest might have for a keychain, most specific first:
/// `CUA_ENV_LOGIN_KEYCHAIN_PW`; the first line of `~/.cua/spacesd/keychain-password`
/// or `/etc/cua/keychain-password` (a provisioner writes one, 0600/0644 root
/// or the desktop user); then the Lume image's account password `lume`.
#[cfg(target_os = "macos")]
fn keychain_passwords(host: &dyn HostEffects) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |p: String| {
        let p = p.trim_end_matches(['\r', '\n']).to_string();
        if !p.is_empty() && !out.contains(&p) {
            out.push(p);
        }
    };
    if let Ok(p) = std::env::var("CUA_ENV_LOGIN_KEYCHAIN_PW") {
        push(p);
    }
    let mut files = vec![std::path::PathBuf::from("/etc/cua/keychain-password")];
    if let Some(home) = host.home_dir() {
        files.insert(0, home.join(".cua/spacesd/keychain-password"));
    }
    for f in files {
        if let Ok(text) = std::fs::read_to_string(f) {
            if let Some(line) = text.lines().next() {
                push(line.to_string());
            }
        }
    }
    push("lume".to_string());
    out
}

/// The code-signing Team Identifier of the Chromium-family browsers whose
/// Safe Storage key this crate installs, used when the app is not installed
/// yet to be inspected with `codesign`: the partition list must name the
/// browser's team or the browser prompts for the key.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn known_team_id(service: &str) -> Option<&'static str> {
    match service {
        "Chrome Safe Storage" => Some("EQHXZ8M8AV"),
        "Microsoft Edge Safe Storage" => Some("UBF8T346G9"),
        "Brave Safe Storage" => Some("KL8N8XSYF4"),
        _ => None,
    }
}

/// The macOS app bundle that owns Safe Storage item `service`, for
/// [`KeychainItem::trust_app`].
pub(crate) fn trusted_app_for(service: &str) -> Option<&'static str> {
    match service {
        "Chrome Safe Storage" => Some("/Applications/Google Chrome.app"),
        "Chromium Safe Storage" => Some("/Applications/Chromium.app"),
        "Microsoft Edge Safe Storage" => Some("/Applications/Microsoft Edge.app"),
        "Brave Safe Storage" => Some("/Applications/Brave Browser.app"),
        _ => None,
    }
}

/// Where spacesd's own keychain lives (`CUA_ENV_KEYCHAIN` overrides it for a
/// provisioner that brings its own, already unlocked).
#[cfg(target_os = "macos")]
fn cua_keychain_path(host: &dyn HostEffects) -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("CUA_ENV_KEYCHAIN") {
        let p = std::path::PathBuf::from(p);
        if !p.as_os_str().is_empty() {
            return Some(p);
        }
    }
    Some(host.home_dir()?.join("Library/Keychains/cua.keychain-db"))
}

/// Unlocks `path` with the first of `passwords` it takes and gives it no
/// auto-lock: `set-keychain-settings` without `-l` (lock on sleep) or `-u`
/// (lock after a timeout) leaves it open until logout. Whether it opened.
/// `unlock-keychain -p` is non-interactive by construction: with a password
/// given it either opens the keychain or fails, it never shows a dialog.
#[cfg(target_os = "macos")]
fn unlock_and_unlimit(host: &dyn HostEffects, path: &str, passwords: &[String]) -> bool {
    let mut opened = false;
    for pw in passwords {
        if security_ok(
            host,
            EffectKind::KeychainWrite,
            &["unlock-keychain", "-p", pw, path],
        ) {
            opened = true;
            break;
        }
    }
    if opened {
        security_ok(
            host,
            EffectKind::KeychainWrite,
            &["set-keychain-settings", path],
        );
    }
    opened
}

/// Re-opens `keychain` (a keychain spacesd owns) with the passwords this guest
/// knows. Cheap and silent; called after anything that re-locks it.
#[cfg(target_os = "macos")]
fn keep_unlocked(host: &dyn HostEffects, keychain: &str) {
    unlock_and_unlimit(host, keychain, &keychain_passwords(host));
}

/// spacesd's keychain when it already exists on disk and opens with a password
/// this guest knows; `None` otherwise (nothing to read from, and nothing here
/// may create one: that is [`ensure_cua_keychain`]'s job).
#[cfg(target_os = "macos")]
fn existing_cua_keychain(host: &dyn HostEffects) -> Option<String> {
    let path = cua_keychain_path(host)?;
    if !path.exists() {
        return None;
    }
    let path = path.to_string_lossy().into_owned();
    unlock_and_unlimit(host, &path, &keychain_passwords(host)).then_some(path)
}

/// Makes sure spacesd's own keychain exists, is unlocked and never auto-locks,
/// and is FIRST on the user's keychain search list, and returns its path.
///
/// Created with the first of [`keychain_passwords`], so it always takes a
/// password this guest knows (unlike the login keychain of an image with the
/// autologin-password bug). If one exists that takes none of them (corrupt, or
/// re-keyed by something else) it is set aside as `cua.keychain-db.unusable`
/// (never deleted) and replaced: it only ever held items spacesd itself can
/// install again.
///
/// First on the list, because Chromium and Electron apps look their Safe
/// Storage key up through the search list and take the first match. That is
/// what makes an item the app created in the login keychain irrelevant: ours
/// wins, silently.
#[cfg(target_os = "macos")]
fn ensure_cua_keychain(host: &dyn HostEffects) -> std::io::Result<String> {
    let path = cua_keychain_path(host)
        .ok_or_else(|| std::io::Error::other("this Space has no home directory"))?;
    let path_str = path.to_string_lossy().into_owned();
    let passwords = keychain_passwords(host);
    let usable = path.exists() && unlock_and_unlimit(host, &path_str, &passwords);
    if !usable {
        if path.exists() {
            eprintln!(
                "teleport: {path_str} takes no password this Space knows; setting it aside and \
                 creating a new one"
            );
            set_aside(&path);
        }
        // `?`: a call stuck behind a dialog says so (see `security`), rather than
        // being reported as a keychain that merely could not be created.
        let created = security(
            host,
            EffectKind::KeychainWrite,
            vec![
                "create-keychain".into(),
                "-p".into(),
                passwords.first().cloned().unwrap_or_else(|| "lume".into()),
                path_str.clone(),
            ],
            None,
        )?
        .success;
        if !created || !unlock_and_unlimit(host, &path_str, &passwords) {
            return Err(std::io::Error::other(format!(
                "could not create or open the keychain {path_str} that Cua uses to hand \
                 saved browser keys to apps"
            )));
        }
    }
    put_first_on_search_list(host, &path_str);
    Ok(path_str)
}

/// Renames `path` and its SQLite sidecars to `<name>.unusable` (replacing an
/// earlier one), so a new keychain can take the name.
#[cfg(target_os = "macos")]
fn set_aside(path: &std::path::Path) {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name().and_then(|n| n.to_str())) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let file = entry.file_name().to_string_lossy().into_owned();
        if file.starts_with(name) && !file.ends_with(".unusable") {
            let _ = std::fs::rename(entry.path(), dir.join(format!("{file}.unusable")));
        }
    }
}

/// Makes spacesd's keychain the first entry of the user's keychain search list,
/// keeping everything else after it, in order. A no-op when it already is.
#[cfg(target_os = "macos")]
fn put_first_on_search_list(host: &dyn HostEffects, path: &str) {
    let mut list: Vec<String> = Vec::new();
    if let Ok(o) = security(
        host,
        EffectKind::KeychainRead,
        vec!["list-keychains".into(), "-d".into(), "user".into()],
        None,
    ) {
        for line in String::from_utf8_lossy(&o.stdout).lines() {
            let p = line.trim().trim_matches('"').to_string();
            if !p.is_empty() {
                list.push(p);
            }
        }
    }
    if list.first().map(String::as_str) == Some(path) {
        return;
    }
    list.retain(|p| p != path);
    list.insert(0, path.to_string());
    let mut args = vec![
        "list-keychains".to_string(),
        "-d".into(),
        "user".into(),
        "-s".into(),
    ];
    args.extend(list);
    let _ = security(host, EffectKind::KeychainWrite, args, None);
}

/// Keeps spacesd's keychain open and first on the search list, if it exists:
/// run at startup and periodically by the server, so a reboot, a wake, a
/// timeout or something resetting the search list never leaves a LOCKED
/// keychain for Spotlight and friends to ask about, or lets the login keychain's
/// own copy of an app's key win the lookup. Never creates the keychain (a Space
/// that was never teleported into has none). Whether it is open.
pub fn keep_cua_keychain_unlocked(host: &dyn HostEffects) -> bool {
    #[cfg(target_os = "macos")]
    {
        let Some(path) = existing_cua_keychain(host) else {
            return false;
        };
        put_first_on_search_list(host, &path);
        true
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = host;
        false
    }
}

/// How often the server re-runs [`keep_cua_keychain_unlocked`].
pub const KEEPER_INTERVAL: std::time::Duration = std::time::Duration::from_secs(20);

#[cfg(test)]
pub(crate) fn is_add_command(call: &crate::host::HostCommand) -> bool {
    call.stdin
        .as_ref()
        .is_some_and(|s| s.as_bytes().starts_with(b"add-generic-password "))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    use crate::host::HostOutput;
    use crate::host::{EffectKind, FakeHost};

    #[test]
    fn install_all_on_empty_or_garbage_is_zero() {
        let host = FakeHost::new();
        assert_eq!(install_all(&host, b"[]"), 0);
        assert_eq!(install_all(&host, b"not json"), 0);
        assert!(host
            .calls_of(EffectKind::KeychainWrite)
            .iter()
            // Only the best-effort unlock of the (fake) keychain, if anything.
            .all(|c| !is_add_command(c)));
    }

    /// Off macOS an install is a reported no-op that touches nothing.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn install_is_a_no_op_off_macos() {
        let host = FakeHost::new();
        let item = KeychainItem {
            service: "cua-spacesd-client fixture service".into(),
            account: "fixture".into(),
            secret: b"not-a-secret".to_vec(),
            trust_app: None,
        };
        let bytes = cua_teleport_bundle::keychain::serialize(&[item]);
        assert_eq!(install_all(&host, &bytes), 1);
        assert!(host.calls().is_empty(), "{:?}", host.calls());
    }

    /// Keychain installs go through the injected host: with a fake, every
    /// write is recorded and none reaches the real `security` tool.
    #[cfg(target_os = "macos")]
    #[test]
    fn install_goes_through_the_injected_host() {
        let host = FakeHost::new()
            .with_home("/nonexistent-fake-home")
            .with_responder(|_| Ok(HostOutput::ok("")));
        let item = KeychainItem {
            service: "cua-spacesd-client fixture service".into(),
            account: "fixture".into(),
            secret: b"not-a-secret".to_vec(),
            trust_app: None,
        };
        let installed = install_all(&host, &cua_teleport_bundle::keychain::serialize(&[item]));
        assert_eq!(installed, 1);
        let writes = host.calls_of(EffectKind::KeychainWrite);
        assert!(writes.iter().any(is_add_command), "{writes:?}");
        assert!(writes.iter().all(|call| call.program == "security"));
    }

    /// The secret never appears on any argv (visible in `ps`); it reaches
    /// `security -i` hex-encoded on stdin, and a recorded or logged command
    /// never prints it.
    #[cfg(target_os = "macos")]
    #[test]
    fn install_never_puts_the_secret_on_argv() {
        let host = FakeHost::new()
            .with_home("/nonexistent-fake-home")
            .with_responder(|_| Ok(HostOutput::ok("")));
        let secret = b"fixture-secret \"quoted\" value";
        let item = KeychainItem {
            service: "Fixture \"Safe\" Storage".into(),
            account: "fixture".into(),
            secret: secret.to_vec(),
            trust_app: None,
        };
        let mut record = ImportRecord::default();
        let bytes = cua_teleport_bundle::keychain::serialize(&[item]);
        assert_eq!(install_all_recorded(&host, &bytes, &mut record), 1);
        let hex = hex::encode(secret);
        let plain = String::from_utf8_lossy(secret).into_owned();
        for call in host.calls() {
            for arg in &call.args {
                assert!(
                    !arg.contains(&plain) && !arg.contains(&hex),
                    "{:?}",
                    call.args
                );
            }
            let debug = format!("{call:?}");
            assert!(!debug.contains(&plain) && !debug.contains(&hex), "{debug}");
        }
        let fed: Vec<_> = host
            .calls()
            .into_iter()
            .filter_map(|c| {
                c.stdin
                    .map(|s| String::from_utf8(s.as_bytes().to_vec()).unwrap())
            })
            .collect();
        assert_eq!(fed.len(), 1, "{fed:?}");
        assert!(fed[0].starts_with("add-generic-password -U -s \"Fixture \\\"Safe\\\" Storage\""));
        assert!(fed[0].contains(&format!(" -X {hex} ")), "{}", fed[0]);
        assert!(fed[0].ends_with('\n') && fed[0].matches('\n').count() == 1);
        assert_eq!(
            record.keychain_items,
            vec![crate::ledger::KeychainRef {
                service: "Fixture \"Safe\" Storage".into(),
                account: "fixture".into()
            }]
        );
    }

    /// A failed add (the item is not found afterwards) is not recorded.
    #[cfg(target_os = "macos")]
    #[test]
    fn failed_install_is_not_recorded() {
        let host = FakeHost::new()
            .with_home("/nonexistent-fake-home")
            .with_responder(|c| {
                Ok(
                    if c.args.first().map(String::as_str) == Some("find-generic-password") {
                        HostOutput::failed()
                    } else {
                        HostOutput::ok("")
                    },
                )
            });
        let item = KeychainItem {
            service: "s".into(),
            account: "a".into(),
            secret: b"x".to_vec(),
            trust_app: None,
        };
        let mut record = ImportRecord::default();
        let bytes = cua_teleport_bundle::keychain::serialize(&[item]);
        assert_eq!(install_all_recorded(&host, &bytes, &mut record), 0);
        assert!(record.keychain_items.is_empty());
    }

    #[cfg(target_os = "macos")]
    fn chrome_item() -> KeychainItem {
        KeychainItem {
            service: "Chrome Safe Storage".into(),
            account: "Chrome".into(),
            secret: b"s3cret".to_vec(),
            trust_app: trusted_app_for("Chrome Safe Storage").map(str::to_string),
        }
    }

    #[cfg(target_os = "macos")]
    fn partition_calls(host: &FakeHost) -> Vec<crate::host::HostCommand> {
        host.calls_of(EffectKind::KeychainWrite)
            .into_iter()
            .filter(|c| {
                c.args.first().map(String::as_str) == Some("set-generic-password-partition-list")
            })
            .collect()
    }

    /// A Safe Storage item we create trusts Chrome from the start: its team id
    /// is in the partition list even though Chrome is not installed to be
    /// inspected (so nothing prompts, and no password dialog is needed).
    #[cfg(target_os = "macos")]
    #[test]
    fn chrome_is_trusted_on_the_item_it_is_created_with() {
        let host = FakeHost::new()
            .with_home("/nonexistent-fake-home")
            .with_responder(|c| {
                Ok(if c.program == "codesign" {
                    HostOutput::failed()
                } else {
                    HostOutput::ok("")
                })
            });
        let mut notes = Vec::new();
        install_generic_noted(&host, &chrome_item(), &mut notes).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        let calls = partition_calls(&host);
        assert_eq!(calls.len(), 1, "{calls:?}");
        let partitions = &calls[0].args[2];
        assert!(partitions.contains("teamid:EQHXZ8M8AV"), "{partitions}");
        assert!(partitions.contains("apple-tool:"), "{partitions}");
    }

    /// The login keychain's password is not one this guest knows (the image's
    /// autologin password file is wrong): the item moves to a keychain spacesd
    /// owns, where the write takes, and Chrome is still authorized silently.
    /// A Space that has never been teleported into, with the login keychain on
    /// the search list (the shape of a fresh Lume image).
    #[cfg(target_os = "macos")]
    fn fresh_space(home: &std::path::Path) -> FakeHost {
        let login = format!(
            "\"{}/Library/Keychains/login.keychain-db\"\n",
            home.display()
        );
        FakeHost::new().with_home(home).with_responder(move |c| {
            Ok(match c.args.first().map(String::as_str) {
                Some("list-keychains") if !c.args.contains(&"-s".to_string()) => {
                    HostOutput::ok(login.clone())
                }
                _ => HostOutput::ok(""),
            })
        })
    }

    /// Every keychain a `security` call names, wherever it appears (argument or
    /// `security -i` stdin line).
    #[cfg(target_os = "macos")]
    fn touched_keychains(call: &crate::host::HostCommand) -> Vec<String> {
        let mut text: Vec<String> = call.args.clone();
        if let Some(stdin) = &call.stdin {
            text.push(String::from_utf8_lossy(stdin.as_bytes()).into_owned());
        }
        text.iter()
            .flat_map(|t| t.split('"').map(str::to_string).collect::<Vec<_>>())
            .filter(|t| t.ends_with(".keychain-db") || t.ends_with(".keychain"))
            .collect()
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn chrome_s_key_goes_into_spacesds_keychain_first_on_the_search_list() {
        let home = tempfile::tempdir().unwrap();
        let host = fresh_space(home.path());
        let mut notes = Vec::new();
        install_generic_noted(&host, &chrome_item(), &mut notes).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        let cua = format!(
            "{}/Library/Keychains/cua.keychain-db",
            home.path().display()
        );
        let login = format!(
            "{}/Library/Keychains/login.keychain-db",
            home.path().display()
        );
        let writes = host.calls_of(EffectKind::KeychainWrite);
        // Created with a password spacesd knows, kept open, never auto-locked.
        assert!(writes.iter().any(
            |c| c.args == ["create-keychain", "-p", "lume", cua.as_str()]
                || c.args.first().map(String::as_str) == Some("create-keychain")
                    && c.args.last().map(String::as_str) == Some(cua.as_str())
        ));
        assert!(writes
            .iter()
            .any(|c| c.args == ["set-keychain-settings", cua.as_str()]));
        // FIRST on the list, the login keychain after it (the app's lookup takes
        // the first match, so ours wins over the key the app made itself).
        let set_list = writes
            .iter()
            .find(|c| c.args.first().map(String::as_str) == Some("list-keychains"))
            .expect("the search list was written");
        assert_eq!(set_list.args[4..], [cua.clone(), login.clone()]);
        // The item and its authorization are in OUR keychain.
        assert_eq!(writes.iter().filter(|c| is_add_command(c)).count(), 1);
        let add = writes.iter().find(|c| is_add_command(c)).unwrap();
        assert!(touched_keychains(add).iter().all(|k| *k == cua), "{add:?}");
        let last = partition_calls(&host).pop().unwrap();
        assert_eq!(last.args.last().map(String::as_str), Some(cua.as_str()));
        assert!(last.args[2].contains("teamid:EQHXZ8M8AV"));
    }

    /// The prompt this fix exists for: `security` reading, deleting or
    /// re-authorizing an item in the login keychain (Chrome's own Safe Storage
    /// item trusts only Chrome; the login keychain's password is not the
    /// account password). Whatever happens, no call may name it other than to
    /// list the search list.
    #[cfg(target_os = "macos")]
    #[test]
    fn spacesd_never_touches_the_login_keychain() {
        let home = tempfile::tempdir().unwrap();
        let login = format!(
            "{}/Library/Keychains/login.keychain-db",
            home.path().display()
        );
        for existing_cua_keychain in [false, true] {
            if existing_cua_keychain {
                let dir = home.path().join("Library/Keychains");
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join("cua.keychain-db"), b"").unwrap();
            }
            let host = fresh_space(home.path());
            let mut record = ImportRecord::default();
            let items = cua_teleport_bundle::keychain::serialize(&[chrome_item()]);
            assert_eq!(install_all_recorded(&host, &items, &mut record), 1);
            let _ = read_secret(&host, "Chrome Safe Storage", "Chrome");
            let _ = keep_cua_keychain_unlocked(&host);
            let _ = remove_generic(&host, "Chrome Safe Storage", "Chrome");
            for call in host.calls() {
                let list_only = call.args.first().map(String::as_str) == Some("list-keychains");
                let names_login = touched_keychains(&call).contains(&login);
                assert!(
                    !names_login || list_only,
                    "a call touched the login keychain: {call:?}"
                );
                if call.program == "security" {
                    assert!(
                        call.timeout.is_some(),
                        "an unbounded security call: {call:?}"
                    );
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_security_call_stuck_on_a_dialog_fails_fast_and_dismisses_it() {
        let home = tempfile::tempdir().unwrap();
        let host = FakeHost::new().with_home(home.path()).with_responder(|c| {
            if c.args.first().map(String::as_str) == Some("create-keychain") {
                Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "hung"))
            } else {
                Ok(HostOutput::ok(""))
            }
        });
        let started = std::time::Instant::now();
        let err = install_generic_noted(&host, &chrome_item(), &mut Vec::new()).unwrap_err();
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(
            err.to_string().contains("waiting on a Keychain dialog"),
            "{err}"
        );
        let kills = host.calls_of(EffectKind::ProcessTerminate);
        assert!(
            kills
                .iter()
                .any(|c| c.args == ["-KILL", "-x", "SecurityAgent"]),
            "the dialog is dismissed: {kills:?}"
        );
        assert!(host
            .calls()
            .iter()
            .filter(|c| c.program == "security")
            .all(|c| c.timeout == Some(SECURITY_TIMEOUT)));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_keychain_that_takes_no_known_password_is_set_aside_and_replaced() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("Library/Keychains");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("cua.keychain-db"), b"old").unwrap();
        let created = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = created.clone();
        let host = FakeHost::new()
            .with_home(home.path())
            .with_responder(move |c| {
                let sub = c.args.first().map(String::as_str);
                if sub == Some("create-keychain") {
                    seen.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                // Until it is recreated, no password opens it.
                Ok(
                    if sub == Some("unlock-keychain")
                        && !seen.load(std::sync::atomic::Ordering::SeqCst)
                    {
                        HostOutput::failed()
                    } else {
                        HostOutput::ok("")
                    },
                )
            });
        install_generic_noted(&host, &chrome_item(), &mut Vec::new()).unwrap();
        assert!(created.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(
            std::fs::read(dir.join("cua.keychain-db.unusable")).unwrap(),
            b"old",
            "set aside, never deleted"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_keeper_unlocks_the_cua_keychain_and_never_auto_locks_it() {
        let dir = tempfile::tempdir().unwrap();
        let kc = dir.path().join("Library/Keychains");
        std::fs::create_dir_all(&kc).unwrap();
        let path = kc.join("cua.keychain-db");
        let p = path.to_string_lossy().into_owned();
        let none = FakeHost::new().with_home(dir.path());
        assert!(!keep_cua_keychain_unlocked(&none), "no keychain, no work");
        assert!(none.calls().is_empty());

        std::fs::write(&path, b"").unwrap();
        let login = format!(
            "\"{}/Library/Keychains/login.keychain-db\"\n",
            dir.path().display()
        );
        let host = FakeHost::new()
            .with_home(dir.path())
            .with_responder(move |c| {
                Ok(match c.args.first().map(String::as_str) {
                    Some("unlock-keychain") if c.args[2] != "lume" => HostOutput::failed(),
                    // The search list lost our keychain (something reset it).
                    Some("list-keychains") if !c.args.contains(&"-s".to_string()) => {
                        HostOutput::ok(login.clone())
                    }
                    _ => HostOutput::ok(""),
                })
            });
        assert!(keep_cua_keychain_unlocked(&host));
        let calls = host.calls_of(EffectKind::KeychainWrite);
        assert!(calls
            .iter()
            .any(|c| c.args == ["set-keychain-settings".to_string(), p.clone()]));
        assert!(
            calls.iter().all(|c| !c
                .args
                .iter()
                .any(|a| matches!(a.as_str(), "-l" | "-u" | "-t"))),
            "no lock on sleep, no timeout"
        );
        assert!(calls.iter().any(|c| c.args
            == [
                "unlock-keychain".to_string(),
                "-p".into(),
                "lume".into(),
                p.clone()
            ]));
        let relist = calls
            .iter()
            .find(|c| c.args.first().map(String::as_str) == Some("list-keychains"))
            .expect("our keychain is put back at the front");
        assert_eq!(relist.args[4], p);

        // Already first: the search list is left alone.
        let host = FakeHost::new().with_home(dir.path()).with_responder({
            let p = p.clone();
            move |c| {
                Ok(match c.args.first().map(String::as_str) {
                    Some("list-keychains") if !c.args.contains(&"-s".to_string()) => {
                        HostOutput::ok(format!("\"{p}\"\n"))
                    }
                    _ => HostOutput::ok(""),
                })
            }
        });
        assert!(keep_cua_keychain_unlocked(&host));
        assert!(host.calls_of(EffectKind::KeychainWrite).iter().all(|c| c
            .args
            .first()
            .map(String::as_str)
            != Some("list-keychains")));

        let host = FakeHost::new()
            .with_home(dir.path())
            .with_responder(|_| Ok(HostOutput::failed()));
        assert!(!keep_cua_keychain_unlocked(&host));
        assert!(host
            .calls()
            .iter()
            .all(|c| c.args.first().map(String::as_str) != Some("set-keychain-settings")));
    }

    /// If the app cannot be pre-authorized (it would prompt), the teleport
    /// fails, fast and clearly, instead of leaving it to hang on a dialog.
    #[cfg(target_os = "macos")]
    #[test]
    fn an_item_that_cannot_be_authorized_stops_the_teleport() {
        let home = tempfile::tempdir().unwrap();
        let host = FakeHost::new().with_home(home.path()).with_responder(|c| {
            Ok(match c.args.first().map(String::as_str) {
                Some("set-generic-password-partition-list") => HostOutput::failed(),
                _ => HostOutput::ok(""),
            })
        });
        let err = install_generic_noted(&host, &chrome_item(), &mut Vec::new()).unwrap_err();
        assert!(
            err.to_string()
                .contains("could not pre-authorize Chrome to read its saved key"),
            "{err}"
        );
    }

    #[test]
    fn names_with_line_breaks_are_refused() {
        let item = KeychainItem {
            service: "a\nadd-generic-password".into(),
            account: "x".into(),
            secret: vec![],
            trust_app: None,
        };
        assert!(add_command_line(&item, "/k").is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn remove_goes_through_the_injected_host_and_only_in_spacesds_keychain() {
        let home = tempfile::tempdir().unwrap();
        let kc = home.path().join("Library/Keychains");
        std::fs::create_dir_all(&kc).unwrap();
        std::fs::write(kc.join("cua.keychain-db"), b"").unwrap();
        let host = FakeHost::new()
            .with_home(home.path())
            .with_responder(|_| Ok(HostOutput::ok("")));
        assert!(remove_generic(&host, "svc", "acct").unwrap());
        let deletes: Vec<_> = host
            .calls_of(EffectKind::KeychainWrite)
            .into_iter()
            .filter(|c| c.args.first().map(String::as_str) == Some("delete-generic-password"))
            .collect();
        assert_eq!(deletes.len(), 1, "{deletes:?}");
        assert_eq!(deletes[0].program, "security");
        assert_eq!(
            deletes[0].args[..5],
            ["delete-generic-password", "-s", "svc", "-a", "acct"]
        );
        assert!(deletes[0].args[5].ends_with("cua.keychain-db"));
        // No keychain of ours yet: nothing to remove, nothing run.
        let none = FakeHost::new().with_home("/nonexistent-fake-home");
        assert!(!remove_generic(&none, "svc", "acct").unwrap());
        assert!(none.calls().is_empty());
    }
}
