use super::*;

#[test]
fn a_file_chooser_keeping_the_basename_counts_as_committed() {
    assert!(path_normalised_to_basename(
        "/home/user/Desktop/tone.wav",
        "tone.wav"
    ));
    assert!(!path_normalised_to_basename(
        "/home/user/Desktop/tone.wav",
        "other.wav"
    ));
    assert!(!path_normalised_to_basename("tone.wav", "tone.wav"));
    let ok = set_value_result(
        3,
        "/home/user/Desktop/tone.wav",
        "ax",
        Some("tone.wav".into()),
        None,
    );
    let s = ok.structured_content.unwrap();
    assert_eq!(s["verified"], true);
    assert_eq!(s["effect"], "confirmed");
}

#[test]
fn readback_agreement_is_numeric_aware() {
    assert!(values_agree("40", "40.0"));
    assert!(values_agree("40", " 40 "));
    assert!(values_agree("abc", "abc"));
    assert!(!values_agree("40", "41"));
    assert!(!values_agree("40", "forty"));
}

#[test]
fn set_value_result_is_verified_only_on_matching_readback() {
    let ok = set_value_result(3, "40", "click_type_mpx", Some("40.0".into()), None);
    let s = ok.structured_content.unwrap();
    assert_eq!(s["verified"], true);
    assert_eq!(s["effect"], "confirmed");
    let noop = set_value_result(3, "40", "ax", Some("10.0".into()), None);
    let s = noop.structured_content.unwrap();
    assert_eq!(s["verified"], false);
    assert_eq!(s["effect"], "suspected_noop");
    assert_eq!(s["readback"], "10.0");
    let blind = set_value_result(3, "40", "ax", None, None);
    assert_eq!(blind.structured_content.unwrap()["effect"], "unverifiable");
}
