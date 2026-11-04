// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The five-square change bar and the thousands separator the diff screens
//! share with the approval card: the same proportions as GitHub's bar, so a
//! file reads the same in the thread and on the phone's diff screen.

use serde::Serialize;

/// One square of a file's bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Square {
    Add,
    Del,
    None,
}

/// `12345` as `12,345`.
pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// GitHub's five-square bar for `add` and `del` lines: squares in
/// proportion, a changed side never rounds to nothing, the rest neutral.
/// No change, and a binary file, is five neutral squares.
pub fn bar(add: u64, del: u64) -> [Square; 5] {
    let total = add.saturating_add(del);
    if total == 0 {
        return [Square::None; 5];
    }
    let share = |n: u64| ((n as u128 * 5 + total as u128 / 2) / total as u128) as u64;
    let (mut a, mut d) = (share(add), share(del));
    if add > 0 && a == 0 {
        a = 1;
    }
    if del > 0 && d == 0 {
        d = 1;
    }
    while a + d > 5 {
        if a >= d {
            a -= 1;
        } else {
            d -= 1;
        }
    }
    let mut out = [Square::None; 5];
    for (i, s) in out.iter_mut().enumerate() {
        let i = i as u64;
        *s = if i < a {
            Square::Add
        } else if i < a + d {
            Square::Del
        } else {
            Square::None
        };
    }
    out
}
