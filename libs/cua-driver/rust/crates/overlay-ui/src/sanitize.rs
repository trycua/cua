const MAX_LABEL_CHARS: usize = 72;
const MAX_SUMMARY_CHARS: usize = 280;

/// Strip control and bidirectional-override characters from short labels.
pub fn sanitize_label(input: &str) -> String {
    sanitize(input, MAX_LABEL_CHARS, false)
}

/// Strip controls, collapse whitespace, and bound untrusted resource text.
pub fn sanitize_summary(input: &str) -> String {
    sanitize(input, MAX_SUMMARY_CHARS, true)
}

fn sanitize(input: &str, max_chars: usize, collapse_whitespace: bool) -> String {
    let mut output = String::with_capacity(input.len().min(max_chars));
    let mut previous_space = false;
    for ch in input.chars() {
        if output.chars().count() >= max_chars {
            break;
        }
        if is_bidi_control(ch) || ch.is_control() {
            if collapse_whitespace && ch.is_whitespace() && !previous_space && !output.is_empty() {
                output.push(' ');
                previous_space = true;
            }
            continue;
        }
        if collapse_whitespace && ch.is_whitespace() {
            if !previous_space && !output.is_empty() {
                output.push(' ');
                previous_space = true;
            }
        } else {
            output.push(ch);
            previous_space = false;
        }
    }
    let output = output.trim().to_owned();
    if input.chars().count() > max_chars && !output.is_empty() {
        format!("{}…", output.trim_end_matches('…'))
    } else {
        output
    }
}

fn is_bidi_control(ch: char) -> bool {
    matches!(
        ch,
        '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_removes_controls_and_collapses_lines() {
        assert_eq!(
            sanitize_summary("Chrome\n\u{202e}evil\t profile"),
            "Chrome evil profile"
        );
    }

    #[test]
    fn label_is_bounded() {
        let label = sanitize_label(&"a".repeat(100));
        assert_eq!(label.chars().count(), 73);
        assert!(label.ends_with('…'));
    }
}
