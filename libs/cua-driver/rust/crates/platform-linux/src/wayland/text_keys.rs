// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! What typing text with key presses means on Wayland, shared by the libei and
//! niri keyboards: which characters can be typed, which keys apps receive, the
//! key and modifiers that type a symbol, and how a string is refused.

use std::borrow::Cow;
use std::ops::RangeInclusive;
use std::sync::OnceLock;

use anyhow::{anyhow, Result};
use xkbcommon::xkb;
use xkbcommon::xkb::ffi as xkb_ffi;

/// An evdev code is its xkb keycode minus this.
pub(in crate::wayland) const EVDEV_OFFSET: u32 = 8;

/// The evdev codes up to the highest keycode X11 clients can receive (255) that
/// Chromium gives a DOM `code` (the rows of
/// `ui/events/keycodes/dom/dom_code_data.inc` with an evdev code and a code
/// string). Chromium and Electron drop a key without one.
pub(in crate::wayland) const CHROMIUM_CODES: [RangeInclusive<u32>; 22] = [
    1..=83,
    85..=94,
    96..=100,
    102..=111,
    113..=117,
    119..=129,
    131..=138,
    140..=140,
    142..=144,
    155..=156,
    158..=159,
    161..=161,
    163..=168,
    171..=173,
    179..=180,
    183..=194,
    201..=201,
    207..=208,
    217..=217,
    224..=225,
    227..=227,
    231..=233,
];

pub(in crate::wayland) fn evdev_in(code: xkb::Keycode, ranges: &[RangeInclusive<u32>]) -> bool {
    code.raw()
        .checked_sub(EVDEV_OFFSET)
        .is_some_and(|evdev| ranges.iter().any(|range| range.contains(&evdev)))
}

/// Whether Chromium receives evdev `code`, which X11 apps then do too. A symbol
/// a layout has only on other keys, such as US's `EuroSign` on `<I443>` (evdev
/// 435), cannot reach every app.
pub(in crate::wayland) fn chromium_receives(code: u32) -> bool {
    CHROMIUM_CODES.iter().any(|range| range.contains(&code))
}

/// Whether evdev `code` is on the main typing block: the row keys, the space
/// bar, the 102nd key and the two extra keys of JIS and ABNT layouts.
pub(in crate::wayland) fn is_typing_key(code: u32) -> bool {
    matches!(code, 1..=53 | 57 | 86 | 89 | 124)
}

/// How every refused string's message ends.
pub(crate) const NOTHING_TYPED: &str = "nothing was typed";

/// A string that goes out whole or not at all, on every compositor.
pub(in crate::wayland) fn refused(detail: impl std::fmt::Display) -> anyhow::Error {
    anyhow!("foreground_unavailable: {detail}; {NOTHING_TYPED}")
}

super::dynamic_library::entry_points! { Xkb1, xkb_ffi, {
    xkb_utf32_to_keysym: fn(u32) -> u32;
    xkb_keymap_key_get_mods_for_level: fn(*mut xkb_ffi::xkb_keymap, u32, u32, u32, *mut u32, usize) -> usize;
}}

/// libxkbcommon 1.0's two functions typing needs. Importing them would stop
/// cua-driver and the SDK library from loading at all with an older
/// libxkbcommon, such as Ubuntu 20.04's 0.10, so they are looked up at run time
/// and typing refuses without them.
pub(in crate::wayland) fn xkb1() -> Result<&'static Xkb1> {
    // The process already links libxkbcommon.so.0, and opening it again returns
    // that library, also when the SDK library was loaded RTLD_LOCAL. Its symbols
    // cannot change, so a failure is cached as well.
    static LOADED: OnceLock<Result<Xkb1>> = OnceLock::new();
    LOADED
        .get_or_init(|| load_xkb1("libxkbcommon.so.0"))
        .as_ref()
        .map_err(|error| anyhow!("{error}"))
}

fn load_xkb1(soname: &str) -> Result<Xkb1> {
    // SAFETY: libxkbcommon has no load-time initialisers with preconditions.
    unsafe { Xkb1::open(soname) }.map_err(|_| {
        refused("typing needs libxkbcommon 1.0 or later (libxkbcommon0 on Debian and Ubuntu)")
    })
}

/// `text` with every line ending a line feed, so a Windows line ending is one
/// press of Return on every compositor.
pub(in crate::wayland) fn with_line_feeds(text: &str) -> Cow<'_, str> {
    if text.contains('\r') {
        Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        Cow::Borrowed(text)
    }
}

/// The keysym that types `c`. A line feed and a tab are the Return and Tab
/// keys; any other control character is refused, so it never runs as a command
/// in the app (`\u{8}` would press Backspace).
pub(in crate::wayland) fn text_sym(xkb: &Xkb1, c: char) -> Result<xkb::Keysym> {
    match c {
        '\n' => Ok(xkb::Keysym::Return),
        '\t' => Ok(xkb::Keysym::Tab),
        c if c.is_control() => Err(refused(format!(
            "control character U+{:04X} cannot be typed",
            u32::from(c)
        ))),
        // SAFETY: xkb_utf32_to_keysym takes any value and reads nothing else.
        c => match xkb::Keysym::new(unsafe { (xkb.xkb_utf32_to_keysym)(u32::from(c)) }) {
            sym if sym.raw() == 0 => Err(refused(format!("U+{:04X} has no keysym", u32::from(c)))),
            sym => Ok(sym),
        },
    }
}

/// `text` as it goes out on a route that types by itself, such as `wtype`: refused
/// if any of it can't be typed, with line endings made line feeds.
pub(in crate::wayland) fn typable_text(text: &str) -> Result<Cow<'_, str>> {
    let xkb = xkb1()?;
    let text = with_line_feeds(text);
    for c in text.chars() {
        text_sym(xkb, c)?;
    }
    Ok(text)
}

pub(in crate::wayland) fn level_mods(
    xkb: &Xkb1,
    map: &xkb::Keymap,
    code: xkb::Keycode,
    layout: u32,
    level: u32,
) -> Vec<u32> {
    let mut masks = [0_u32; 32];
    // SAFETY: `map` holds a live keymap, and `masks` has room for `masks.len()` masks.
    let count = unsafe {
        (xkb.xkb_keymap_key_get_mods_for_level)(
            map.get_raw_ptr(),
            code.raw(),
            layout,
            level,
            masks.as_mut_ptr(),
            masks.len(),
        )
    };
    masks.iter().take(count).copied().collect()
}

/// The modifiers apps read as commands, whatever the key's symbol: Alt+`÷` is a
/// shortcut, not a typed `÷`.
fn command_modifiers(map: &xkb::Keymap) -> u32 {
    [xkb::MOD_NAME_CTRL, xkb::MOD_NAME_ALT, xkb::MOD_NAME_LOGO]
        .into_iter()
        .map(|name| 1_u32.checked_shl(map.mod_get_index(name)).unwrap_or(0))
        .fold(0, |mask, bit| mask | bit)
}

/// The key that types `sym` in layout `group`: one Chromium receives first, then
/// one on the main typing block, then the fewest modifiers, then the lowest
/// code. `us` also has `(` and `)` with no modifier on keypad keys, which
/// terminals can read as commands, and `$` on a media key that X11 apps drop.
///
/// `reach` is asked about each key and modifier mask the keymap offers for
/// `sym`. It proves that the mask really types `sym` on that keyboard, and
/// returns how many modifiers it takes and its own answer, which comes back with
/// the key's evdev code.
pub(in crate::wayland) fn find_key<T>(
    xkb: &Xkb1,
    map: &xkb::Keymap,
    group: u32,
    sym: xkb::Keysym,
    mut reach: impl FnMut(xkb::Keycode, u32) -> Option<(u32, T)>,
) -> Option<(u32, T)> {
    let commands = command_modifiers(map);
    let mut best: Option<((bool, bool, u32, u32), T)> = None;
    for raw in map.min_keycode().raw().max(EVDEV_OFFSET)..=map.max_keycode().raw() {
        let code = xkb::Keycode::new(raw);
        for level in 0..map.num_levels_for_key(code, group) {
            if map.key_get_syms_by_level(code, group, level) != [sym] {
                continue;
            }
            for mask in level_mods(xkb, map, code, group, level) {
                if mask & commands != 0 {
                    continue;
                }
                let Some((modifiers, answer)) = reach(code, mask) else {
                    continue;
                };
                let evdev = raw - EVDEV_OFFSET;
                let rank = (
                    !chromium_receives(evdev),
                    !is_typing_key(evdev),
                    modifiers,
                    evdev,
                );
                if best.as_ref().is_none_or(|(known, _)| rank < *known) {
                    best = Some((rank, answer));
                }
            }
        }
    }
    best.map(|((_, _, _, evdev), answer)| (evdev, answer))
}

/// The ways to hold modifiers in layout `group` while typing, fewest keys first,
/// including none: combinations of keys every app receives whose press only sets
/// modifiers while it is down. A key that locks, latches or changes the layout is
/// left out, so typing never moves the user's keyboard state, and so are the
/// modifiers apps read as commands. Keys on the main typing block go first, and
/// each modifier takes one key.
#[cfg(feature = "portal-input")]
pub(in crate::wayland) fn modifier_chords(map: &xkb::Keymap, group: u32) -> Vec<Vec<u32>> {
    /// More than any real layout has: Shift, AltGr and a level 5 key.
    const MOST_MODIFIER_KEYS: usize = 5;
    let commands = command_modifiers(map);
    let mut found: Vec<(u32, u32)> = (map.min_keycode().raw().max(EVDEV_OFFSET)
        ..=map.max_keycode().raw())
        .filter(|raw| chromium_receives(raw - EVDEV_OFFSET))
        .filter_map(|raw| {
            let mut state = xkb::State::new(map);
            state.update_mask(0, 0, 0, 0, 0, group);
            state.update_key(xkb::Keycode::new(raw), xkb::KeyDirection::Down);
            let held = state.serialize_mods(xkb::STATE_MODS_DEPRESSED);
            let sticky = state.serialize_mods(xkb::STATE_MODS_LATCHED)
                | state.serialize_mods(xkb::STATE_MODS_LOCKED);
            (held != 0
                && held & commands == 0
                && sticky == 0
                && state.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE) == group)
                .then_some((raw - EVDEV_OFFSET, held))
        })
        .collect();
    found.sort_by_key(|&(evdev, _)| (!is_typing_key(evdev), evdev));
    let mut keys: Vec<(u32, u32)> = Vec::new();
    for (evdev, held) in found {
        if keys.iter().all(|&(_, other)| other != held) {
            keys.push((evdev, held));
        }
    }
    keys.truncate(MOST_MODIFIER_KEYS);
    let mut chords: Vec<Vec<u32>> = (0..1_u32 << keys.len())
        .map(|subset| {
            keys.iter()
                .enumerate()
                .filter(|(index, _)| subset & (1 << index) != 0)
                .map(|(_, &(evdev, _))| evdev)
                .collect()
        })
        .collect();
    chords.sort_by_key(Vec::len);
    chords
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_library_names_the_requirement() {
        assert_eq!(
            load_xkb1("libxkbcommon-cua-test-absent.so.0")
                .err()
                .expect("no such library")
                .to_string(),
            "foreground_unavailable: typing needs libxkbcommon 1.0 or later (libxkbcommon0 on \
             Debian and Ubuntu); nothing was typed"
        );
    }

    #[test]
    fn the_crate_does_not_require_libxkbcommon_1_0() {
        // Keep both typing paths live so dead-code elimination cannot hide a
        // load-time requirement in them.
        std::hint::black_box(text_sym as *const ());
        std::hint::black_box(level_mods as *const ());
        let exe = std::env::current_exe().unwrap();
        let output = std::process::Command::new("readelf")
            .arg("--version-info")
            .arg(&exe)
            .output()
            .expect("readelf (binutils) is needed to inspect the test binary");
        assert!(
            output.status.success(),
            "readelf failed for {}",
            exe.display()
        );
        let versions = String::from_utf8_lossy(&output.stdout);
        let xkb = versions
            .split("File: ")
            .find(|entry| entry.starts_with("libxkbcommon.so.0 "))
            .expect("the test binary links libxkbcommon");
        assert!(!xkb.contains("Name: V_1.0.0 "), "{xkb}");
    }
}
