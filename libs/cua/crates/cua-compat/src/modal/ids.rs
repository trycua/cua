//! Object ids. The Modal client checks their shape: `<prefix>-` and 22
//! characters of `[0-9A-Za-z]` (a sandbox id of any other shape is taken
//! to be a V2 sandbox), and a task id that ends in `V` is a V2 task.

use rand::Rng;

const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

pub fn new_id(prefix: &str) -> String {
    let mut rng = rand::rng();
    let tail: String = (0..22)
        .map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char)
        .collect();
    format!("{prefix}-{tail}")
}

/// A V2 sandbox id: not the V1 shape (22 alphanumerics), which is how the
/// client tells the two backends apart.
pub fn new_id_v2(prefix: &str) -> String {
    let mut rng = rand::rng();
    let tail: String = (0..24)
        .map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char)
        .collect();
    format!("{prefix}-{tail}")
}

/// A V2 task id: ends in `V`.
pub fn new_task_id_v2() -> String {
    let mut rng = rand::rng();
    let tail: String = (0..21)
        .map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char)
        .collect();
    format!("ta-{tail}V")
}

/// A V1 task id: never ends in `V` (that marks a V2 sandbox task).
pub fn new_task_id() -> String {
    loop {
        let id = new_id("ta");
        if !id.ends_with('V') {
            return id;
        }
    }
}

/// Whether `id` is `<prefix>-` plus 22 alphanumerics.
pub fn is_id(id: &str, prefix: &str) -> bool {
    id.strip_prefix(prefix)
        .and_then(|r| r.strip_prefix('-'))
        .is_some_and(|t| t.len() == 22 && t.bytes().all(|b| b.is_ascii_alphanumeric()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_have_the_shape_the_client_requires() {
        for _ in 0..200 {
            let id = new_id("sb");
            assert!(is_id(&id, "sb"), "{id}");
            assert!(!new_task_id().ends_with('V'));
        }
        assert!(!is_id(&new_id_v2("sb"), "sb"));
        let t = new_task_id_v2();
        assert!(t.starts_with("ta-") && t.ends_with('V') && t.len() == 25);
        assert!(!is_id("sb-short", "sb"));
        assert!(!is_id("ap-0123456789012345678901", "sb"));
    }
}
