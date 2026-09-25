use super::resolve_max_image_dimension;

#[test]
fn canonical_override_wins_and_zero_requests_native_resolution() {
    assert_eq!(resolve_max_image_dimension(1568, None, None), 1568);
    assert_eq!(resolve_max_image_dimension(1568, Some(800), None), 800);
    assert_eq!(resolve_max_image_dimension(800, Some(1568), None), 1568);
    assert_eq!(resolve_max_image_dimension(1568, Some(0), None), 0);
    assert_eq!(
        resolve_max_image_dimension(1568, Some(1200), Some(400)),
        1200
    );
}

#[test]
fn omitted_canonical_override_preserves_legacy_folding() {
    assert_eq!(resolve_max_image_dimension(1568, None, Some(800)), 800);
    assert_eq!(resolve_max_image_dimension(800, None, Some(1568)), 800);
    assert_eq!(resolve_max_image_dimension(0, None, Some(800)), 800);
}
