//! `cua spaces suspend` and `cua spaces resize` reach the Spaces runtime:
//! an unknown Space is reported as unknown (not refused as a command the
//! CLI cannot run). Everything runs in a temp home with no daemon.

mod common;

use common::Home;

#[tokio::test]
async fn suspend_and_resize_name_the_unknown_space() {
    let h = Home::new();
    for args in [
        vec!["--embedded", "spaces", "suspend", "local:nope"],
        vec![
            "--embedded",
            "spaces",
            "resize",
            "local:nope",
            "--memory-mb",
            "2048",
        ],
    ] {
        let o = h.run(&args).await;
        assert_ne!(o.code, 0, "{o:?}");
        assert!(!o.stderr.contains("only `cua spaces ls`"), "{o:?}");
        assert!(o.stderr.contains("nope"), "{o:?}");
    }
    // A resize says what to change.
    let o = h
        .run(&["--embedded", "spaces", "resize", "local:nope"])
        .await;
    assert_ne!(o.code, 0, "{o:?}");
}
