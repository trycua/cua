/// Return the Windows virtual-key code for an ASCII letter or digit.
///
/// Windows defines these codes as the corresponding uppercase ASCII value,
/// independent of the active keyboard layout.
pub(crate) fn ascii_alphanumeric_virtual_key_code(key: &str) -> Option<u16> {
    let [byte] = key.as_bytes() else {
        return None;
    };
    match byte {
        b'a'..=b'z' => Some((byte - b'a' + b'A') as u16),
        b'A'..=b'Z' | b'0'..=b'9' => Some(*byte as u16),
        _ => None,
    }
}

/// The virtual key that types a character unshifted on the target's layout,
/// given that character's `VkKeyScanExW` result on that layout.
///
/// `None` means the key cannot be sent as a plain virtual key: the layout has
/// no key for the character (-1, whose shift byte is also set), it needs a
/// shift state, or it is only on the numpad, which types a navigation key
/// when NumLock is off.
pub(crate) fn unshifted_layout_key(vk_scan: i16) -> Option<u16> {
    let [vk, shift_state] = vk_scan.to_le_bytes();
    (shift_state == 0 && !(0x60..=0x6F).contains(&vk)).then_some(vk as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unshifted_key_on_the_layout_is_sent_as_that_key() {
        // US: 'a' is VK_A, '/' is VK_OEM_2; F1 sits just past the numpad range.
        assert_eq!(unshifted_layout_key(0x0041), Some(0x41));
        assert_eq!(unshifted_layout_key(0x00BF), Some(0xBF));
        assert_eq!(unshifted_layout_key(0x0070), Some(0x70));
    }

    #[test]
    fn character_the_layout_cannot_type_unshifted_has_no_key() {
        // Thai Kedmanee has no key for 'a'.
        assert_eq!(unshifted_layout_key(-1), None);
        // AZERTY types '3' with Shift+VK_3; US types 'A' with Shift+VK_A.
        assert_eq!(unshifted_layout_key(0x0133), None);
        assert_eq!(unshifted_layout_key(0x0141), None);
        // Thai Kedmanee only has '3' on the numpad (VK_NUMPAD3); the whole
        // numpad range through VK_DIVIDE is excluded.
        assert_eq!(unshifted_layout_key(0x0063), None);
        assert_eq!(unshifted_layout_key(0x0060), None);
        assert_eq!(unshifted_layout_key(0x006F), None);
    }

    #[test]
    fn maps_ascii_letters_to_uppercase_virtual_key_codes() {
        for letter in b'a'..=b'z' {
            let lowercase = char::from(letter).to_string();
            let uppercase = char::from(letter.to_ascii_uppercase()).to_string();
            let expected = (letter - b'a' + b'A') as u16;

            assert_eq!(
                ascii_alphanumeric_virtual_key_code(&lowercase),
                Some(expected)
            );
            assert_eq!(
                ascii_alphanumeric_virtual_key_code(&uppercase),
                Some(expected)
            );
        }
    }

    #[test]
    fn maps_ascii_digits_to_matching_virtual_key_codes() {
        for digit in b'0'..=b'9' {
            let key = char::from(digit).to_string();
            assert_eq!(
                ascii_alphanumeric_virtual_key_code(&key),
                Some(digit as u16)
            );
        }
    }

    #[test]
    fn ignores_non_alphanumeric_or_multi_character_names() {
        for key in ["", " ", "/", "return", "é"] {
            assert_eq!(ascii_alphanumeric_virtual_key_code(key), None);
        }
    }
}
