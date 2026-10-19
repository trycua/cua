// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Fixtures for the UX core's tests: throwaway app bundles and `.desktop`
//! entries under temp directories, and provider views built from the real
//! registry on a fake host.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::catalog::ProviderView;

/// Writes `<root>/<name>.app` with an Info.plist (binary when `binary`).
pub fn fixture_app(root: &Path, name: &str, bundle_id: &str, short: &str, binary: bool) -> PathBuf {
    let bundle = root.join(format!("{name}.app"));
    std::fs::create_dir_all(bundle.join("Contents/Resources")).unwrap();
    let mut d = plist::Dictionary::new();
    d.insert("CFBundleIdentifier".into(), bundle_id.into());
    d.insert("CFBundleName".into(), short.into());
    d.insert("CFBundleShortVersionString".into(), "1.0.0".into());
    d.insert("CFBundleIconFile".into(), "app".into());
    let v = plist::Value::Dictionary(d);
    let path = bundle.join("Contents/Info.plist");
    if binary {
        v.to_file_binary(&path).unwrap();
    } else {
        v.to_file_xml(&path).unwrap();
    }
    bundle
}

/// Writes `<root>/<id>.desktop`.
pub fn fixture_desktop(root: &Path, id: &str, name: &str, exec: &str, extra: &str) -> PathBuf {
    let p = root.join(format!("{id}.desktop"));
    std::fs::write(
        &p,
        format!("[Desktop Entry]\nType=Application\nName={name}\nExec={exec}\nIcon={id}\n{extra}"),
    )
    .unwrap();
    p
}

/// Writes an installed app the way this OS lists apps: a `<name>.app`
/// bundle on macOS, a `<desktop_id>.desktop` entry on Linux, a
/// `<name>.lnk` Start Menu shortcut on Windows.
pub fn fixture_host_app(root: &Path, name: &str, bundle_id: &str, desktop_id: &str) -> PathBuf {
    if cfg!(target_os = "macos") {
        fixture_app(root, name, bundle_id, name, false)
    } else if cfg!(target_os = "windows") {
        let p = root.join(format!("{name}.lnk"));
        std::fs::write(&p, b"").unwrap();
        p
    } else {
        fixture_desktop(root, desktop_id, name, desktop_id, "")
    }
}

/// Asserts `manifest`'s opt-ins match [`super::catalog::OPT_IN_ITEMS`]:
/// every sensitive item left out by default has a group, no other item has
/// one, and the groups seen are exactly the ones the catalog offers.
pub fn assert_opt_ins_match(provider_id: &str, manifest: &crate::TransferManifest) {
    use super::catalog::SensitiveGroup;
    let mut seen = Vec::new();
    for i in &manifest.items {
        let group = SensitiveGroup::of_item(provider_id, &i.rel_path);
        if i.sensitive && !i.default_checked {
            let g = group.unwrap_or_else(|| panic!("{} has no opt-in group", i.rel_path));
            if !seen.contains(&g) {
                seen.push(g);
            }
        } else {
            assert_eq!(
                group, None,
                "{} is in a group but sensitive={} default_checked={}",
                i.rel_path, i.sensitive, i.default_checked
            );
        }
    }
    seen.sort_by_key(|g| SensitiveGroup::ALL.iter().position(|a| a == g));
    assert_eq!(seen, SensitiveGroup::offered_by(provider_id));
}

/// Every built-in provider on a fake host, none installed.
pub fn providers() -> Vec<ProviderView> {
    crate::ExportRegistry::with_builtin_host(Arc::new(crate::FakeHost::new()))
        .infos()
        .into_iter()
        .map(|info| ProviderView {
            info,
            installed_here: false,
        })
        .collect()
}
