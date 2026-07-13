//! Security.framework identity checks for approved macOS applications.

use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
use core_foundation::string::{CFString, CFStringRef};
use core_foundation::url::CFURL;
use std::path::Path;

type SecurityCodeRef = *const libc::c_void;
type SecurityRequirementRef = *const libc::c_void;
type SecurityStatus = i32;
type SecurityFlags = u32;

const STRICT_STATIC_VALIDATION: SecurityFlags = (1 << 0) | (1 << 3) | (1 << 4);

#[link(name = "Security", kind = "framework")]
extern "C" {
    static kSecGuestAttributePid: core_foundation::string::CFStringRef;
    fn SecStaticCodeCreateWithPath(
        path: core_foundation::url::CFURLRef,
        flags: SecurityFlags,
        code: *mut SecurityCodeRef,
    ) -> SecurityStatus;
    fn SecStaticCodeCheckValidity(
        code: SecurityCodeRef,
        flags: SecurityFlags,
        requirement: SecurityRequirementRef,
    ) -> SecurityStatus;
    fn SecCodeCopyDesignatedRequirement(
        code: SecurityCodeRef,
        flags: SecurityFlags,
        requirement: *mut SecurityRequirementRef,
    ) -> SecurityStatus;
    fn SecRequirementCopyString(
        requirement: SecurityRequirementRef,
        flags: SecurityFlags,
        text: *mut CFStringRef,
    ) -> SecurityStatus;
    fn SecRequirementCreateWithString(
        text: CFStringRef,
        flags: SecurityFlags,
        requirement: *mut SecurityRequirementRef,
    ) -> SecurityStatus;
    fn SecCodeCopyGuestWithAttributes(
        host: SecurityCodeRef,
        attributes: core_foundation::dictionary::CFDictionaryRef,
        flags: SecurityFlags,
        guest: *mut SecurityCodeRef,
    ) -> SecurityStatus;
    fn SecCodeCheckValidity(
        code: SecurityCodeRef,
        flags: SecurityFlags,
        requirement: SecurityRequirementRef,
    ) -> SecurityStatus;
}

struct OwnedSecurityRef(SecurityCodeRef);

impl Drop for OwnedSecurityRef {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0 as CFTypeRef) };
        }
    }
}

fn static_code(path: &Path) -> Result<OwnedSecurityRef, String> {
    let url = CFURL::from_path(path, true)
        .ok_or_else(|| format!("could not construct a file URL for {}", path.display()))?;
    let mut code: SecurityCodeRef = std::ptr::null();
    let status = unsafe { SecStaticCodeCreateWithPath(url.as_concrete_TypeRef(), 0, &mut code) };
    if status == 0 && !code.is_null() {
        Ok(OwnedSecurityRef(code))
    } else {
        Err(format!(
            "Security.framework could not inspect {} (OSStatus {status})",
            path.display()
        ))
    }
}

fn compiled_requirement(text: &str) -> Result<OwnedSecurityRef, String> {
    let text = CFString::new(text);
    let mut requirement: SecurityRequirementRef = std::ptr::null();
    let status =
        unsafe { SecRequirementCreateWithString(text.as_concrete_TypeRef(), 0, &mut requirement) };
    if status == 0 && !requirement.is_null() {
        Ok(OwnedSecurityRef(requirement))
    } else {
        Err(format!(
            "Security.framework rejected the approved app requirement (OSStatus {status})"
        ))
    }
}

pub(crate) fn designated_requirement(path: &Path) -> Result<String, String> {
    let code = static_code(path)?;
    let validity =
        unsafe { SecStaticCodeCheckValidity(code.0, STRICT_STATIC_VALIDATION, std::ptr::null()) };
    if validity != 0 {
        return Err(format!(
            "the app at {} has an invalid or modified code signature (OSStatus {validity})",
            path.display()
        ));
    }
    let mut requirement: SecurityRequirementRef = std::ptr::null();
    let requirement_status =
        unsafe { SecCodeCopyDesignatedRequirement(code.0, 0, &mut requirement) };
    if requirement_status != 0 || requirement.is_null() {
        return Err(format!(
            "Security.framework could not derive the app signing identity for {} (OSStatus {requirement_status})",
            path.display()
        ));
    }
    let requirement = OwnedSecurityRef(requirement);
    let mut text: CFStringRef = std::ptr::null();
    let text_status = unsafe { SecRequirementCopyString(requirement.0, 0, &mut text) };
    if text_status != 0 || text.is_null() {
        return Err(format!(
            "Security.framework could not serialize the app signing identity for {} (OSStatus {text_status})",
            path.display()
        ));
    }
    let text = unsafe { CFString::wrap_under_create_rule(text) };
    Ok(text.to_string())
}

pub(crate) fn validate_path(path: &Path, requirement: &str) -> Result<(), String> {
    let code = static_code(path)?;
    let requirement = compiled_requirement(requirement)?;
    let status =
        unsafe { SecStaticCodeCheckValidity(code.0, STRICT_STATIC_VALIDATION, requirement.0) };
    if status == 0 {
        Ok(())
    } else {
        Err(format!(
            "the app at {} no longer satisfies its approved signing identity (OSStatus {status})",
            path.display()
        ))
    }
}

pub(crate) fn validate_pid(pid: libc::pid_t, requirement: &str) -> Result<(), String> {
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;

    let pid_key = unsafe { CFString::wrap_under_get_rule(kSecGuestAttributePid) };
    let attributes = CFDictionary::from_CFType_pairs(&[(pid_key, CFNumber::from(pid as i32))]);
    let mut code: SecurityCodeRef = std::ptr::null();
    let status = unsafe {
        SecCodeCopyGuestWithAttributes(
            std::ptr::null(),
            attributes.as_concrete_TypeRef(),
            0,
            &mut code,
        )
    };
    if status != 0 || code.is_null() {
        return Err(format!(
            "Security.framework could not inspect running app pid {pid} (OSStatus {status})"
        ));
    }
    let code = OwnedSecurityRef(code);
    let requirement = compiled_requirement(requirement)?;
    let validity = unsafe { SecCodeCheckValidity(code.0, 0, requirement.0) };
    if validity == 0 {
        Ok(())
    } else {
        Err(format!(
            "running app pid {pid} no longer satisfies its approved signing identity (OSStatus {validity})"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn unchanged_signed_app_has_a_stable_valid_requirement() {
        let path = Path::new("/System/Applications/Calculator.app");
        let requirement = designated_requirement(path).unwrap();
        assert!(!requirement.is_empty());
        validate_path(path, &requirement).unwrap();
    }

    #[test]
    fn modified_approved_executable_fails_strict_validation() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("SignedTool");
        std::fs::copy("/usr/bin/true", &executable).unwrap();
        let status = std::process::Command::new("/usr/bin/codesign")
            .args(["--force", "--sign", "-", executable.to_str().unwrap()])
            .status()
            .unwrap();
        assert!(status.success());
        let requirement = designated_requirement(&executable).unwrap();
        validate_path(&executable, &requirement).unwrap();

        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&executable)
            .unwrap();
        file.write_all(b"tampered").unwrap();
        file.sync_all().unwrap();
        assert!(validate_path(&executable, &requirement).is_err());
    }
}
