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
}

const ERR_SEC_SUCCESS: OSStatus = 0;
const ERR_SEC_ITEM_NOT_FOUND: OSStatus = -25300;
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

/// The comment of an item written with the Cua access list.
pub(crate) const ACL_MARKER: &str = "cua-acl/v2";

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

/// An access object trusting this process and its companion executables.
fn cua_access(descriptor: &str) -> Result<Owned, Status> {
    let mut apps: Vec<Owned> = Vec::new();
    let mut me: SecRef = ptr::null_mut();
    // NULL: the calling application.
    check(unsafe { SecTrustedApplicationCreateFromPath(ptr::null(), &mut me) })?;
    apps.push(Owned(me));
    let exe = std::env::current_exe().unwrap_or_default();
    for path in companion_executables(&exe, Path::new(INSTALLED_APP)) {
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

/// The item, without reading its secret (no prompt).
fn find_item(kc: &Owned, service: &str, account: &str) -> Result<Option<Owned>, Status> {
    let mut item: SecRef = ptr::null_mut();
    let status = unsafe {
        SecKeychainFindGenericPassword(
            kc.0 as CFTypeRef,
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

fn create(kc: &Owned, service: &str, account: &str, secret: &[u8]) -> Result<(), Status> {
    let access = cua_access(service)?;
    let attr = |tag: u32, v: &str| SecKeychainAttribute {
        tag,
        length: v.len() as u32,
        data: v.as_ptr() as *mut c_void,
    };
    let mut attrs = [
        attr(ATTR_SERVICE, service),
        attr(ATTR_ACCOUNT, account),
        attr(ATTR_LABEL, service),
        attr(ATTR_COMMENT, ACL_MARKER),
    ];
    let mut list = SecKeychainAttributeList {
        count: attrs.len() as u32,
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
            access.0,
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
    let kc = default_keychain()?;
    match find_item(&kc, service, account)? {
        None => create(&kc, service, account, secret),
        Some(item) if comment(&item).as_deref() == Some(ACL_MARKER) => modify(&item, secret),
        Some(item) => {
            // Written by an older build with the default access list:
            // recreate it with the Cua one. If it cannot be deleted (not
            // ours to delete), update it in place as before.
            if unsafe { SecKeychainItemDelete(item.0) } == ERR_SEC_SUCCESS {
                drop(item);
                create(&kc, service, account, secret)
            } else {
                modify(&item, secret)
            }
        }
    }
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
}
