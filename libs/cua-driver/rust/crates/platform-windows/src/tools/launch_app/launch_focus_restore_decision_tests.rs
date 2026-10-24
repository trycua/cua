use super::{
    first_unopened_shell_url_index, should_restore_foreground_after_launch, LaunchTargetShape,
};

#[test]
fn only_app_identifying_launches_restore_foreground() {
    for (fields, restore) in [
        // A urls-only launch explicitly asks for navigation in the default
        // browser; that browser is the legitimate foreground.
        (&["urls"][..], false),
        (&["name"], true),
        (&["path"], true),
        // The AUMID path restores synchronously in launch_uwp.rs; the
        // caller gates the polling restore on `aumid_for_uwp.is_none()`.
        (&["aumid"], true),
        (&["bundle_id"], true),
        (&["launch_path"], true),
        // An app-identifying field alongside urls opens them in that app
        // in the background, so the prior foreground is restored.
        (&["name", "urls"], true),
        (&["path", "urls"], true),
        (&["aumid", "urls"], true),
        // Nothing was launched.
        (&[], false),
    ] {
        let shape = LaunchTargetShape {
            has_aumid: fields.contains(&"aumid"),
            has_bundle_id: fields.contains(&"bundle_id"),
            has_name: fields.contains(&"name"),
            has_path: fields.contains(&"path"),
            has_launch_path: fields.contains(&"launch_path"),
            has_urls: fields.contains(&"urls"),
        };
        assert_eq!(
            should_restore_foreground_after_launch(shape),
            restore,
            "{fields:?}"
        );
    }
}

#[test]
fn named_app_launch_forwards_every_url() {
    assert_eq!(first_unopened_shell_url_index(true), 0);
}

#[test]
fn urls_only_launch_does_not_reopen_primary_url() {
    assert_eq!(first_unopened_shell_url_index(false), 1);
}
