//! The one place a run's application metadata is smuggled through the prompt.
//!
//! `FRICTION.md` §21 and §41 both ask for metadata on `agent_start`, echoed by
//! `agent_list` and `agent_status`. The server does not have it. Until it
//! does, the prompt is the only carrier — the prompt is echoed back as
//! `summary` — and OpenKoalaBots ended up smuggling two separate markers through
//! it, both visible to the agent and both needing stripping at every display
//! site, which the live suite caught being missed in exactly one place.
//!
//! This does not make that problem go away; it makes it *one* problem.
//! Encoding and stripping happen here and nowhere else, the marker is a single
//! trailing comment rather than a prefix in the sentence the agent reads, and
//! every `summary` the core publishes has already been through [`strip`].
//!
//! The encoding is byte-identical to the Swift package's, so a run started by
//! one and read by the other agrees.

use crate::client::model::MetadataEntry;

/// Recognisable, greppable, and last — so an agent reading the prompt meets
/// the instruction first and the bookkeeping after it.
pub const OPEN_MARKER: &str = "<!--cua-spaces-meta:";
pub const CLOSE_MARKER: &str = "-->";

fn escape(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace(';', "%3B")
        .replace('=', "%3D")
        .replace("-->", "%2D%2D%3E")
}

fn unescape(value: &str) -> String {
    value
        .replace("%2D%2D%3E", "-->")
        .replace("%3D", "=")
        .replace("%3B", ";")
        .replace("%25", "%")
}

/// The prompt as it should be sent. Keys are sorted, so the same metadata
/// always produces the same bytes.
pub fn encode(prompt: &str, metadata: &[MetadataEntry]) -> String {
    if metadata.is_empty() {
        return prompt.to_string();
    }
    let mut sorted: Vec<&MetadataEntry> = metadata.iter().collect();
    sorted.sort_by(|a, b| a.key.cmp(&b.key));
    let pairs: Vec<String> = sorted
        .iter()
        .map(|entry| format!("{}={}", escape(&entry.key), escape(&entry.value)))
        .collect();
    format!("{prompt}\n\n{OPEN_MARKER}{}{CLOSE_MARKER}", pairs.join(";"))
}

fn marker_range(text: &str) -> Option<(usize, usize)> {
    let open = text.find(OPEN_MARKER)?;
    let body_start = open + OPEN_MARKER.len();
    let close = text[body_start..].find(CLOSE_MARKER)? + body_start;
    Some((open, close + CLOSE_MARKER.len()))
}

/// The metadata carried by a prompt echo (a `summary`), if any.
pub fn decode(text: &str) -> Vec<MetadataEntry> {
    let Some((open, close)) = marker_range(text) else {
        return Vec::new();
    };
    let body = &text[open + OPEN_MARKER.len()..close - CLOSE_MARKER.len()];
    body.split(';')
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            Some(MetadataEntry {
                key: unescape(key),
                value: unescape(value),
            })
        })
        .collect()
}

/// The text with the marker removed, for anything a person will read.
pub fn strip(text: &str) -> String {
    match marker_range(text) {
        Some((open, close)) => {
            let mut copy = String::with_capacity(text.len());
            copy.push_str(&text[..open]);
            copy.push_str(&text[close..]);
            copy.trim().to_string()
        }
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &str, value: &str) -> MetadataEntry {
        MetadataEntry {
            key: key.into(),
            value: value.into(),
        }
    }

    #[test]
    fn a_prompt_without_metadata_is_untouched() {
        assert_eq!(encode("ship it", &[]), "ship it");
        assert_eq!(strip("ship it"), "ship it");
        assert!(decode("ship it").is_empty());
    }

    #[test]
    fn the_marker_round_trips_and_is_sorted() {
        let encoded = encode(
            "ship it",
            &[entry("cua.schedule", "s-1"), entry("bot", "koala")],
        );
        assert_eq!(
            encoded,
            "ship it\n\n<!--cua-spaces-meta:bot=koala;cua.schedule=s-1-->"
        );
        assert_eq!(
            decode(&encoded),
            vec![entry("bot", "koala"), entry("cua.schedule", "s-1")]
        );
        assert_eq!(strip(&encoded), "ship it");
    }

    /// The separators a value could otherwise inject.
    #[test]
    fn separators_in_a_value_survive_the_round_trip() {
        let awkward = entry("k", "a=b;c%d-->e");
        let encoded = encode("go", std::slice::from_ref(&awkward));
        assert!(!encoded.contains("a=b;c"));
        assert_eq!(decode(&encoded), vec![awkward]);
        assert_eq!(strip(&encoded), "go");
    }
}
