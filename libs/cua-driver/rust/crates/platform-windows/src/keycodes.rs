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

#[cfg(test)]
mod tests {
    use super::*;

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
