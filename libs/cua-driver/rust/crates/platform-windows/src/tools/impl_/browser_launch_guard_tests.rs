use super::contains_remote_debugging_flag;

#[test]
fn rejects_all_chromium_remote_debugging_spellings() {
    assert!(contains_remote_debugging_flag("--remote-debugging-port=0"));
    assert!(contains_remote_debugging_flag("--REMOTE-DEBUGGING-PIPE"));
    assert!(contains_remote_debugging_flag(
        r#"C:\Program Files\Chrome\chrome.exe --remote-debugging-port 9222"#
    ));
    assert!(!contains_remote_debugging_flag(
        r#"--user-data-dir=C:\Temp\profile"#
    ));
}
