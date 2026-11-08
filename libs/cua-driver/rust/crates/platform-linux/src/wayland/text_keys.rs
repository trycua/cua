// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! What typing text with key presses means on Wayland, shared by the libei and
//! niri keyboards: which characters can be typed, which keys apps receive, the
//! key and modifiers that type a symbol, and how a string is refused.

use std::borrow::Cow;
use std::ops::RangeInclusive;

use anyhow::{anyhow, Result};
use xkbcommon::xkb;

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
pub(in crate::wayland) fn text_sym(c: char) -> Result<xkb::Keysym> {
    match c {
        '\n' => Ok(xkb::Keysym::Return),
        '\t' => Ok(xkb::Keysym::Tab),
        c if c.is_control() => Err(refused(format!(
            "control character U+{:04X} cannot be typed",
            u32::from(c)
        ))),
        c => match xkb::utf32_to_keysym(u32::from(c)) {
            sym if sym.raw() == 0 => Err(refused(format!("U+{:04X} has no keysym", u32::from(c)))),
            sym => Ok(sym),
        },
    }
}

/// `text` as it goes out on a route that types by itself, such as `wtype`: refused
/// if any of it can't be typed, with line endings made line feeds.
pub(in crate::wayland) fn typable_text(text: &str) -> Result<Cow<'_, str>> {
    let text = with_line_feeds(text);
    for c in text.chars() {
        text_sym(c)?;
    }
    Ok(text)
}

pub(in crate::wayland) fn level_mods(
    map: &xkb::Keymap,
    code: xkb::Keycode,
    layout: u32,
    level: u32,
) -> Vec<u32> {
    let mut masks = [0_u32; 32];
    let count = map.key_get_mods_for_level(code, layout, level, &mut masks);
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
            for mask in level_mods(map, code, group, level) {
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
