use super::exact_window_ownership_result;

#[test]
fn explicit_pid_hwnd_guard_refuses_wrong_or_stale_owners() {
    assert!(exact_window_ownership_result(42, 7, Some(42)).is_ok());

    let wrong = exact_window_ownership_result(42, 7, Some(99)).unwrap_err();
    assert_eq!(wrong.is_error, Some(true));
    assert_eq!(
        wrong.structured_content.as_ref().unwrap()["code"],
        "window_target_mismatch"
    );
    assert_eq!(wrong.structured_content.as_ref().unwrap()["owner_pid"], 99);

    let stale = exact_window_ownership_result(42, 7, None).unwrap_err();
    assert_eq!(
        stale.structured_content.as_ref().unwrap()["code"],
        "window_target_not_found"
    );
}
