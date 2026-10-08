//! The macOS login keychain item behind [`crate::Store::Keyring`], written
//! with an access list that trusts every Cua executable of this machine.
//!
//! Why: the legacy keychain gives a new generic password the default
//! access list, which trusts only the executable that created it. The Cua
//! Spaces app (`Contents/MacOS/CuaSpacesMac`) signs in and creates the
//! `run.cua.ai` / `cua-cli` item; the bundled `cua` daemon and CLI
//! (`Contents/MacOS/cua`, same Developer ID team, different executable)
//! then read it and macOS asks for the login password ("cua wants to use
//! your confidential information stored in run.cua.ai"), right after every
//! first sign-in.
//!
//! The fix keeps the default model (an explicit list of trusted
//! applications, everyone else is asked) and only widens the list: the
//! creating executable plus the other executables of the same app bundle
//! (`Contents/MacOS/*`) and, for a `cua` outside a bundle, those of the
//! installed `Cua Spaces.app`. Trusted-application entries record each
//! binary's designated requirement (bundle id + Cua's team), so they keep
//! matching after updates, and a copy of `cua` elsewhere with the same
//! signature matches too. Unlike an "allow all applications" entry
//! (`SecACLSetContents` with no application list; note an *empty*
//! `SecAccessCreate` list trusts no one, and NULL trusts only the caller),
//! this never depends on the item's partition list (`teamid:YCK386LBJ7`)
//! to keep other apps out.
//!
//! An access list can only be given when an item is created (changing the
//! access list of an existing item asks for the keychain password since
//! macOS 10.12), so an item that an older build created with the default
//! list is deleted and created again the next time its creator writes it
//! (token refreshes write it often). Items written this way carry the
//! comment [`ACL_MARKER`], so that happens once.
//!
//! Only a Cua-signed (Developer ID) writer gives stable entries: an ad hoc
//! or unsigned build's designated requirement is its cdhash, which the next
//! build does not meet, and its items get the partition of that build
//! rather than `teamid:YCK386LBJ7`. So a Cua-signed writer marks its items
//! [`ACL_MARKER`] (`v3`) and recreates every other item, including the `v2`
//! ones an unsigned writer marks [`ACL_MARKER_UNSIGNED`]. A Cua-signed
//! *reader* does the same right after a successful read ([`adopt`]): the
//! first time a Developer ID build reads an item an ad hoc build wrote
//! (the one keychain prompt, which the Spaces app asks for from a visible
//! window through `cua auth keychain --prompt`), the item is recreated with
//! the Cua access list, so no later Cua build asks again.
//!
//! [`check_items`] reads every Cua item without any prompt (user interaction
//! disabled for the call) to tell "readable" from "needs a prompt", which
//! the Spaces app asks before it starts anything that reads the session.

use core_foundation::base::TCFType as _;
use security_framework::item::{ItemClass, ItemSearchOptions, Limit};
use security_framework::os::macos::keychain::SecKeychain;
use std::ffi::{CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::ptr;

type OSStatus = i32;
type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CFArrayRef = *const c_void;
type SecRef = *mut c_void;

#[repr(C)]
struct SecKeychainAttribute {
    tag: u32,
    length: u32,
    data: *mut c_void,
}

#[repr(C)]
struct SecKeychainAttributeList {
    count: u32,
    attr: *mut SecKeychainAttribute,
}

#[repr(C)]
struct CFArrayCallBacks {
    _private: [u8; 0],
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFTypeArrayCallBacks: CFArrayCallBacks;
    fn CFRelease(cf: CFTypeRef);
    fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
    fn CFArrayCreate(
        allocator: CFTypeRef,
        values: *const CFTypeRef,
        count: isize,
        callbacks: *const CFArrayCallBacks,
    ) -> CFArrayRef;
    fn CFStringCreateWithCString(
        allocator: CFTypeRef,
        s: *const c_char,
        encoding: u32,
    ) -> CFStringRef;
}

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SecKeychainCopyDefault(keychain: *mut SecRef) -> OSStatus;
    fn SecKeychainGetStatus(keychain: SecRef, status: *mut u32) -> OSStatus;
    fn SecKeychainFindGenericPassword(
        keychain_or_array: CFTypeRef,
        service_len: u32,
        service: *const c_char,
        account_len: u32,
        account: *const c_char,
        password_len: *mut u32,
        password: *mut *mut c_void,
        item: *mut SecRef,
    ) -> OSStatus;
    fn SecKeychainItemCopyContent(
        item: SecRef,
        item_class: *mut u32,
        attr_list: *mut SecKeychainAttributeList,
        length: *mut u32,
        out_data: *mut *mut c_void,
    ) -> OSStatus;
    fn SecKeychainItemFreeContent(
        attr_list: *mut SecKeychainAttributeList,
        data: *mut c_void,
    ) -> OSStatus;
    fn SecKeychainItemModifyAttributesAndData(
        item: SecRef,
        attr_list: *const SecKeychainAttributeList,
        length: u32,
        data: *const c_void,
    ) -> OSStatus;
    fn SecKeychainItemDelete(item: SecRef) -> OSStatus;
    fn SecKeychainItemCreateFromContent(
        item_class: u32,
        attr_list: *mut SecKeychainAttributeList,
        length: u32,
        data: *const c_void,
        keychain: SecRef,
        initial_access: SecRef,
        item: *mut SecRef,
    ) -> OSStatus;
    fn SecTrustedApplicationCreateFromPath(path: *const c_char, app: *mut SecRef) -> OSStatus;
    fn SecAccessCreate(
        descriptor: CFStringRef,
        trusted_list: CFArrayRef,
        access: *mut SecRef,
    ) -> OSStatus;
    fn SessionGetInfo(session: u32, session_id: *mut u32, attributes: *mut u32) -> OSStatus;
    fn SecKeychainGetUserInteractionAllowed(state: *mut u8) -> OSStatus;
    fn SecKeychainSetUserInteractionAllowed(state: u8) -> OSStatus;
}

/// Keychain prompts off (process-wide) while it lives: a call that would
/// prompt fails with `errSecInteractionNotAllowed` instead. Restores the
/// previous setting on drop, so it nests.
pub(crate) struct Quiet(u8);

impl Quiet {
    pub(crate) fn new() -> Self {
        let mut before = 1u8;
        unsafe {
            SecKeychainGetUserInteractionAllowed(&mut before);
            SecKeychainSetUserInteractionAllowed(0);
        }
        Quiet(before)
    }
}

impl Drop for Quiet {
    fn drop(&mut self) {
        unsafe { SecKeychainSetUserInteractionAllowed(self.0) };
    }
}

const ERR_SEC_SUCCESS: OSStatus = 0;
const ERR_SEC_ITEM_NOT_FOUND: OSStatus = -25300;
/// `errSecAuthFailed`: the user denied the prompt or gave a wrong password.
const ERR_SEC_AUTH_FAILED: OSStatus = -25293;
/// `userCanceledErr`: the user cancelled the prompt.
const USER_CANCELED: OSStatus = -128;
/// `errSecInteractionNotAllowed`: the keychain needs a prompt (unlock or
/// access confirmation) that this session cannot show (an SSH session).
pub(crate) const ERR_SEC_INTERACTION_NOT_ALLOWED: OSStatus = -25308;

const GENERIC_PASSWORD_CLASS: u32 = u32::from_be_bytes(*b"genp");
const ATTR_SERVICE: u32 = u32::from_be_bytes(*b"svce");
const ATTR_ACCOUNT: u32 = u32::from_be_bytes(*b"acct");
const ATTR_LABEL: u32 = u32::from_be_bytes(*b"labl");
const ATTR_COMMENT: u32 = u32::from_be_bytes(*b"icmt");
const UNLOCKED: u32 = 1;
const CALLER_SECURITY_SESSION: u32 = u32::MAX;
const SESSION_HAS_GRAPHIC_ACCESS: u32 = 0x0010;
const UTF8: u32 = 0x0800_0100;

/// The comment of an item a Cua-signed (Developer ID) process wrote with
/// the Cua access list: every entry is a designated requirement (bundle id
/// and Cua's team), which every later Cua build meets.
pub(crate) const ACL_MARKER: &str = "cua-acl/v3";

/// The comment of an item an unsigned or ad hoc process wrote with the Cua
/// access list (`CUA_CREDENTIAL_STORE=keychain`): its entries name that
/// build's cdhash, so a Cua-signed writer or reader recreates it.
pub(crate) const ACL_MARKER_UNSIGNED: &str = "cua-acl/v2";

/// The marker a writer leaves.
fn marker(signed: bool) -> &'static str {
    if signed { ACL_MARKER } else { ACL_MARKER_UNSIGNED }
}

/// Whether a writer (Cua-signed or not) recreates an item with `comment`
/// rather than updating it in place: anything without the Cua access list,
/// and, for a Cua-signed writer, the list an unsigned build wrote.
pub(crate) fn must_recreate(comment: Option<&str>, signed: bool) -> bool {
    match comment {
        Some(ACL_MARKER) => false,
        Some(ACL_MARKER_UNSIGNED) => signed,
        _ => true,
    }
}

/// The installed app a `cua` outside a bundle also trusts.
const INSTALLED_APP: &str = "/Applications/Cua Spaces.app";

/// A Security / CoreFoundation object released on drop.
struct Owned(SecRef);

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0 as CFTypeRef) };
        }
    }
}

/// A Security framework failure.
#[derive(Debug)]
pub(crate) struct Status(pub OSStatus);

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", security_framework::base::Error::from_code(self.0))
    }
}

fn check(status: OSStatus) -> Result<(), Status> {
    if status == ERR_SEC_SUCCESS {
        Ok(())
    } else {
        Err(Status(status))
    }
}

/// The executables an item this process creates trusts besides itself:
/// the other executables in the `Contents/MacOS` of the app bundle `exe`
/// runs from, else (a `cua` outside a bundle) those of `installed_app`.
pub(crate) fn companion_executables(exe: &Path, installed_app: &Path) -> Vec<PathBuf> {
    let exe = exe.canonicalize().unwrap_or_else(|_| exe.to_path_buf());
    let bundle = exe
        .ancestors()
        .find(|d| d.extension().is_some_and(|e| e == "app"))
        .map(Path::to_path_buf)
        .unwrap_or_else(|| installed_app.to_path_buf());
    let Ok(entries) = std::fs::read_dir(bundle.join("Contents/MacOS")) else {
        return vec![];
    };
    let mut out: Vec<PathBuf> = entries
        .filter_map(|e| e.ok()?.path().canonicalize().ok())
        .filter(|p| p != &exe && is_executable(p))
        .collect();
    out.sort();
    out.dedup();
    out
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// This process's companion executables ([`companion_executables`]; a
/// test sets its own bundle).
fn companions() -> Vec<PathBuf> {
    #[cfg(test)]
    if let Some(exe) = tests::AS_EXE.with(|e| e.borrow().clone()) {
        return companion_executables(&exe, Path::new("/nonexistent/Cua Spaces.app"));
    }
    let exe = std::env::current_exe().unwrap_or_default();
    companion_executables(&exe, Path::new(INSTALLED_APP))
}

/// An access object trusting this process and its companion executables.
fn cua_access(descriptor: &str) -> Result<Owned, Status> {
    let mut apps: Vec<Owned> = Vec::new();
    let mut me: SecRef = ptr::null_mut();
    // NULL: the calling application.
    check(unsafe { SecTrustedApplicationCreateFromPath(ptr::null(), &mut me) })?;
    apps.push(Owned(me));
    for path in companions() {
        let Ok(c) = CString::new(path.as_os_str().as_encoded_bytes()) else {
            continue;
        };
        let mut app: SecRef = ptr::null_mut();
        // A companion that cannot be described is left out (it asks).
        if unsafe { SecTrustedApplicationCreateFromPath(c.as_ptr(), &mut app) } == ERR_SEC_SUCCESS {
            apps.push(Owned(app));
        }
    }
    let values: Vec<CFTypeRef> = apps.iter().map(|a| a.0 as CFTypeRef).collect();
    let list = Owned(unsafe {
        CFArrayCreate(
            ptr::null(),
            values.as_ptr(),
            values.len() as isize,
            &kCFTypeArrayCallBacks,
        )
    } as SecRef);
    let desc = CString::new(descriptor).unwrap_or_default();
    let desc =
        Owned(unsafe { CFStringCreateWithCString(ptr::null(), desc.as_ptr(), UTF8) } as SecRef);
    if list.0.is_null() || desc.0.is_null() {
        return Err(Status(-108)); // memFullErr
    }
    let mut access: SecRef = ptr::null_mut();
    check(unsafe { SecAccessCreate(desc.0 as CFStringRef, list.0 as CFArrayRef, &mut access) })?;
    Ok(Owned(access))
}

fn default_keychain() -> Result<Owned, Status> {
    let mut kc: SecRef = ptr::null_mut();
    check(unsafe { SecKeychainCopyDefault(&mut kc) })?;
    Ok(Owned(kc))
}

/// The keychain an operation uses: the user's default keychain (reads
/// search the keychain search list), or the given one (tests use a
/// throwaway keychain in a temporary directory, never the login keychain).
#[derive(Clone, Copy)]
pub(crate) enum Keychain<'a> {
    Default,
    #[cfg_attr(not(test), allow(dead_code))]
    Given(&'a SecKeychain),
}

impl Keychain<'_> {
    /// The keychain to create items in.
    fn target(self) -> Result<Owned, Status> {
        match self {
            Keychain::Default => default_keychain(),
            Keychain::Given(k) => {
                let r = k.as_concrete_TypeRef() as SecRef;
                // Owned releases: retain once for it.
                unsafe { CFRetain(r as CFTypeRef) };
                Ok(Owned(r))
            }
        }
    }

    /// The keychain (or NULL: the search list) to search.
    fn search(self) -> CFTypeRef {
        match self {
            Keychain::Default => ptr::null(),
            Keychain::Given(k) => k.as_concrete_TypeRef() as CFTypeRef,
        }
    }
}

/// The item, without reading its secret (no prompt).
fn find_item(kc: Keychain<'_>, service: &str, account: &str) -> Result<Option<Owned>, Status> {
    let mut item: SecRef = ptr::null_mut();
    let status = unsafe {
        SecKeychainFindGenericPassword(
            kc.search(),
            service.len() as u32,
            service.as_ptr().cast(),
            account.len() as u32,
            account.as_ptr().cast(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut item,
        )
    };
    match status {
        ERR_SEC_SUCCESS => Ok(Some(Owned(item))),
        ERR_SEC_ITEM_NOT_FOUND => Ok(None),
        s => Err(Status(s)),
    }
}

/// The item's secret (macOS asks first when this process is not trusted,
/// unless user interaction is disabled: then `errSecInteractionNotAllowed`).
fn read_secret(kc: Keychain<'_>, service: &str, account: &str) -> Result<Option<Vec<u8>>, Status> {
    let mut len = 0u32;
    let mut data: *mut c_void = ptr::null_mut();
    let status = unsafe {
        SecKeychainFindGenericPassword(
            kc.search(),
            service.len() as u32,
            service.as_ptr().cast(),
            account.len() as u32,
            account.as_ptr().cast(),
            &mut len,
            &mut data,
            ptr::null_mut(),
        )
    };
    match status {
        ERR_SEC_SUCCESS => {
            let out = if data.is_null() {
                Vec::new()
            } else {
                unsafe { std::slice::from_raw_parts(data as *const u8, len as usize) }.to_vec()
            };
            unsafe { SecKeychainItemFreeContent(ptr::null_mut(), data) };
            Ok(Some(out))
        }
        ERR_SEC_ITEM_NOT_FOUND => Ok(None),
        s => Err(Status(s)),
    }
}

/// The item's comment (an unencrypted attribute: no prompt).
fn comment(item: &Owned) -> Option<String> {
    let mut attr = SecKeychainAttribute {
        tag: ATTR_COMMENT,
        length: 0,
        data: ptr::null_mut(),
    };
    let mut list = SecKeychainAttributeList {
        count: 1,
        attr: &mut attr,
    };
    let status = unsafe {
        SecKeychainItemCopyContent(
            item.0,
            ptr::null_mut(),
            &mut list,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    if status != ERR_SEC_SUCCESS {
        return None;
    }
    let a = unsafe { &*list.attr };
    let text = (!a.data.is_null()).then(|| {
        let bytes = unsafe { std::slice::from_raw_parts(a.data as *const u8, a.length as usize) };
        String::from_utf8_lossy(bytes).into_owned()
    });
    unsafe { SecKeychainItemFreeContent(&mut list, ptr::null_mut()) };
    text
}

/// Creates the item with the Cua access list, marked as `signed` says.
fn create(
    kc: Keychain<'_>,
    service: &str,
    account: &str,
    secret: &[u8],
    signed: bool,
) -> Result<(), Status> {
    let access = cua_access(service)?;
    create_with(kc, service, account, secret, Some((&access, marker(signed))))
}

/// Creates the item with `access` and its marker, or (`None`) with the
/// default access list (this process only) and no marker.
fn create_with(
    kc: Keychain<'_>,
    service: &str,
    account: &str,
    secret: &[u8],
    access: Option<(&Owned, &str)>,
) -> Result<(), Status> {
    let kc = kc.target()?;
    let comment = access.map(|(_, m)| m).unwrap_or_default();
    let attr = |tag: u32, v: &str| SecKeychainAttribute {
        tag,
        length: v.len() as u32,
        data: v.as_ptr() as *mut c_void,
    };
    let mut attrs = [
        attr(ATTR_SERVICE, service),
        attr(ATTR_ACCOUNT, account),
        attr(ATTR_LABEL, service),
        attr(ATTR_COMMENT, comment),
    ];
    // No marker: no comment attribute at all.
    let count = if access.is_some() { attrs.len() } else { attrs.len() - 1 };
    let mut list = SecKeychainAttributeList {
        count: count as u32,
        attr: attrs.as_mut_ptr(),
    };
    let mut item: SecRef = ptr::null_mut();
    check(unsafe {
        SecKeychainItemCreateFromContent(
            GENERIC_PASSWORD_CLASS,
            &mut list,
            secret.len() as u32,
            secret.as_ptr().cast(),
            kc.0,
            access.map_or(ptr::null_mut(), |(a, _)| a.0),
            &mut item,
        )
    })?;
    drop(Owned(item));
    Ok(())
}

fn modify(item: &Owned, secret: &[u8]) -> Result<(), Status> {
    check(unsafe {
        SecKeychainItemModifyAttributesAndData(
            item.0,
            ptr::null(),
            secret.len() as u32,
            secret.as_ptr().cast(),
        )
    })
}

/// Stores `secret` as the generic password `service` / `account` in the
/// default keychain, trusted by the Cua executables (see the module docs).
pub(crate) fn set_generic_password(
    service: &str,
    account: &str,
    secret: &[u8],
) -> Result<(), Status> {
    set_in(Keychain::Default, service, account, secret, crate::signed_by_cua())
}

/// [`set_generic_password`] in `kc`, for a writer that is Cua-signed or not.
pub(crate) fn set_in(
    kc: Keychain<'_>,
    service: &str,
    account: &str,
    secret: &[u8],
    signed: bool,
) -> Result<(), Status> {
    match find_item(kc, service, account)? {
        None => create(kc, service, account, secret, signed),
        Some(item) if !must_recreate(comment(&item).as_deref(), signed) => modify(&item, secret),
        Some(item) => {
            // Written without this writer's access list: recreate it. If
            // it cannot be deleted (not ours to delete), update it in place
            // as before.
            if recreate(kc, item, service, account, secret, signed)? {
                Ok(())
            } else {
                let item = find_item(kc, service, account)?.ok_or(Status(ERR_SEC_ITEM_NOT_FOUND))?;
                modify(&item, secret)
            }
        }
    }
}

/// The account a recreated item is written under first (never listed as a
/// Cua item of its own).
pub(crate) const STAGING_SUFFIX: &str = "~staging";

/// Replaces `item` with one carrying the Cua access list, never losing the
/// secret: the new item is written first (under `<account>~staging`), then
/// the old one deleted, then the new one renamed. `false` (nothing changed)
/// when the new one cannot be written or the old one cannot be deleted. A
/// rename that fails leaves the staged copy, which [`recover_staged`]
/// renames on the next read.
fn recreate(
    kc: Keychain<'_>,
    item: Owned,
    service: &str,
    account: &str,
    secret: &[u8],
    signed: bool,
) -> Result<bool, Status> {
    let staging = format!("{account}{STAGING_SUFFIX}");
    if let Some(stale) = find_item(kc, service, &staging)? {
        unsafe { SecKeychainItemDelete(stale.0) };
    }
    if create(kc, service, &staging, secret, signed).is_err() {
        return Ok(false);
    }
    if unsafe { SecKeychainItemDelete(item.0) } != ERR_SEC_SUCCESS {
        // Not ours to delete: keep the old one as it was.
        if let Some(staged) = find_item(kc, service, &staging)? {
            unsafe { SecKeychainItemDelete(staged.0) };
        }
        return Ok(false);
    }
    drop(item);
    let staged = find_item(kc, service, &staging)?.ok_or(Status(ERR_SEC_ITEM_NOT_FOUND))?;
    rename(&staged, account)?;
    Ok(true)
}

/// Gives the item another account (attributes only: no secret read).
fn rename(item: &Owned, account: &str) -> Result<(), Status> {
    let mut attr = SecKeychainAttribute {
        tag: ATTR_ACCOUNT,
        length: account.len() as u32,
        data: account.as_ptr() as *mut c_void,
    };
    let list = SecKeychainAttributeList {
        count: 1,
        attr: &mut attr,
    };
    check(unsafe { SecKeychainItemModifyAttributesAndData(item.0, &list, 0, ptr::null()) })
}

/// When `account` is missing but a recreate left its staged copy (it
/// stopped between the delete and the rename), renames the copy back.
/// Returns whether it did.
pub(crate) fn recover_staged(kc: Keychain<'_>, service: &str, account: &str) -> bool {
    let _quiet = Quiet::new();
    if !matches!(find_item(kc, service, account), Ok(None)) {
        return false;
    }
    match find_item(kc, service, &format!("{account}{STAGING_SUFFIX}")) {
        Ok(Some(staged)) => rename(&staged, account).is_ok(),
        _ => false,
    }
}

/// After this process read `secret` from the item: a Cua-signed process
/// recreates an item that lacks the Cua access list (written by an older or
/// ad hoc build), so later Cua builds read it without a prompt. Returns
/// whether it did. Unsigned processes never change an item they only read.
/// `quiet`: never prompt for the delete (an ordinary read, which may run
/// on an app's main thread); it is left as is when the delete would ask.
/// Only the Spaces app's explicit `cua auth keychain --prompt`, with its
/// waiting window up, lets macOS ask.
pub(crate) fn adopt(
    kc: Keychain<'_>,
    service: &str,
    account: &str,
    secret: &[u8],
    signed: bool,
    quiet: bool,
) -> Result<bool, Status> {
    if !signed {
        return Ok(false);
    }
    let _quiet = quiet.then(Quiet::new);
    let Some(item) = find_item(kc, service, account)? else {
        return Ok(false);
    };
    if !must_recreate(comment(&item).as_deref(), true) {
        return Ok(false);
    }
    recreate(kc, item, service, account, secret, true)
}

/// What reading one Cua item found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ItemState {
    /// Read without a prompt (or after the one asked for); `recreated`: it
    /// now carries the Cua access list.
    Ready { recreated: bool },
    /// Reading it needs a prompt (an untrusted build, or a locked keychain).
    NeedsAccess,
    /// The user denied the prompt.
    Denied,
    /// Anything else.
    Failed(String),
}

/// The accounts of `service`'s generic passwords that `wanted` keeps (the
/// session first), read from their attributes only (no prompt).
fn accounts(kc: Keychain<'_>, service: &str, wanted: &dyn Fn(&str) -> bool) -> Vec<String> {
    let mut search = ItemSearchOptions::new();
    search
        .class(ItemClass::generic_password())
        .service(service)
        .load_attributes(true)
        .limit(Limit::All);
    if let Keychain::Given(k) = kc {
        search.keychains(std::slice::from_ref(k));
    }
    let mut out: Vec<String> = search
        .search()
        .unwrap_or_default()
        .iter()
        .filter_map(|r| r.simplify_dict()?.get("acct").cloned())
        .filter(|a| !a.ends_with(STAGING_SUFFIX) && wanted(a))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Reads every Cua item of `service` (`wanted` picks the accounts),
/// without any prompt unless `prompt`, and recreates the ones read that
/// lack the Cua access list (a Cua-signed reader only, see [`adopt`]).
pub(crate) fn check_items(
    kc: Keychain<'_>,
    service: &str,
    wanted: &dyn Fn(&str) -> bool,
    prompt: bool,
    signed: bool,
) -> Vec<(String, ItemState)> {
    // No prompt: every call below fails with errSecInteractionNotAllowed
    // instead of showing one (this process-wide switch is why the Spaces
    // app runs the check in a `cua` of its own, not in-process).
    let _quiet = (!prompt).then(Quiet::new);
    accounts(kc, service, wanted)
        .into_iter()
        .map(|account| {
            let state = match read_secret(kc, service, &account) {
                Ok(Some(secret)) => match adopt(kc, service, &account, &secret, signed, !prompt) {
                    Ok(recreated) => ItemState::Ready { recreated },
                    Err(_) => ItemState::Ready { recreated: false },
                },
                // Gone since it was listed.
                Ok(None) => ItemState::Ready { recreated: false },
                Err(Status(ERR_SEC_INTERACTION_NOT_ALLOWED)) => ItemState::NeedsAccess,
                // With interaction disabled, an access list that would ask
                // fails as "auth failed" (no one was asked); after a
                // prompt, it means the user denied it.
                Err(Status(ERR_SEC_AUTH_FAILED)) if !prompt => ItemState::NeedsAccess,
                Err(Status(ERR_SEC_AUTH_FAILED | USER_CANCELED)) => ItemState::Denied,
                Err(e) => ItemState::Failed(e.to_string()),
            };
            (account, state)
        })
        .collect()
}

/// Whether this process's security session can show UI (a console login
/// can; an SSH session cannot). `true` when unknown.
pub(crate) fn has_graphic_access() -> bool {
    let (mut id, mut attrs) = (0u32, 0u32);
    if unsafe { SessionGetInfo(CALLER_SECURITY_SESSION, &mut id, &mut attrs) } != ERR_SEC_SUCCESS {
        return true;
    }
    attrs & SESSION_HAS_GRAPHIC_ACCESS != 0
}

/// Whether the default keychain is unlocked (`None` when unknown).
pub(crate) fn default_keychain_unlocked() -> Option<bool> {
    let kc = default_keychain().ok()?;
    let mut status = 0u32;
    (unsafe { SecKeychainGetStatus(kc.0, &mut status) } == ERR_SEC_SUCCESS)
        .then_some(status & UNLOCKED != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exe(p: &Path) {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"\0").unwrap();
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn companions_are_the_other_executables_of_the_bundle() {
        let d = tempfile::tempdir().unwrap();
        let macos = d.path().join("Cua Spaces.app/Contents/MacOS");
        exe(&macos.join("CuaSpacesMac"));
        exe(&macos.join("cua"));
        std::fs::write(macos.join("notes.txt"), b"x").unwrap();
        let installed = d.path().join("Elsewhere.app");
        let app = macos.join("CuaSpacesMac").canonicalize().unwrap();
        let cua = macos.join("cua").canonicalize().unwrap();
        assert_eq!(companion_executables(&app, &installed), vec![cua.clone()]);
        assert_eq!(companion_executables(&cua, &installed), vec![app.clone()]);
        // A `cua` outside a bundle trusts the installed app's executables.
        let loose = d.path().join("bin/cua");
        exe(&loose);
        let other = d.path().join("Cua Spaces.app");
        let mut both = companion_executables(&loose, &other);
        both.sort();
        assert_eq!(both, vec![app, cua]);
        // No app installed: nobody else.
        assert!(companion_executables(&loose, &installed).is_empty());
    }

    #[test]
    fn four_char_codes() {
        assert_eq!(GENERIC_PASSWORD_CLASS, 0x6765_6e70);
        assert_eq!(ATTR_COMMENT, 0x6963_6d74);
    }

    #[test]
    fn a_cua_signed_writer_recreates_every_item_without_its_list() {
        // No list, or an unknown comment: every writer recreates it.
        assert!(must_recreate(None, false));
        assert!(must_recreate(None, true));
        assert!(must_recreate(Some("note"), true));
        // An unsigned writer's list (cdhash entries): only a signed one.
        assert!(!must_recreate(Some(ACL_MARKER_UNSIGNED), false));
        assert!(must_recreate(Some(ACL_MARKER_UNSIGNED), true));
        // A signed writer's list is kept by everyone.
        assert!(!must_recreate(Some(ACL_MARKER), false));
        assert!(!must_recreate(Some(ACL_MARKER), true));
    }

    unsafe extern "C" {
        fn SecKeychainDelete(keychain: SecRef) -> OSStatus;
        fn SecKeychainItemCopyAccess(item: SecRef, access: *mut SecRef) -> OSStatus;
        fn SecAccessCopyACLList(access: SecRef, list: *mut CFArrayRef) -> OSStatus;
        fn SecACLCopyContents(
            acl: SecRef,
            apps: *mut CFArrayRef,
            description: *mut CFStringRef,
            prompt: *mut u16,
        ) -> OSStatus;
        fn SecTrustedApplicationCopyData(app: SecRef, data: *mut CFTypeRef) -> OSStatus;
        fn CFArrayGetCount(a: CFArrayRef) -> isize;
        fn CFArrayGetValueAtIndex(a: CFArrayRef, i: isize) -> CFTypeRef;
        fn CFDataGetLength(d: CFTypeRef) -> isize;
        fn CFDataGetBytePtr(d: CFTypeRef) -> *const u8;
    }

    thread_local! {
        /// The executable `companions` takes this thread to be.
        pub(super) static AS_EXE: std::cell::RefCell<Option<PathBuf>> =
            const { std::cell::RefCell::new(None) };
    }

    /// The paths of the applications the item's access list trusts (every
    /// entry with an application list).
    fn trusted_paths(t: &TestKeychain, account: &str) -> Vec<String> {
        let item = find_item(t.at(), SERVICE, account).unwrap().unwrap();
        let mut access: SecRef = ptr::null_mut();
        check(unsafe { SecKeychainItemCopyAccess(item.0, &mut access) }).unwrap();
        let access = Owned(access);
        let mut acls: CFArrayRef = ptr::null();
        check(unsafe { SecAccessCopyACLList(access.0, &mut acls) }).unwrap();
        let acls = Owned(acls as SecRef);
        let mut out = vec![];
        for i in 0..unsafe { CFArrayGetCount(acls.0 as CFArrayRef) } {
            let acl = unsafe { CFArrayGetValueAtIndex(acls.0 as CFArrayRef, i) } as SecRef;
            let (mut apps, mut desc, mut prompt) = (ptr::null(), ptr::null(), 0u16);
            if unsafe { SecACLCopyContents(acl, &mut apps, &mut desc, &mut prompt) } != 0 {
                continue;
            }
            let _desc = Owned(desc as SecRef);
            if apps.is_null() {
                continue;
            }
            let apps = Owned(apps as SecRef);
            for j in 0..unsafe { CFArrayGetCount(apps.0 as CFArrayRef) } {
                let app = unsafe { CFArrayGetValueAtIndex(apps.0 as CFArrayRef, j) } as SecRef;
                let mut data: CFTypeRef = ptr::null();
                if unsafe { SecTrustedApplicationCopyData(app, &mut data) } == 0 && !data.is_null() {
                    let data = Owned(data as SecRef);
                    let bytes = unsafe {
                        std::slice::from_raw_parts(
                            CFDataGetBytePtr(data.0 as CFTypeRef),
                            CFDataGetLength(data.0 as CFTypeRef) as usize,
                        )
                    };
                    out.push(String::from_utf8_lossy(bytes).trim_end_matches('\0').to_string());
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// A throwaway keychain in a temporary directory (never the login
    /// keychain), unlocked with its own password and deleted on drop.
    struct TestKeychain {
        kc: SecKeychain,
        _dir: tempfile::TempDir,
        // One at a time: the prompts switch is process-wide.
        _serial: std::sync::MutexGuard<'static, ()>,
    }

    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn interaction_allowed() -> bool {
        let mut on = 0u8;
        unsafe { SecKeychainGetUserInteractionAllowed(&mut on) };
        on != 0
    }

    impl TestKeychain {
        fn new() -> Self {
            use security_framework::os::macos::keychain::CreateOptions;
            let serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
            let dir = tempfile::tempdir().unwrap();
            let mut kc = CreateOptions::new()
                .password("test")
                .prompt_user(false)
                .create(dir.path().join("test.keychain"))
                .unwrap();
            kc.unlock(Some("test")).unwrap();
            Self {
                kc,
                _dir: dir,
                _serial: serial,
            }
        }

        fn at(&self) -> Keychain<'_> {
            Keychain::Given(&self.kc)
        }

        fn comment(&self, account: &str) -> Option<String> {
            comment(&find_item(self.at(), SERVICE, account).unwrap().unwrap())
        }
    }

    impl Drop for TestKeychain {
        fn drop(&mut self) {
            unsafe { SecKeychainDelete(self.kc.as_concrete_TypeRef() as SecRef) };
        }
    }

    const SERVICE: &str = "test.cua.invalid";

    fn all(_: &str) -> bool {
        true
    }

    #[test]
    fn quiet_nests_and_restores_the_prompt_switch() {
        let _t = TestKeychain::new();
        let before = interaction_allowed();
        {
            let _outer = Quiet::new();
            assert!(!interaction_allowed());
            {
                let _inner = Quiet::new();
            }
            // The inner one never turns prompts back on early.
            assert!(!interaction_allowed());
        }
        assert_eq!(interaction_allowed(), before);
    }

    #[test]
    fn items_get_the_marker_of_their_writer_and_a_signed_reader_adopts_them() {
        let t = TestKeychain::new();
        // An unsigned writer: its marker, readable by this process.
        set_in(t.at(), SERVICE, "cua-cli", b"one", false).unwrap();
        assert_eq!(t.comment("cua-cli").as_deref(), Some(ACL_MARKER_UNSIGNED));
        assert_eq!(read_secret(t.at(), SERVICE, "cua-cli").unwrap().as_deref(), Some(&b"one"[..]));
        // An unsigned reader changes nothing.
        assert!(!adopt(t.at(), SERVICE, "cua-cli", b"one", false, true).unwrap());
        assert_eq!(
            check_items(t.at(), SERVICE, &all, false, false),
            vec![("cua-cli".into(), ItemState::Ready { recreated: false })]
        );
        // A Cua-signed reader recreates it with the Cua list, keeping the secret.
        assert_eq!(
            check_items(t.at(), SERVICE, &all, false, true),
            vec![("cua-cli".into(), ItemState::Ready { recreated: true })]
        );
        assert_eq!(t.comment("cua-cli").as_deref(), Some(ACL_MARKER));
        assert_eq!(read_secret(t.at(), SERVICE, "cua-cli").unwrap().as_deref(), Some(&b"one"[..]));
        // Once: the next signed read keeps it.
        assert!(!adopt(t.at(), SERVICE, "cua-cli", b"one", true, true).unwrap());
        // A signed write updates it in place; an unsigned one too (it never
        // downgrades the signed list).
        set_in(t.at(), SERVICE, "cua-cli", b"two", true).unwrap();
        set_in(t.at(), SERVICE, "cua-cli", b"three", false).unwrap();
        assert_eq!(t.comment("cua-cli").as_deref(), Some(ACL_MARKER));
        assert_eq!(read_secret(t.at(), SERVICE, "cua-cli").unwrap().as_deref(), Some(&b"three"[..]));
    }

    #[test]
    fn an_item_with_the_default_list_is_recreated_by_a_signed_writer() {
        let t = TestKeychain::new();
        // As an older build wrote it: the default list, no comment.
        create_with(t.at(), SERVICE, "cua-cli.device-key", b"key", None).unwrap();
        assert_eq!(t.comment("cua-cli.device-key"), None);
        set_in(t.at(), SERVICE, "cua-cli.device-key", b"key2", true).unwrap();
        assert_eq!(t.comment("cua-cli.device-key").as_deref(), Some(ACL_MARKER));
        assert_eq!(
            read_secret(t.at(), SERVICE, "cua-cli.device-key").unwrap().as_deref(),
            Some(&b"key2"[..])
        );
    }

    #[test]
    fn the_check_lists_only_the_wanted_accounts_and_none_is_ready() {
        let t = TestKeychain::new();
        assert!(check_items(t.at(), SERVICE, &all, false, true).is_empty());
        set_in(t.at(), SERVICE, "cua-cli", b"s", false).unwrap();
        set_in(t.at(), SERVICE, "other", b"o", false).unwrap();
        let wanted = |a: &str| a.starts_with("cua-cli");
        let found = check_items(t.at(), SERVICE, &wanted, false, false);
        assert_eq!(found, vec![("cua-cli".into(), ItemState::Ready { recreated: false })]);
    }

    /// After the one consented read by the bundle's `cua` (the reader), the
    /// item is recreated trusting the bundle's other executable too (the
    /// app, a different binary), so the app's own read never asks again.
    #[test]
    fn a_signed_cua_reader_recreates_the_item_for_the_apps_executable_too() {
        let t = TestKeychain::new();
        // A bundle whose executables are real signed binaries (copies of
        // Apple's), so each has a designated requirement.
        let dir = tempfile::tempdir().unwrap();
        let macos = dir.path().join("Cua Spaces.app/Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        std::fs::copy("/usr/bin/true", macos.join("cua")).unwrap();
        std::fs::copy("/usr/bin/false", macos.join("CuaSpacesMac")).unwrap();
        let app = macos.join("CuaSpacesMac").canonicalize().unwrap();
        // As an older build left it: the default list (only its creator,
        // here this process, as after the user chose Always Allow), no comment.
        create_with(t.at(), SERVICE, "cua-cli", b"session", None).unwrap();
        assert!(!trusted_paths(&t, "cua-cli").iter().any(|p| p == app.to_str().unwrap()));
        // The reader is the bundle's `cua`.
        AS_EXE.with(|e| *e.borrow_mut() = Some(macos.join("cua")));
        let found = check_items(t.at(), SERVICE, &all, false, true);
        AS_EXE.with(|e| *e.borrow_mut() = None);
        assert_eq!(found, vec![("cua-cli".into(), ItemState::Ready { recreated: true })]);
        assert_eq!(t.comment("cua-cli").as_deref(), Some(ACL_MARKER));
        let trusted = trusted_paths(&t, "cua-cli");
        assert!(trusted.iter().any(|p| p == app.to_str().unwrap()), "{trusted:?}");
        // The reader itself stays trusted, and the secret is unchanged.
        let me = std::env::current_exe().unwrap().canonicalize().unwrap();
        assert!(trusted.iter().any(|p| p == me.to_str().unwrap()), "{trusted:?}");
        assert_eq!(read_secret(t.at(), SERVICE, "cua-cli").unwrap().as_deref(), Some(&b"session"[..]));
        // Once: the next read keeps it.
        assert_eq!(
            check_items(t.at(), SERVICE, &all, false, true),
            vec![("cua-cli".into(), ItemState::Ready { recreated: false })]
        );
    }

    /// A recreate never leaves a moment without the secret, and leaves no
    /// staged copy behind; a staged copy left by an interrupted one is
    /// renamed back.
    #[test]
    fn recreating_writes_the_new_item_before_deleting_the_old_one() {
        let t = TestKeychain::new();
        create_with(t.at(), SERVICE, "cua-cli", b"s1", None).unwrap();
        let item = find_item(t.at(), SERVICE, "cua-cli").unwrap().unwrap();
        assert!(recreate(t.at(), item, SERVICE, "cua-cli", b"s1", true).unwrap());
        assert_eq!(read_secret(t.at(), SERVICE, "cua-cli").unwrap().as_deref(), Some(&b"s1"[..]));
        assert_eq!(t.comment("cua-cli").as_deref(), Some(ACL_MARKER));
        assert!(find_item(t.at(), SERVICE, "cua-cli~staging").unwrap().is_none());
        // Interrupted between the delete and the rename: only the staged copy.
        let item = find_item(t.at(), SERVICE, "cua-cli").unwrap().unwrap();
        create(t.at(), SERVICE, "cua-cli~staging", b"s1", true).unwrap();
        unsafe { SecKeychainItemDelete(item.0) };
        // It is not a Cua item of its own.
        assert!(check_items(t.at(), SERVICE, &all, false, true).is_empty());
        assert!(recover_staged(t.at(), SERVICE, "cua-cli"));
        assert_eq!(read_secret(t.at(), SERVICE, "cua-cli").unwrap().as_deref(), Some(&b"s1"[..]));
        assert!(!recover_staged(t.at(), SERVICE, "cua-cli"));
    }

    /// An item this process is not trusted by (another build wrote it):
    /// with interaction disabled the check says so instead of prompting.
    #[test]
    fn an_untrusted_item_needs_access_without_a_prompt() {
        let t = TestKeychain::new();
        let c = CString::new("/usr/bin/true").unwrap();
        let mut other: SecRef = ptr::null_mut();
        check(unsafe { SecTrustedApplicationCreateFromPath(c.as_ptr(), &mut other) }).unwrap();
        let other = Owned(other);
        let values = [other.0 as CFTypeRef];
        let list = Owned(unsafe {
            CFArrayCreate(ptr::null(), values.as_ptr(), 1, &kCFTypeArrayCallBacks)
        } as SecRef);
        let desc = CString::new(SERVICE).unwrap();
        let desc =
            Owned(unsafe { CFStringCreateWithCString(ptr::null(), desc.as_ptr(), UTF8) } as SecRef);
        let mut access: SecRef = ptr::null_mut();
        check(unsafe { SecAccessCreate(desc.0 as CFStringRef, list.0 as CFArrayRef, &mut access) })
            .unwrap();
        let access = Owned(access);
        create_with(t.at(), SERVICE, "cua-cli", b"s", Some((&access, "elsewhere"))).unwrap();
        assert_eq!(
            check_items(t.at(), SERVICE, &all, false, true),
            vec![("cua-cli".into(), ItemState::NeedsAccess)]
        );
        // Nothing changed.
        assert_eq!(t.comment("cua-cli").as_deref(), Some("elsewhere"));
    }
}
