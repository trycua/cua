
use super::*;

#[test]
fn typed_text_is_confirmed_only_when_read_back() {
    let confirmed =
        type_text_readback_result(1, 3, "-40", "via targeted AT-SPI", Some("-40.0".into()));
    let s = confirmed.structured_content.clone().unwrap();
    assert_eq!(s["effect"], "confirmed");
    assert_eq!(s["verified"], true);
    assert_eq!(s["evidence"][0]["kind"], "value_readback");
    let contained = type_text_readback_result(1, 3, "ab", "r", Some("xxabyy".into()));
    assert_eq!(contained.structured_content.unwrap()["effect"], "confirmed");
    let mismatch = type_text_readback_result(1, 3, "ab", "r", Some("zz".into()));
    let s = mismatch.structured_content.unwrap();
    assert_eq!(s["effect"], "unverifiable");
    assert_eq!(s["readback"], "zz");
    let blind = type_text_readback_result(1, 3, "ab", "r", None);
    assert_eq!(blind.structured_content.unwrap()["effect"], "unverifiable");
}
