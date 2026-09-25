use super::contains_remote_debugging_flag;

#[test]
fn rejects_all_chromium_remote_debugging_spellings() {
    assert!(contains_remote_debugging_flag("--remote-debugging-port=0"));
    assert!(contains_remote_debugging_flag("--REMOTE-DEBUGGING-PIPE"));
    assert!(contains_remote_debugging_flag(
        "/usr/bin/chrome --remote-debugging-port 9222"
    ));
    assert!(!contains_remote_debugging_flag(
        "--user-data-dir=/tmp/profile"
    ));
}
