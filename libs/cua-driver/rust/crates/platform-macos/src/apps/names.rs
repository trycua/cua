// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Find an installed `.app` bundle by any name it ships, wherever it is.
//!
//! The candidates are every app Spotlight knows on this Mac, plus the
//! application folders and the system's own apps walked on disk, so a
//! missing or slow Spotlight index still finds the usual places.

use super::{plist_string, read_bundle_plist};
use core_foundation::{
    array::{CFArray, CFArrayRef},
    base::{CFType, TCFType},
    dictionary::CFDictionary,
    string::CFString,
};
use cua_driver_core::app_names::{self, AppName, NameMiss, NamedApp};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long Spotlight may take to list the installed apps before the walk on
/// disk is used alone.
const SPOTLIGHT_WAIT: Duration = Duration::from_secs(2);

/// How deep the walk goes below each application folder: vendors nest their
/// apps in a folder of their own, sometimes two.
const WALK_DEPTH: usize = 4;

/// The system's own apps (Finder, Keychain Access, Archive Utility), outside
/// the application folders.
const SYSTEM_APPS: &str = "/System/Library/CoreServices";

/// The metadata keys that hold a bundle's shown name, display name first.
const NAME_KEYS: [&str; 2] = ["CFBundleDisplayName", "CFBundleName"];

/// The language each pre-ISO `.lproj` folder name stands for.
const LEGACY_LANGUAGES: [(&str, &str); 19] = [
    ("English", "en"),
    ("French", "fr"),
    ("German", "de"),
    ("Japanese", "ja"),
    ("Spanish", "es"),
    ("Italian", "it"),
    ("Dutch", "nl"),
    ("Swedish", "sv"),
    ("Danish", "da"),
    ("Norwegian", "no"),
    ("Finnish", "fi"),
    ("Portuguese", "pt"),
    ("Korean", "ko"),
    ("Russian", "ru"),
    ("Polish", "pl"),
    ("Turkish", "tr"),
    ("Greek", "el"),
    ("Hebrew", "he"),
    ("Arabic", "ar"),
];

struct Bundle {
    path: PathBuf,
    bundle_id: Option<String>,
    app: NamedApp,
}

/// The installed bundle `query` names in any language it ships, by an exact
/// name or the start of one. Of bundles a name fits equally well, the one in
/// the best place wins (`roots` first, then the system's apps, then anywhere
/// else); several in that place are refused.
pub(crate) fn find(
    query: &str,
    roots: &[PathBuf],
    languages: &[String],
) -> Result<PathBuf, NameMiss> {
    let mut paths = spotlight_apps(SPOTLIGHT_WAIT);
    paths.extend(walk_all(roots));
    find_in(query, paths, roots, languages)
}

/// `find` over the given bundle paths.
fn find_in(
    query: &str,
    paths: Vec<PathBuf>,
    roots: &[PathBuf],
    languages: &[String],
) -> Result<PathBuf, NameMiss> {
    let mut paths: Vec<PathBuf> = paths
        .into_iter()
        .filter(|path| is_candidate(path))
        .collect();
    // Best place first, then the roots in their order (system before user),
    // so the copy `merge` keeps is the one the Mac itself would prefer.
    let root_index = |path: &Path| roots.iter().position(|root| path.starts_with(root));
    paths.sort_by(|a, b| {
        (place(a, roots), root_index(a), a).cmp(&(place(b, roots), root_index(b), b))
    });
    paths.dedup();
    let bundles = merge(
        paths
            .into_iter()
            .map(|path| {
                let place = place(&path, roots);
                let mut bundle = load_bundle(path);
                bundle.app.place = place;
                bundle
            })
            .collect(),
    );
    let apps: Vec<NamedApp> = bundles.iter().map(|bundle| bundle.app.clone()).collect();
    let query = query.trim().trim_end_matches(".app");
    app_names::lookup(query, &apps, languages).map(|index| bundles[index].path.clone())
}

/// Where a bundle is installed: 0 in the application folders, 1 among the
/// system's own apps, 2 anywhere else (a build folder, a vendor's support
/// folder).
fn place(path: &Path, roots: &[PathBuf]) -> u8 {
    if roots.iter().any(|root| path.starts_with(root)) {
        0
    } else if path.starts_with(SYSTEM_APPS) {
        1
    } else {
        2
    }
}

/// An app someone can launch by name: not a helper inside another bundle,
/// not in the Trash.
fn is_candidate(path: &Path) -> bool {
    let inside_bundle = path
        .parent()
        .is_some_and(|parent| parent.ancestors().any(is_app));
    let in_trash = path.components().any(|part| part.as_os_str() == ".Trash");
    is_app(path) && !inside_bundle && !in_trash
}

/// Every app Spotlight has indexed on this Mac, or none when it doesn't
/// answer within `wait` (indexing off, or busy).
fn spotlight_apps(wait: Duration) -> Vec<PathBuf> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(query_spotlight());
    });
    receiver.recv_timeout(wait).unwrap_or_default()
}

fn query_spotlight() -> Vec<PathBuf> {
    use core_foundation::base::{kCFAllocatorDefault, CFAllocatorRef, CFRelease, CFTypeRef};
    use core_foundation::string::CFStringRef;
    use std::ffi::c_void;

    #[link(name = "CoreServices", kind = "framework")]
    extern "C" {
        static kMDItemPath: CFStringRef;
        fn MDQueryCreate(
            allocator: CFAllocatorRef,
            query: CFStringRef,
            value_list_attrs: CFArrayRef,
            sorting_attrs: CFArrayRef,
        ) -> *mut c_void;
        fn MDQueryExecute(query: *mut c_void, options: usize) -> u8;
        fn MDQueryGetResultCount(query: *mut c_void) -> isize;
        fn MDQueryGetResultAtIndex(query: *mut c_void, index: isize) -> *const c_void;
        fn MDItemCopyAttribute(item: *const c_void, name: CFStringRef) -> CFTypeRef;
    }
    const SYNCHRONOUS: usize = 1;
    let text = CFString::new("kMDItemContentType == 'com.apple.application-bundle'");
    let mut paths = Vec::new();
    unsafe {
        let query = MDQueryCreate(
            kCFAllocatorDefault,
            text.as_concrete_TypeRef(),
            std::ptr::null(),
            std::ptr::null(),
        );
        if query.is_null() {
            return paths;
        }
        if MDQueryExecute(query, SYNCHRONOUS) != 0 {
            for index in 0..MDQueryGetResultCount(query) {
                let item = MDQueryGetResultAtIndex(query, index);
                if item.is_null() {
                    continue;
                }
                let path = MDItemCopyAttribute(item, kMDItemPath);
                if !path.is_null() {
                    let path = CFString::wrap_under_create_rule(path as CFStringRef);
                    paths.push(PathBuf::from(path.to_string()));
                }
            }
        }
        CFRelease(query as CFTypeRef);
    }
    paths
}

/// The `.app` bundles in each root at any depth up to [`WALK_DEPTH`], then
/// the system's own apps.
fn walk_all(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut paths = walk_roots(roots);
    walk(Path::new(SYSTEM_APPS), WALK_DEPTH, &mut paths);
    paths
}

/// The `.app` bundles in each root at any depth up to [`WALK_DEPTH`], in
/// root order.
fn walk_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for root in roots {
        walk(root, WALK_DEPTH, &mut paths);
    }
    paths
}

fn walk(folder: &Path, depth: usize, paths: &mut Vec<PathBuf>) {
    for path in sorted_dirs(folder) {
        if is_app(&path) {
            paths.push(path);
        } else if depth > 0 {
            walk(&path, depth - 1, paths);
        }
    }
}

/// The Mac's languages, first choice first (`fr-FR`, `en-US`).
pub(crate) fn preferred_languages() -> Vec<String> {
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFLocaleCopyPreferredLanguages() -> CFArrayRef;
    }
    let raw = unsafe { CFLocaleCopyPreferredLanguages() };
    if raw.is_null() {
        return Vec::new();
    }
    let languages = unsafe { CFArray::<CFString>::wrap_under_create_rule(raw) };
    languages
        .iter()
        .map(|language| language.to_string())
        .collect()
}

fn is_app(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "app")
}

fn sorted_dirs(folder: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs
}

/// One bundle per bundle id, at its first-seen path and holding every name
/// of its copies.
fn merge(bundles: Vec<Bundle>) -> Vec<Bundle> {
    let mut merged: Vec<Bundle> = Vec::new();
    for bundle in bundles {
        let first = bundle.bundle_id.as_ref().and_then(|id| {
            merged
                .iter_mut()
                .find(|known| known.bundle_id.as_ref() == Some(id))
        });
        match first {
            Some(first) => {
                for name in bundle.app.names {
                    if !first.app.names.contains(&name) {
                        first.app.names.push(name);
                    }
                }
            }
            None => merged.push(bundle),
        }
    }
    merged
}

fn load_bundle(path: PathBuf) -> Bundle {
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let info = read_bundle_plist(&path.join("Contents/Info.plist"));
    let own = info
        .as_ref()
        .map(|info| named(info, ""))
        .unwrap_or_default();
    let title = own
        .first()
        .map_or_else(|| stem.clone(), |name| name.text.clone());
    let mut names = vec![AppName::new(stem, "")];
    names.extend(own);
    names.extend(localized_names(&path.join("Contents/Resources")));
    Bundle {
        bundle_id: info
            .as_ref()
            .and_then(|info| plist_string(info, "CFBundleIdentifier")),
        path,
        app: NamedApp {
            title,
            names,
            place: 0,
        },
    }
}

/// Every language's shown names, from the loctable and the `.lproj` folders.
fn localized_names(resources: &Path) -> Vec<AppName> {
    let mut names = Vec::new();
    if let Some(table) = read_bundle_plist(&resources.join("InfoPlist.loctable")) {
        let (keys, values) = table.get_keys_and_values();
        for (key, value) in keys.into_iter().zip(values) {
            let key = unsafe { CFType::wrap_under_get_rule(key) };
            let value = unsafe { CFType::wrap_under_get_rule(value) };
            let (Some(language), Some(strings)) =
                (key.downcast::<CFString>(), value.downcast::<CFDictionary>())
            else {
                continue;
            };
            let language = language.to_string();
            if language != "Base" {
                names.extend(named(&strings, &language));
            }
        }
    }
    for folder in sorted_dirs(resources) {
        if folder.extension().is_none_or(|ext| ext != "lproj") {
            continue;
        }
        let stem = folder.file_stem().unwrap_or_default().to_string_lossy();
        let language = LEGACY_LANGUAGES
            .iter()
            .find(|(legacy, _)| *legacy == stem)
            .map_or(stem.as_ref(), |(_, code)| *code);
        if language == "Base" {
            continue;
        }
        if let Some(strings) = read_bundle_plist(&folder.join("InfoPlist.strings")) {
            names.extend(named(&strings, language));
        }
    }
    names
}

fn named(strings: &CFDictionary, language: &str) -> Vec<AppName> {
    NAME_KEYS
        .iter()
        .filter_map(|key| plist_string(strings, key))
        .map(|text| AppName::new(text, language))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{find_in, walk_roots};
    use cua_driver_core::app_names::NameMiss;
    use std::path::{Path, PathBuf};

    /// Write an `.app` bundle with `info` inside its Info.plist dict. Nothing
    /// runs: the lookup only reads files.
    fn bundle(at: &Path, bundle_id: &str, info: &str) -> PathBuf {
        let contents = at.join("Contents");
        std::fs::create_dir_all(contents.join("Resources")).unwrap();
        std::fs::write(
            contents.join("Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>{bundle_id}</string>
{info}
</dict></plist>"#
            ),
        )
        .unwrap();
        at.to_path_buf()
    }

    /// `find` over the bundles in `roots` only, so the Mac's own apps
    /// (through Spotlight or the system folder) can't change what a test
    /// finds.
    fn find(query: &str, roots: &[PathBuf], languages: &[String]) -> Result<PathBuf, NameMiss> {
        find_in(query, walk_roots(roots), roots, languages)
    }

    fn langs(tags: &[&str]) -> Vec<String> {
        tags.iter().map(|tag| (*tag).to_owned()).collect()
    }

    #[test]
    fn an_app_one_folder_down_is_found_by_its_name() {
        let root = tempfile::tempdir().unwrap();
        let indesign = bundle(
            &root
                .path()
                .join("Adobe InDesign 2026/Adobe InDesign 2026.app"),
            "com.adobe.InDesign",
            "",
        );
        let roots = [root.path().to_path_buf()];
        assert_eq!(
            find("Adobe InDesign 2026", &roots, &[]),
            Ok(indesign.clone())
        );
        assert_eq!(
            find("indesign", &roots, &[]).unwrap_err().suggestions,
            ["Adobe InDesign 2026"]
        );
        assert_eq!(find("adobe indesign", &roots, &[]), Ok(indesign));
    }

    #[test]
    fn the_shown_name_finds_a_bundle_whose_file_name_differs() {
        let root = tempfile::tempdir().unwrap();
        let bambu = bundle(
            &root.path().join("BambuStudio.app"),
            "com.bambulab.bambu-studio",
            "<key>CFBundleName</key><string>BambuStudio</string>",
        );
        let viewer = bundle(
            &root.path().join("PV.app"),
            "com.example.viewer",
            "<key>CFBundleDisplayName</key><string>Photo Viewer</string>",
        );
        let roots = [root.path().to_path_buf()];
        assert_eq!(find("bambustudio", &roots, &[]), Ok(bambu));
        assert_eq!(find("photo viewer", &roots, &[]), Ok(viewer));
    }

    #[test]
    fn a_localized_name_from_a_loctable_or_strings_file_finds_the_app() {
        let root = tempfile::tempdir().unwrap();
        let chess = bundle(
            &root.path().join("Chess.app"),
            "com.apple.Chess",
            "<key>CFBundleName</key><string>Chess</string>",
        );
        std::fs::write(
            chess.join("Contents/Resources/InfoPlist.loctable"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>fr</key><dict><key>CFBundleDisplayName</key><string>Échecs</string></dict>
<key>LocProvenance</key><dict><key>fr</key><integer>1</integer></dict>
</dict></plist>"#,
        )
        .unwrap();
        let mail = bundle(&root.path().join("Mail.app"), "com.apple.mail", "");
        // A legacy folder name and an old-style UTF-16 strings file.
        let german = mail.join("Contents/Resources/German.lproj");
        std::fs::create_dir_all(&german).unwrap();
        let strings: Vec<u8> = "\u{FEFF}/* names */\n\"CFBundleDisplayName\" = \"Post\";\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        std::fs::write(german.join("InfoPlist.strings"), strings).unwrap();
        let roots = [root.path().to_path_buf()];

        assert_eq!(
            find("échecs", &roots, &langs(&["en-US"])),
            Ok(chess.clone())
        );
        assert_eq!(find("Échecs", &roots, &langs(&["fr-FR"])), Ok(chess));
        assert_eq!(find("post", &roots, &langs(&["de-DE"])), Ok(mail));
    }

    #[test]
    fn two_apps_answering_to_a_name_are_refused_with_suggestions() {
        let root = tempfile::tempdir().unwrap();
        bundle(
            &root
                .path()
                .join("Adobe InDesign 2025/Adobe InDesign 2025.app"),
            "com.adobe.InDesign.2025",
            "",
        );
        bundle(
            &root
                .path()
                .join("Adobe InDesign 2026/Adobe InDesign 2026.app"),
            "com.adobe.InDesign",
            "",
        );
        let roots = [root.path().to_path_buf()];
        assert_eq!(
            find("Adobe InDesign", &roots, &[]),
            Err(NameMiss {
                ambiguous: true,
                suggestions: vec![
                    "Adobe InDesign 2025".to_owned(),
                    "Adobe InDesign 2026".to_owned()
                ],
            })
        );
    }

    #[test]
    fn copies_of_one_bundle_id_are_one_app_at_the_first_root() {
        let system = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        let first = bundle(&system.path().join("Notes.app"), "com.apple.Notes", "");
        bundle(&user.path().join("Notes copy.app"), "com.apple.Notes", "");
        let roots = [system.path().to_path_buf(), user.path().to_path_buf()];
        assert_eq!(find("notes", &roots, &[]), Ok(first));
    }

    #[test]
    fn an_app_two_folders_down_is_found() {
        let root = tempfile::tempdir().unwrap();
        let deep = bundle(
            &root
                .path()
                .join("Quillsoft Suite 2026/Extras/Quillsoft Deep.app"),
            "com.example.deep",
            "",
        );
        let roots = [root.path().to_path_buf()];
        assert_eq!(find("quillsoft deep", &roots, &[]), Ok(deep));
    }

    #[test]
    fn an_app_outside_the_folders_is_found_from_its_path() {
        let root = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let outside = bundle(
            &elsewhere.path().join("Vendor Tools/Quillsoft Outside.app"),
            "com.example.outside",
            "",
        );
        let roots = [root.path().to_path_buf()];
        assert_eq!(
            find_in("Quillsoft Outside", vec![outside.clone()], &roots, &[]),
            Ok(outside)
        );
    }

    #[test]
    fn an_installed_app_wins_over_a_build_copy_of_its_name() {
        let root = tempfile::tempdir().unwrap();
        let build = tempfile::tempdir().unwrap();
        let installed = bundle(&root.path().join("Quillmark.app"), "com.example.q", "");
        let copy = bundle(&build.path().join("Quillmark.app"), "com.example.q-dev", "");
        let roots = [root.path().to_path_buf()];
        let paths = vec![copy.clone(), installed.clone()];
        assert_eq!(find_in("Quillmark", paths, &roots, &[]), Ok(installed));
        // Alone, the build copy is still found.
        assert_eq!(
            find_in("Quillmark", vec![copy.clone()], &roots, &[]),
            Ok(copy)
        );
    }

    #[test]
    fn a_helper_inside_an_app_or_an_app_in_the_trash_is_never_launched() {
        let root = tempfile::tempdir().unwrap();
        let app = bundle(&root.path().join("Quillmark.app"), "com.example.q", "");
        let helper = bundle(
            &app.join("Contents/Helpers/Quillmark Helper.app"),
            "com.example.q.helper",
            "",
        );
        let trashed = bundle(
            &root.path().join(".Trash/Quill Gone.app"),
            "com.example.gone",
            "",
        );
        let roots = [root.path().to_path_buf()];
        assert!(find_in("Quillmark Helper", vec![helper], &roots, &[]).is_err());
        assert!(find_in("Quill Gone", vec![trashed], &roots, &[]).is_err());
    }
}
