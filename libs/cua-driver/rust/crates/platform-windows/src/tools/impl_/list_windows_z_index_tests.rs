
use super::{exact_window_ownership_result, z_index_from_front_to_back};

#[test]
fn enum_windows_front_to_back_order_normalizes_to_higher_is_frontmost() {
    let indices: Vec<_> = (0..3)
        .map(|position| z_index_from_front_to_back(3, position))
        .collect();
    assert_eq!(indices, vec![2, 1, 0]);
    assert!(indices[0] > indices[2]);
}

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
