use super::{inject_chromium_anti_throttling_flags, is_chromium_browser_target};

#[test]
fn detects_bare_browser_names() {
    for name in [
        "msedge", "chrome", "brave", "opera", "vivaldi", "chromium", "thorium", "iridium",
        "browser", "arc",
    ] {
        assert!(is_chromium_browser_target(name), "{name} should match");
        assert!(
            is_chromium_browser_target(&format!("{name}.exe")),
            "{name}.exe should match"
        );
        // Case-insensitive.
        assert!(
            is_chromium_browser_target(&name.to_uppercase()),
            "uppercase {name} should match"
        );
    }
}

#[test]
fn detects_full_paths() {
    assert!(is_chromium_browser_target(
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
    ));
    assert!(is_chromium_browser_target(
        r"C:\Program Files\Google\Chrome\Application\chrome.exe"
    ));
    // Forward slashes too (some shells write paths that way).
    assert!(is_chromium_browser_target(
        r"C:/Program Files/Google/Chrome/Application/chrome.exe"
    ));
}

#[test]
fn detects_launch_path_with_trailing_args() {
    // Round-tripped launch_path from list_apps with shortcut arguments.
    assert!(is_chromium_browser_target(
        r#""C:\Program Files\Google\Chrome\Application\chrome.exe" --profile-directory="Profile 2""#
    ));
}

#[test]
fn does_not_match_non_chromium_apps() {
    for name in ["firefox", "notepad", "explorer", "code", "soffice"] {
        assert!(!is_chromium_browser_target(name), "{name} should NOT match");
        assert!(
            !is_chromium_browser_target(&format!("{name}.exe")),
            "{name}.exe should NOT match"
        );
    }
    // Empty target.
    assert!(!is_chromium_browser_target(""));
}

#[test]
fn injects_three_flags_into_empty_args() {
    let mut args: Vec<String> = vec![];
    inject_chromium_anti_throttling_flags(&mut args);
    assert!(args.contains(&"--disable-features=CalculateNativeWinOcclusion".to_string()));
    assert!(args.contains(&"--disable-backgrounding-occluded-windows".to_string()));
    assert!(args.contains(&"--disable-renderer-backgrounding".to_string()));
    assert_eq!(args.len(), 3);
}

#[test]
fn merges_into_existing_disable_features_list() {
    let mut args = vec!["--disable-features=Foo,Bar".to_string()];
    inject_chromium_anti_throttling_flags(&mut args);
    // Should NOT have two --disable-features= entries.
    let dfe: Vec<_> = args
        .iter()
        .filter(|a| a.starts_with("--disable-features="))
        .collect();
    assert_eq!(dfe.len(), 1);
    assert!(dfe[0].contains("CalculateNativeWinOcclusion"));
    assert!(dfe[0].contains("Foo"));
    assert!(dfe[0].contains("Bar"));
}

#[test]
fn idempotent_when_all_flags_already_present() {
    let mut args = vec![
        "--disable-features=CalculateNativeWinOcclusion".to_string(),
        "--disable-backgrounding-occluded-windows".to_string(),
        "--disable-renderer-backgrounding".to_string(),
    ];
    let before = args.clone();
    inject_chromium_anti_throttling_flags(&mut args);
    assert_eq!(args, before, "must not duplicate flags");
}

#[test]
fn preserves_user_url_argument_after_flags() {
    let mut args = vec!["file:///C:/test_page.html".to_string()];
    inject_chromium_anti_throttling_flags(&mut args);
    // URL must still be present.
    assert!(args.iter().any(|a| a == "file:///C:/test_page.html"));
    // All three flags now in args.
    assert!(args
        .iter()
        .any(|a| a == "--disable-features=CalculateNativeWinOcclusion"));
    assert!(args
        .iter()
        .any(|a| a == "--disable-backgrounding-occluded-windows"));
    assert!(args.iter().any(|a| a == "--disable-renderer-backgrounding"));
}
