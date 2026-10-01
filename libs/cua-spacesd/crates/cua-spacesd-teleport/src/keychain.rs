// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! macOS Keychain *installs* for session teleport (receiver side).
//!
//! Chromium/Electron apps (Chrome, Slack, Discord, …) encrypt their local
//! cookies and auth tokens with an app-specific **"Safe Storage"** key held in
//! the login Keychain, and some keep their auth token there too. The sender
//! reads those items and packs them into the reserved [`KEYCHAIN_ENTRY`];
//! [`install_all`] reinstalls them in the destination's Keychain via
//! `security add-generic-password`, through the injected [`HostEffects`].
//! Non-macOS destinations install nothing (the functions are no-ops that
//! report it), so the crate still builds and runs everywhere.

pub use cua_teleport_bundle::keychain::{KeychainItem, KEYCHAIN_ENTRY};

use crate::host::HostEffects;
#[cfg(target_os = "macos")]
use crate::host::{EffectKind, HostCommand};
use crate::ledger::ImportRecord;

/// Reads a generic-password item's secret from the destination's own target
/// keychain ([`target_keychain_path`]) -- never the search list, so this never
/// picks up a same-named item some other keychain happens to hold. `None`
/// when no such item exists there, or off macOS (nothing is ever installed
/// there). Used to detect an already-initialized Safe Storage key (e.g. this
/// guest's Chrome already ran once) so a re-encrypting import reuses it
/// instead of minting a second, conflicting one.
pub fn read_generic(
    host: &dyn HostEffects,
    service: &str,
    account: Option<&str>,
) -> Option<Vec<u8>> {
    #[cfg(target_os = "macos")]
    {
        let keychain = target_keychain_path(host).to_string_lossy().into_owned();
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
        let output = host
            .run(&HostCommand::new(EffectKind::KeychainRead, "security").args(args))
            .ok()?;
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

/// Remove a generic-password item (by service and account) from the target
/// keychain and every keychain on the search list. Returns whether one was
/// removed. Off macOS nothing was ever installed, so this is `Ok(false)`.
pub fn remove_generic(
    host: &dyn HostEffects,
    service: &str,
    account: &str,
) -> std::io::Result<bool> {
    #[cfg(target_os = "macos")]
    {
        let mut removed = false;
        for keychain in search_list_keychains(host) {
            let output = host.run(
                &HostCommand::new(EffectKind::KeychainWrite, "security").args([
                    "delete-generic-password",
                    "-s",
                    service,
                    "-a",
                    account,
                    &keychain,
                ]),
            )?;
            removed |= output.success;
        }
        Ok(removed)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (host, service, account);
        Ok(false)
    }
}

/// Reads a generic-password item's secret from the DESTINATION's own
/// Keychain (this machine's, never the source's -- nothing in this crate ever
/// receives a source secret to read). `None` when no such item exists yet
/// (a fresh Space, or a browser that has not created its Safe Storage item
/// on this machine). Used by [`crate::cookies::ensure_safe_storage_secret`]
/// to tell "this destination already has its own key, use it" from "none
/// exists, create one" -- the destination's re-encryption always uses a key
/// that either already lived here or was generated here, never one carried
/// on the wire.
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

/// Install one generic-password item into the login Keychain, first deleting
/// any existing item with the same service+account so the write never needs
/// authorization. Non-macOS is a no-op reporting success (there is nothing to
/// install). Returns `Ok` on success.
pub fn install_generic(host: &dyn HostEffects, item: &KeychainItem) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        // Install into the DEFAULT keychain (see target_keychain_path): apps
        // read their Safe Storage key / tokens from the DEFAULT keychain, not
        // the search list, so a non-default keychain is not consulted — a
        // teleported Unity Hub whose `unity` token item sat in a search-list-only
        // keychain logged "No tokens found, logging out" and rendered the
        // sign-in screen with every teleported file correct on disk. The
        // destination must therefore keep its login keychain unlocked (the Cua
        // golden resets it at build so autologin unlocks it — see the golden
        // build). `-A` = all apps may read without a per-access prompt; `-U`
        // replaces any existing item.
        // Delete any existing item for this service+account FIRST, rather than
        // replacing it with `-U`. Updating an item the caller does not own needs
        // authorization, and in an unattended destination (a Space) that opens a
        // SecurityAgent dialog nobody can answer: `security add-generic-password
        // -U` then hangs forever, the import never returns, and the teleport
        // looks like a network failure. Creating a *fresh* item never prompts.
        // Deleting is safe here: we are about to write the authoritative value,
        // and a teleport is expected to overwrite the destination's session.
        // Delete from EVERY keychain in the search list, not just the one we
        // are about to write. A stale copy in any other keychain shadows ours:
        // reads resolve through the search list, and an app that minted its own
        // key before the teleport arrived (Unity Hub's Electron "Safe Storage"
        // key, written the moment the Hub launches) will keep winning, leaving
        // the app visibly signed out with the correct account in its database.
        // Deleting is safe: we are about to write the authoritative value, and
        // a teleport is expected to overwrite the destination's session.
        for keychain in search_list_keychains(host) {
            let _ = host.run(
                &HostCommand::new(EffectKind::KeychainWrite, "security").args([
                    "delete-generic-password",
                    "-s",
                    &item.service,
                    "-a",
                    &item.account,
                    &keychain,
                ]),
            );
        }

        // The secret never goes on argv: argv is readable by every local
        // user (`ps`, /proc/<pid>/cmdline). `security -i` reads commands from
        // stdin instead, and the value travels hex-encoded (`-X`, see
        // security(1) add-generic-password), so no quoting of the secret is
        // needed. `-w` as the last argument (prompt) is not usable here: the
        // keychain path must be the final positional argument, and the
        // prompt needs a terminal.
        //
        // `-A`: every application may read this item without an "allow
        // access?" prompt.
        //
        // `-T <app>` looks tighter and was what this did, but it does not
        // actually work here: with only Unity Hub in the ACL (by its correct
        // designated requirement, and with the team id in the item's
        // partition list) a teleported Unity Hub still raised "Unity Hub
        // wants to access key 'unity' in your keychain" on every launch, an
        // unanswerable SecurityAgent dialog inside a Space. The ACL app entry
        // is matched against the *reading* process, and for an Electron app
        // that is not reliably the bundle we can name.
        //
        // The exposure `-A` adds is bounded: the destination of a teleport is a
        // single-user throwaway VM whose whole purpose is to act as the user,
        // and anything that could read the item could equally read the profile
        // it decrypts. Correctness of the unattended run wins.
        //
        // The destination keychain is named explicitly: it is deliberately
        // not the default (see target_keychain_path), so an unqualified add
        // would land the item somewhere the partition-list write below does
        // not touch.
        let keychain = target_keychain_path(host).to_string_lossy().into_owned();
        let line = add_command_line(item, &keychain)?;
        let output = host.run(
            &HostCommand::new(EffectKind::KeychainWrite, "security")
                .args(["-q", "-i"])
                .stdin(line.into_bytes()),
        )?;
        // Interactive mode's exit status does not reliably reflect the
        // command's, so confirm the item exists (attributes only: neither
        // `-g` nor `-w`, so no secret is printed).
        let present = output.success
            && host
                .run(
                    &HostCommand::new(EffectKind::KeychainRead, "security").args([
                        "find-generic-password",
                        "-s",
                        &item.service,
                        "-a",
                        &item.account,
                        &keychain,
                    ]),
                )
                .map(|o| o.success)
                .unwrap_or(false);
        if !present {
            return Err(std::io::Error::other(format!(
                "security add-generic-password failed for {:?}",
                item.service
            )));
        }
        // `-T <app>` puts the app in the item's ACL, but macOS *also* gates
        // reads on the item's partition list. Without the app's code-signing
        // team in it, the trusted app still gets "<App> wants to access key
        // '<service>' in your keychain. To allow this, enter the 'login'
        // keychain password." — an unanswerable dialog inside an unattended
        // Space, which is what left a teleported Unity Hub signed in but
        // unlicensed ("Access token is unavailable").
        // ALWAYS, not only when a trust app is known: macOS gates reads on the
        // partition list independently of the ACL, and an item created by
        // `security` gets `apple-tool:` — which excludes every app that is not
        // an Apple command-line tool.
        set_partition_list(
            host,
            &item.service,
            &item.account,
            item.trust_app
                .as_deref()
                .and_then(|app| team_identifier(host, app))
                .as_deref(),
        );
        // Re-open the keychain IMMEDIATELY, per item, not just once after the
        // whole bundle. `set_partition_list` takes `-k <password>`, and every
        // `security` subcommand that does unlocks the keychain for the call and
        // RE-LOCKS it on the way out. In a Cua Space that keychain is also the
        // DEFAULT one, so for as long as it stays locked any process that
        // reaches for the default keychain — Spotlight's mdworker is the one
        // that actually did it — raises "<Process> wants to use the '<keychain>'
        // keychain", a SecurityAgent dialog nothing in an unattended Space can
        // answer. Doing this only after the last item leaves that window open
        // across every item but the last.
        ensure_login_unlocked(host);
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (host, item);
        Ok(())
    }
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
    #[cfg(target_os = "macos")]
    ensure_login_unlocked(host);
    let items = cua_teleport_bundle::keychain::parse(bytes);
    let installed = items
        .iter()
        .filter(|item| {
            let ok = install_generic(host, item).is_ok();
            if ok && cfg!(target_os = "macos") {
                record.keychain_installed(&item.service, &item.account);
            }
            ok
        })
        .count();
    // AGAIN, afterwards. `security` commands that take `-k <password>` — which
    // is every partition-list write above — unlock the keychain for the call
    // and RE-LOCK it when they finish. Leaving it locked is not a silent
    // detail: the app we just installed credentials for launches next and gets
    // "<App> wants to use the '<keychain>' keychain", a SecurityAgent dialog
    // that nothing in an unattended destination can answer.
    #[cfg(target_os = "macos")]
    ensure_login_unlocked(host);
    installed
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
fn set_partition_list(host: &dyn HostEffects, service: &str, account: &str, team_id: Option<&str>) {
    // Target whatever keychain the item actually went into, which is the
    // DEFAULT keychain: `security add-generic-password` without `-k` writes
    // there, not necessarily to login.keychain-db. Hardcoding the login
    // keychain silently no-ops on a destination whose default is something
    // else, and the app then hits "<App> wants to access key '<service>' in
    // your keychain" — which is exactly what a Cua Space does, because the
    // login keychain macOS creates at login does not take a password we know,
    // so the Space installs a `cua` keychain and makes that the default
    // instead. Falling back to the login keychain keeps the old behaviour for
    // hosts that never changed their default.
    //
    // "does not take a password we know" re-verified 2026-09-12 inside a Space,
    // in the aqua session as uid 501: this exact call with `-k lume` against
    // login.keychain-db returns "SecKeychainItemSetAccessWithPassword: The user
    // name or passphrase you entered is not correct". `security unlock-keychain
    // -p lume login.keychain-db` succeeding does NOT contradict that — unlocking
    // an already-unlocked keychain never checks the password.
    //
    // The destination keychain must therefore also be the DEFAULT one, not just
    // first on the search list: Unity Hub's keyring binding (and anything else
    // built on a `SecKeychain::default()`-style lookup) resolves the default
    // keychain only, so an item parked in a non-default keychain reads as
    // absent. The Space's prepare-space.sh makes the `cua` keychain the default
    // for that reason; CUA_ENV_KEYCHAIN names the same one.
    let path = target_keychain_path(host);
    let pw = std::env::var("CUA_ENV_LOGIN_KEYCHAIN_PW").unwrap_or_else(|_| "lume".to_string());
    let partitions = match team_id {
        // apple-tool: keeps /usr/bin/security itself able to touch the item
        // (the golden's per-boot verification uses it), apple: covers
        // Apple-signed callers, and the team id covers the destination app.
        Some(id) => format!("teamid:{id},apple:,apple-tool:"),
        None => "apple:,apple-tool:".to_string(),
    };
    let mut args: Vec<String> = vec![
        "set-generic-password-partition-list".into(),
        "-S".into(),
        partitions,
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
    // Report a failure. This call is best-effort in the sense that the bundle
    // still lands without it, but its failure mode is a SecurityAgent dialog
    // nothing in an unattended destination can answer, so swallowing the error
    // turned a diagnosable misconfiguration into an hour of a demo run staring
    // at a password box. `security` writes the reason to stderr.
    match host.run(&HostCommand::new(EffectKind::KeychainWrite, "security").args(args)) {
        Ok(out) if out.success => {}
        Ok(out) => eprintln!(
            "warning: could not authorize keychain item {service:?} for silent reads \
             ({}); the destination app will raise an \"wants to access key\" dialog. \
             Keychain: {}",
            String::from_utf8_lossy(&out.stderr).trim(),
            path.display()
        ),
        Err(e) => eprintln!("warning: could not run security set-…-partition-list: {e}"),
    }
}

/// The keychain teleported items are installed into.
///
/// `CUA_ENV_KEYCHAIN` names it explicitly, which is what an unattended
/// destination should do: the login keychain macOS creates at login has a
/// password we cannot reproduce, so the partition-list write below fails on
/// it, and making a keychain we *did* create the DEFAULT instead redirects
/// Spotlight and other system agents into it — both end in a SecurityAgent
/// dialog nobody can answer. Naming one keychain keeps the default alone.
/// Falls back to the default keychain, then to the login keychain.
#[cfg(target_os = "macos")]
/// Every user keychain on the search list, plus our explicit target, as paths.
///
/// `security list-keychains` prints them quoted, one per line. The target is
/// included even if it is not (yet) on the list, so a fresh keychain is still
/// cleaned.
fn search_list_keychains(host: &dyn HostEffects) -> Vec<String> {
    let mut out = vec![target_keychain_path(host).to_string_lossy().into_owned()];
    if let Ok(o) = host.run(
        &HostCommand::new(EffectKind::KeychainRead, "security").args([
            "list-keychains",
            "-d",
            "user",
        ]),
    ) {
        for line in String::from_utf8_lossy(&o.stdout).lines() {
            let path = line.trim().trim_matches('"').to_string();
            if !path.is_empty() && !out.contains(&path) {
                out.push(path);
            }
        }
    }
    out
}

#[cfg(target_os = "macos")]
fn target_keychain_path(host: &dyn HostEffects) -> std::path::PathBuf {
    if let Some(p) = std::env::var_os("CUA_ENV_KEYCHAIN") {
        let p = std::path::PathBuf::from(p);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    default_keychain_path(host).unwrap_or_else(|| {
        host.home_dir()
            .unwrap_or_default()
            .join("Library/Keychains/login.keychain-db")
    })
}

/// Path of the user's default keychain — where `security add-generic-password`
/// puts a new item when no keychain is named. `security default-keychain`
/// prints it quoted, one line.
#[cfg(target_os = "macos")]
fn default_keychain_path(host: &dyn HostEffects) -> Option<std::path::PathBuf> {
    let out = host
        .run(
            &HostCommand::new(EffectKind::KeychainRead, "security").args([
                "default-keychain",
                "-d",
                "user",
            ]),
        )
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let path = text.trim().trim_matches('"').trim();
    if path.is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(path))
    }
}

/// Best-effort: make sure the destination's login Keychain won't auto-lock and
/// re-prompt after we install into it. NON-destructive by design: recreating a
/// keychain the session already holds open does not give us an unlocked one, it
/// gives the session a *different*, locked file at the same path, and the first
/// app to touch it raises "<App> wants to use the '<keychain>' keychain" — an
/// unanswerable SecurityAgent dialog in an unattended destination. So we only
/// try a harmless unlock with the account password (which helps for the
/// keychain the destination created itself, e.g. the Space's `cua` keychain,
/// and is a no-op for the login keychain, whose password we do not know) and
/// disable the auto-lock timeout.
/// Password overridable via `CUA_ENV_LOGIN_KEYCHAIN_PW` (default "lume").
#[cfg(target_os = "macos")]
fn ensure_login_unlocked(host: &dyn HostEffects) {
    let Some(home) = host.home_dir() else {
        return;
    };
    let pw = std::env::var("CUA_ENV_LOGIN_KEYCHAIN_PW").unwrap_or_else(|_| "lume".to_string());
    // The login keychain, and the default keychain if it is a different one —
    // the items go into the default, so that is the one that has to be open.
    let mut paths = vec![home.join("Library/Keychains/login.keychain-db")];
    for extra in [
        default_keychain_path(host),
        Some(target_keychain_path(host)),
    ]
    .into_iter()
    .flatten()
    {
        if !paths.contains(&extra) {
            paths.push(extra);
        }
    }
    for path in paths {
        let path_str = path.to_string_lossy().into_owned();
        let _ = host.run(
            &HostCommand::new(EffectKind::KeychainWrite, "security").args([
                "unlock-keychain",
                "-p",
                &pw,
                &path_str,
            ]),
        );
        let _ = host.run(
            &HostCommand::new(EffectKind::KeychainWrite, "security")
                .args(["set-keychain-settings", &path_str]),
        );
    }
}

/// True for the `security -i` call that adds an item (tests).
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
    fn remove_goes_through_the_injected_host() {
        let host = FakeHost::new()
            .with_home("/nonexistent-fake-home")
            .with_responder(|c| {
                Ok(
                    if c.args.first().map(String::as_str) == Some("delete-generic-password") {
                        HostOutput::ok("")
                    } else {
                        HostOutput::failed()
                    },
                )
            });
        assert!(remove_generic(&host, "svc", "acct").unwrap());
        let deletes = host.calls_of(EffectKind::KeychainWrite);
        assert!(!deletes.is_empty());
        assert!(deletes.iter().all(|c| c.program == "security"
            && c.args[..5] == ["delete-generic-password", "-s", "svc", "-a", "acct"]));
    }
}
