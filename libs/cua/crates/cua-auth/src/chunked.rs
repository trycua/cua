//! A value of any size in a vault whose entries are small.
//!
//! Windows Credential Manager (`CredWrite`) refuses a blob over 2,560 bytes,
//! which is 1,280 UTF-16 characters. A Cua session (access, refresh and id
//! tokens) is about 3,800 characters, so it is split over several entries:
//! the main entry then holds a short header (`cua-chunked:1:<n>:<len>`) and
//! the pieces live in `<account>#0 .. <account>#<n-1>`. Every entry stays in
//! Credential Manager, which DPAPI-encrypts for the signed-in user; nothing
//! is ever written in plaintext to disk.
//!
//! The logic is generic over a [`Slots`] backend so it is tested on every
//! platform; only Windows plugs in the real one.

/// Most UTF-16 code units in one entry, well below the 1,280 the OS allows.
pub const CHUNK_UNITS: usize = 1000;
const PREFIX: &str = "cua-chunked:1:";

/// A set of named string slots.
pub trait Slots {
    type Err;
    fn read(&self, name: &str) -> Result<Option<String>, Self::Err>;
    fn write(&self, name: &str, value: &str) -> Result<(), Self::Err>;
    /// Deletes a slot; a missing one is not an error.
    fn remove(&self, name: &str) -> Result<(), Self::Err>;
}

fn chunk_name(account: &str, i: usize) -> String {
    format!("{account}#{i}")
}

/// `(pieces, total length in UTF-16 units)` of a header.
fn parse_header(s: &str) -> Option<(usize, usize)> {
    let rest = s.strip_prefix(PREFIX)?;
    let (n, len) = rest.split_once(':')?;
    Some((n.parse().ok()?, len.parse().ok()?))
}

/// Splits on char boundaries into pieces of at most [`CHUNK_UNITS`] units.
fn split(text: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut units = 0;
    for ch in text.chars() {
        let w = ch.len_utf16();
        if units + w > CHUNK_UNITS {
            out.push(std::mem::take(&mut cur));
            units = 0;
        }
        cur.push(ch);
        units += w;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn old_pieces<S: Slots>(s: &S, account: &str) -> usize {
    match s.read(account) {
        Ok(Some(h)) => parse_header(&h).map_or(0, |(n, _)| n),
        _ => 0,
    }
}

/// Stores `text` under `account`.
pub fn set<S: Slots>(s: &S, account: &str, text: &str) -> Result<(), S::Err> {
    let old = old_pieces(s, account);
    let new = if text.encode_utf16().count() <= CHUNK_UNITS {
        s.write(account, text)?;
        0
    } else {
        let pieces = split(text);
        // Pieces first, header last: a failure midway leaves the previous
        // value readable.
        for (i, p) in pieces.iter().enumerate() {
            s.write(&chunk_name(account, i), p)?;
        }
        let header = format!("{PREFIX}{}:{}", pieces.len(), text.encode_utf16().count());
        s.write(account, &header)?;
        pieces.len()
    };
    for i in new..old {
        s.remove(&chunk_name(account, i))?;
    }
    Ok(())
}

/// Reads the value under `account`. A chunked value with a missing or
/// altered piece reads as absent (the caller signs in again).
pub fn get<S: Slots>(s: &S, account: &str) -> Result<Option<String>, S::Err> {
    let Some(main) = s.read(account)? else {
        return Ok(None);
    };
    if !main.starts_with(PREFIX) {
        return Ok(Some(main));
    }
    let Some((n, len)) = parse_header(&main) else {
        return Ok(None);
    };
    let mut out = String::new();
    for i in 0..n {
        match s.read(&chunk_name(account, i))? {
            Some(p) => out.push_str(&p),
            None => return Ok(None),
        }
    }
    Ok((out.encode_utf16().count() == len).then_some(out))
}

/// Deletes the value and its pieces; returns whether the main entry existed.
pub fn delete<S: Slots>(s: &S, account: &str) -> Result<bool, S::Err> {
    let existed = s.read(account)?.is_some();
    for i in 0..old_pieces(s, account) {
        s.remove(&chunk_name(account, i))?;
    }
    s.remove(account)?;
    Ok(existed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    /// Enforces the Credential Manager limit.
    #[derive(Default)]
    struct Mem(RefCell<HashMap<String, String>>);
    impl Slots for Mem {
        type Err = String;
        fn read(&self, n: &str) -> Result<Option<String>, String> {
            Ok(self.0.borrow().get(n).cloned())
        }
        fn write(&self, n: &str, v: &str) -> Result<(), String> {
            if v.encode_utf16().count() * 2 > 2560 {
                return Err("longer than platform limit of 2560 chars".into());
            }
            self.0.borrow_mut().insert(n.into(), v.into());
            Ok(())
        }
        fn remove(&self, n: &str) -> Result<(), String> {
            self.0.borrow_mut().remove(n);
            Ok(())
        }
    }

    fn big(n: usize) -> String {
        (0..n).map(|i| char::from(b'a' + (i % 26) as u8)).collect()
    }

    #[test]
    fn small_values_are_stored_as_is() {
        let m = Mem::default();
        set(&m, "a", "{\"x\":1}").unwrap();
        assert_eq!(m.0.borrow().len(), 1);
        assert_eq!(get(&m, "a").unwrap().as_deref(), Some("{\"x\":1}"));
    }

    #[test]
    fn a_session_sized_value_round_trips() {
        let m = Mem::default();
        let v = big(3800);
        set(&m, "a", &v).unwrap();
        assert_eq!(get(&m, "a").unwrap(), Some(v));
        assert_eq!(m.0.borrow().len(), 5); // header + 4 pieces
    }

    #[test]
    fn multibyte_values_round_trip_within_the_limit() {
        let m = Mem::default();
        let v: String = "é😀".repeat(2000);
        set(&m, "a", &v).unwrap();
        assert_eq!(get(&m, "a").unwrap(), Some(v));
    }

    #[test]
    fn shrinking_removes_stale_pieces() {
        let m = Mem::default();
        set(&m, "a", &big(5000)).unwrap();
        set(&m, "a", &big(1500)).unwrap();
        assert_eq!(m.0.borrow().len(), 3);
        set(&m, "a", "tiny").unwrap();
        assert_eq!(m.0.borrow().len(), 1);
        assert_eq!(get(&m, "a").unwrap().as_deref(), Some("tiny"));
    }

    #[test]
    fn delete_removes_everything() {
        let m = Mem::default();
        set(&m, "a", &big(4000)).unwrap();
        assert!(delete(&m, "a").unwrap());
        assert!(m.0.borrow().is_empty());
        assert!(!delete(&m, "a").unwrap());
        assert_eq!(get(&m, "a").unwrap(), None);
    }

    #[test]
    fn a_missing_or_altered_piece_reads_as_absent() {
        let m = Mem::default();
        set(&m, "a", &big(4000)).unwrap();
        m.0.borrow_mut().remove("a#2");
        assert_eq!(get(&m, "a").unwrap(), None);
        set(&m, "a", &big(4000)).unwrap();
        m.0.borrow_mut().insert("a#1".into(), "short".into());
        assert_eq!(get(&m, "a").unwrap(), None);
    }

    #[test]
    fn a_failed_write_keeps_the_previous_value() {
        struct Fail(Mem);
        impl Slots for Fail {
            type Err = String;
            fn read(&self, n: &str) -> Result<Option<String>, String> {
                self.0.read(n)
            }
            fn write(&self, n: &str, v: &str) -> Result<(), String> {
                if n == "a#2" {
                    return Err("boom".into());
                }
                self.0.write(n, v)
            }
            fn remove(&self, n: &str) -> Result<(), String> {
                self.0.remove(n)
            }
        }
        let f = Fail(Mem::default());
        set(&f, "a", "old").unwrap();
        assert!(set(&f, "a", &big(4000)).is_err());
        assert_eq!(get(&f, "a").unwrap().as_deref(), Some("old"));
    }
}
