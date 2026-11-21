// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Managed preferences that force extensions on a macOS browser.
//!
//! Chromium reads its policy from the browser's preference domain and treats
//! only forced (managed) values as mandatory. macOS forces a value only when
//! it comes from `/Library/Managed Preferences`, which a configuration
//! profile or root writes, so `CFPreferencesAppValueIsForced` is the trust
//! check: a user's own `defaults write` never counts.

use std::path::Path;

use core_foundation::array::CFArray;
use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::bundle::CFBundle;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use core_foundation::url::CFURL;
use cua_driver_core::browser::managed_policy::policy_forces_extensions;
use serde_json::Value;

const EXTENSION_POLICIES: [&str; 2] = ["ExtensionInstallForcelist", "ExtensionSettings"];

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFPreferencesCopyAppValue(key: CFStringRef, application_id: CFStringRef) -> CFTypeRef;
    fn CFPreferencesAppValueIsForced(key: CFStringRef, application_id: CFStringRef) -> u8;
}

/// The managed preference domain that forces extensions on the browser at
/// `executable`, if one does.
pub(crate) fn managed_extension_policy(executable: &str) -> Option<String> {
    let domain = policy_domain(&bundle_identifier(Path::new(executable))?);
    let application_id = CFString::new(&domain);
    let mut policy = serde_json::Map::new();
    for name in EXTENSION_POLICIES {
        let key = CFString::new(name);
        // SAFETY: both arguments are live CFStrings for the whole call.
        let forced = unsafe {
            CFPreferencesAppValueIsForced(
                key.as_concrete_TypeRef(),
                application_id.as_concrete_TypeRef(),
            )
        } != 0;
        if !forced {
            continue;
        }
        // SAFETY: as above; the result follows the Copy (create) rule.
        let raw = unsafe {
            CFPreferencesCopyAppValue(
                key.as_concrete_TypeRef(),
                application_id.as_concrete_TypeRef(),
            )
        };
        if raw.is_null() {
            continue;
        }
        let value = unsafe { CFType::wrap_under_create_rule(raw) };
        if let Some(value) = property_list_json(&value) {
            policy.insert(name.to_owned(), value);
        }
    }
    policy_forces_extensions(&Value::Object(policy))
        .then(|| format!("managed preferences domain {domain}"))
}

/// The bundle identifier of the app whose executable is `executable`.
fn bundle_identifier(executable: &Path) -> Option<String> {
    let app = executable
        .ancestors()
        .find(|path| path.extension().is_some_and(|extension| extension == "app"))?;
    let bundle = CFBundle::new(CFURL::from_path(app, true)?)?;
    bundle
        .info_dictionary()
        .find(CFString::from_static_string("CFBundleIdentifier"))?
        .downcast::<CFString>()
        .map(|identifier| identifier.to_string())
}

/// The preference domain a browser reads its policy from. It is the bundle
/// identifier, except Microsoft Edge, whose `com.microsoft.edgemac` bundles
/// read `com.microsoft.Edge` (with the same channel suffix).
fn policy_domain(bundle_identifier: &str) -> String {
    match bundle_identifier.strip_prefix("com.microsoft.edgemac") {
        Some(channel) => format!("com.microsoft.Edge{channel}"),
        None => bundle_identifier.to_owned(),
    }
}

/// A property list value as JSON: strings, booleans, numbers, arrays and
/// dictionaries with string keys. Other values (dates, data) are dropped.
fn property_list_json(value: &CFType) -> Option<Value> {
    if let Some(string) = value.downcast::<CFString>() {
        return Some(Value::String(string.to_string()));
    }
    if let Some(boolean) = value.downcast::<CFBoolean>() {
        return Some(Value::Bool(boolean.into()));
    }
    if let Some(number) = value.downcast::<CFNumber>() {
        return number
            .to_i64()
            .map(Value::from)
            .or_else(|| number.to_f64().map(Value::from));
    }
    if let Some(array) = value.downcast::<CFArray>() {
        return Some(Value::Array(
            array
                .iter()
                .filter_map(|item| {
                    // SAFETY: `item` is borrowed from `array`; get-rule retains it.
                    property_list_json(&unsafe { CFType::wrap_under_get_rule(*item) })
                })
                .collect(),
        ));
    }
    if let Some(dictionary) = value.downcast::<CFDictionary>() {
        let (keys, values) = dictionary.get_keys_and_values();
        return Some(Value::Object(
            keys.into_iter()
                .zip(values)
                .filter_map(|(key, value)| {
                    // SAFETY: both are borrowed from `dictionary`; get-rule retains them.
                    let key = unsafe { CFType::wrap_under_get_rule(key) }.downcast::<CFString>()?;
                    let value = property_list_json(&unsafe { CFType::wrap_under_get_rule(value) })?;
                    Some((key.to_string(), value))
                })
                .collect(),
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_reads_its_policy_from_the_edge_domain() {
        assert_eq!(policy_domain("com.google.Chrome"), "com.google.Chrome");
        assert_eq!(
            policy_domain("org.chromium.Chromium"),
            "org.chromium.Chromium"
        );
        assert_eq!(policy_domain("com.microsoft.edgemac"), "com.microsoft.Edge");
        assert_eq!(
            policy_domain("com.microsoft.edgemac.Beta"),
            "com.microsoft.Edge.Beta"
        );
    }

    #[test]
    fn managed_preference_values_become_policy_json() {
        let forcelist = CFArray::from_CFTypes(&[CFString::new(
            "ddkjiahejlhfcafbddmgiahcphecmpfh;https://clients2.google.com/service/update2/crx",
        )]);
        let setting = CFDictionary::from_CFType_pairs(&[
            (
                CFString::new("installation_mode").as_CFType(),
                CFString::new("force_installed").as_CFType(),
            ),
            (
                CFString::new("toolbar_pin").as_CFType(),
                CFBoolean::true_value().as_CFType(),
            ),
            (
                CFString::new("minimum_version").as_CFType(),
                CFNumber::from(3_i64).as_CFType(),
            ),
        ]);
        let settings = CFDictionary::from_CFType_pairs(&[(
            CFString::new("ddkjiahejlhfcafbddmgiahcphecmpfh").as_CFType(),
            setting.as_CFType(),
        )]);
        assert_eq!(
            property_list_json(&forcelist.as_CFType()),
            Some(serde_json::json!([
                "ddkjiahejlhfcafbddmgiahcphecmpfh;https://clients2.google.com/service/update2/crx"
            ]))
        );
        assert_eq!(
            property_list_json(&settings.as_CFType()),
            Some(serde_json::json!({
                "ddkjiahejlhfcafbddmgiahcphecmpfh": {
                    "installation_mode": "force_installed",
                    "toolbar_pin": true,
                    "minimum_version": 3
                }
            }))
        );
    }

    #[test]
    fn an_unbundled_executable_has_no_policy_domain() {
        assert_eq!(bundle_identifier(Path::new("/usr/bin/true")), None);
    }
}
