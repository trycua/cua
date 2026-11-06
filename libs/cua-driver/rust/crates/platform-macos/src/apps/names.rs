// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Find an installed `.app` bundle by any name it ships.

use super::{plist_string, read_bundle_plist};
use core_foundation::{
    array::{CFArray, CFArrayRef},
    base::{CFType, TCFType},
    dictionary::CFDictionary,
    string::CFString,
};
use cua_driver_core::app_names::{self, AppName, NameMiss, NamedApp};
use std::path::{Path, PathBuf};

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

/// The bundle in `roots` that `query` names in any language it ships, by an
/// exact name or the start of one; several equally good bundles are refused.
pub(crate) fn find(
    query: &str,
    roots: &[PathBuf],
    languages: &[String],
) -> Result<PathBuf, NameMiss> {
    let bundles = merge(scan(roots));
    let apps: Vec<NamedApp> = bundles.iter().map(|bundle| bundle.app.clone()).collect();
    let query = query.trim().trim_end_matches(".app");
    app_names::lookup(query, &apps, languages).map(|index| bundles[index].path.clone())
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

/// The `.app` bundles inside each root and one folder down (Adobe's
/// `Adobe InDesign 2026/Adobe InDesign 2026.app`), in root order.
fn scan(roots: &[PathBuf]) -> Vec<Bundle> {
    let mut bundles = Vec::new();
    for root in roots {
        let (apps, folders): (Vec<PathBuf>, Vec<PathBuf>) =
            sorted_dirs(root).into_iter().partition(|path| is_app(path));
        let nested = folders
            .iter()
            .flat_map(|folder| sorted_dirs(folder).into_iter().filter(|path| is_app(path)));
        bundles.extend(apps.into_iter().chain(nested).map(load_bundle));
    }
    bundles
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
        app: NamedApp { title, names },
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
    use super::find;
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
}
