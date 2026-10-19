//! Text input on niri through a virtual keyboard that carries niri's own
//! keymap, so every character goes out on the key the user's layout has it
//! on. A tool like `wtype` instead numbers the n-th distinct character of a
//! call as evdev code n, and Chromium and Electron take the DOM `code` from
//! the evdev code: a punctuation mark on code 1 (Escape) is dropped, one on
//! code 14 (Backspace) deletes the character before it, and one on code 28
//! (Enter) can submit a form.
//!
//! Characters the active layout lacks go on spare keys of an extended copy
//! of the keymap: keys without symbols that Chromium knows, then F13 to F24.
//! niri sends a virtual keyboard's keymap to every client with the device's
//! next key, so clients read each key with the map it was sent in; the
//! compositor's map is uploaded again before the device goes away.
//!
//! Before each key, niri is asked which window has keyboard focus; typing
//! stops when it is no longer the window that had it when typing started. A
//! locked niri reports no focused window, so this also stops at the lock
//! screen. Every round trip and the whole call have a deadline, and held
//! keys are released before returning: destroying the device releases
//! nothing in niri.
//!
//! Text outside the Basic Multilingual Plane, such as most emoji, goes out
//! as its Unicode keysym like any other character, as `wtype` sends it.
//! Chromium cuts a keysym's character to 16 bits, so it types a wrong
//! character there whatever the input route.

use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::ops::RangeInclusive;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::FileExt;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use wayland_client::protocol::{wl_callback, wl_keyboard, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use xkbcommon::xkb;

use super::super::virtual_keyboard::protocol::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
use super::super::virtual_keyboard::protocol::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;

/// The whole call's budget: setup, then this much per key.
const CALL_BASE: Duration = Duration::from_secs(3);
const PER_KEY: Duration = Duration::from_millis(100);

/// How long a call that sends `keys` keys gives itself, from when its
/// keyboard session begins. Past it, the call stops and says how far it got.
pub fn call_budget(keys: usize) -> Duration {
    CALL_BASE + PER_KEY * u32::try_from(keys).unwrap_or(u32::MAX)
}
/// How long releasing held keys and restoring the keymap may take once the
/// call is over, even past its deadline.
const CLEANUP_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_KEYMAP: u32 = 1024 * 1024;

const KEY_RELEASED: u32 = 0;
const KEY_PRESSED: u32 = 1;

// ── Planning ─────────────────────────────────────────────────────────────────

/// One key: its evdev code, and the layout and modifiers it is sent in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Key {
    code: u32,
    modifiers: u32,
    group: u32,
}

/// Consecutive keys typed with one keymap: niri's own when `extended` is
/// `None`, otherwise that copy with missing symbols on spare keys.
#[derive(Debug)]
struct Part {
    keys: Vec<Key>,
    extended: Option<String>,
}

/// The highest keycode Xwayland clients can receive.
const MAX_X11_KEYCODE: u32 = 255;

/// The evdev codes up to `MAX_X11_KEYCODE` that Chromium gives a DOM `code`
/// (the rows of `ui/events/keycodes/dom/dom_code_data.inc` with an evdev
/// code and a code string). Chromium and Electron drop a key without one.
const CHROMIUM_CODES: [RangeInclusive<u32>; 22] = [
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

/// F13 to F24: Chromium knows them, keymaps give them symbols, and few
/// keyboards have them, so a keymap may borrow them once its keys without
/// symbols run out.
const BORROWABLE: RangeInclusive<u32> = 183..=194;

fn evdev_in(code: xkb::Keycode, ranges: &[RangeInclusive<u32>]) -> bool {
    code.raw()
        .checked_sub(8)
        .is_some_and(|evdev| ranges.iter().any(|range| range.contains(&evdev)))
}

/// Whether Chromium receives evdev `code` (all of these are within
/// `MAX_X11_KEYCODE`). A symbol the layout has only on other keys, such as
/// US's `EuroSign` on `<I443>` (evdev 435), counts as missing and goes on a
/// spare key.
fn chromium_receives(code: u32) -> bool {
    CHROMIUM_CODES.iter().any(|range| range.contains(&code))
}

fn refused(detail: impl std::fmt::Display) -> anyhow::Error {
    anyhow!("foreground_unavailable: {detail}; nothing was typed")
}

/// `text`'s keys in layout `group` of `map`, cut into parts whose symbols
/// the layout lacks fit on the keymap's spare keys.
fn plan_text(map: &str, group: u32, text: &str) -> Result<Vec<Part>> {
    let keymap = compile(map)?;
    if group >= keymap.num_layouts() {
        return Err(refused(format!(
            "niri's active layout {group} is absent from its keymap"
        )));
    }
    let spare = spare_keys(&keymap).len();
    split(&keymap, group, text, spare)?
        .into_iter()
        .map(|(part, missing)| {
            if missing.is_empty() {
                return Ok(Part {
                    keys: characters(&keymap, group, part)?,
                    extended: None,
                });
            }
            let (extended, serialized) = extend(map, &keymap, &missing)?;
            Ok(Part {
                keys: characters(&extended, group, part)?,
                extended: Some(serialized),
            })
        })
        .collect()
}

/// The keysym that types `c`. A line feed and a tab are the Return and Tab
/// keys, as `wtype` types them; any other control character is refused.
fn text_sym(c: char) -> Result<xkb::Keysym> {
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

fn characters(keymap: &xkb::Keymap, group: u32, text: &str) -> Result<Vec<Key>> {
    text.chars()
        .map(|c| {
            find(keymap, group, text_sym(c)?)
                .ok_or_else(|| refused(format!("U+{:04X} is absent from the keymap", u32::from(c))))
        })
        .collect()
}

/// `text` cut at character boundaries into parts, each with the distinct
/// symbols the layout lacks, in order of use, at most `spare` of them.
fn split<'t>(
    keymap: &xkb::Keymap,
    group: u32,
    text: &'t str,
    spare: usize,
) -> Result<Vec<(&'t str, Vec<xkb::Keysym>)>> {
    let mut parts = Vec::new();
    let (mut rest, mut start) = (text, 0);
    let mut missing = Vec::new();
    for (at, c) in text.char_indices() {
        let sym = text_sym(c)?;
        let present = find(keymap, group, sym).is_some_and(|key| chromium_receives(key.code));
        if present || missing.contains(&sym) {
            continue;
        }
        if spare == 0 {
            return Err(refused(format!(
                "U+{:04X} is absent from the active layout and niri's keymap has no spare key",
                u32::from(c)
            )));
        }
        if missing.len() == spare {
            let (part, after) = rest.split_at(at - start);
            parts.push((part, std::mem::take(&mut missing)));
            (rest, start) = (after, at);
        }
        missing.push(sym);
    }
    parts.push((rest, missing));
    Ok(parts)
}

/// Put each of `syms` on a spare key of `map` and prove every other key, the
/// layouts and the modifiers unchanged. Returns the extended keymap compiled
/// and as libxkbcommon serializes it.
fn extend(map: &str, keymap: &xkb::Keymap, syms: &[xkb::Keysym]) -> Result<(xkb::Keymap, String)> {
    let spare = spare_keys(keymap);
    if syms.len() > spare.len() {
        return Err(refused("more missing symbols than spare keys"));
    }
    let entries: String = spare
        .iter()
        .zip(syms)
        .map(|((_, name), &sym)| {
            format!(
                "replace key <{name}> {{ [ {} ] }};\n",
                xkb::keysym_get_name(sym)
            )
        })
        .collect();
    let end = symbols_end(map).ok_or_else(|| refused("niri's keymap has an unexpected layout"))?;
    let (head, tail) = map.split_at(end);
    let extended = compile(&format!("{head}{entries}{tail}"))?;
    let used: Vec<xkb::Keycode> = spare
        .iter()
        .take(syms.len())
        .map(|(code, _)| *code)
        .collect();
    if !unchanged(keymap, &extended, &used) {
        return Err(refused(
            "extending niri's keymap would change existing keys",
        ));
    }
    let text = extended.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
    Ok((extended, text))
}

/// The keys up to `MAX_X11_KEYCODE` a keymap may put missing symbols on, in
/// order: those without symbols that Chromium knows, then the borrowable
/// ones the keymap has. Only a keymap with neither uses keys without symbols
/// that Chromium does not know.
fn spare_keys(keymap: &xkb::Keymap) -> Vec<(xkb::Keycode, String)> {
    let first = keymap.min_keycode().raw().max(9);
    let last = keymap.max_keycode().raw().min(MAX_X11_KEYCODE);
    let (empty, defined): (Vec<_>, Vec<_>) = (first..=last)
        .map(xkb::Keycode::new)
        .filter_map(|code| Some((code, keymap.key_get_name(code)?.to_owned())))
        .partition(|(code, _)| keymap.num_layouts_for_key(*code) == 0);
    let (known, unknown): (Vec<_>, Vec<_>) = empty
        .into_iter()
        .partition(|(code, _)| evdev_in(*code, &CHROMIUM_CODES));
    let chromium: Vec<_> = known
        .into_iter()
        .chain(
            defined
                .into_iter()
                .filter(|(code, _)| evdev_in(*code, &[BORROWABLE])),
        )
        .collect();
    if chromium.is_empty() {
        unknown
    } else {
        chromium
    }
}

/// Where the symbols section closes: the last `};` before the keymap's own.
fn symbols_end(map: &str) -> Option<usize> {
    let keymap_end = map.rfind("};")?;
    let end = map.get(..keymap_end)?.rfind("};")?;
    let symbols = map.find("xkb_symbols")?;
    let after = map.get(symbols..end)?;
    let other = ["xkb_keycodes", "xkb_types", "xkb_compat", "xkb_geometry"];
    (!other.iter().any(|section| after.contains(section))).then_some(end)
}

/// Every key but `added` is the same in both keymaps, and each of `added`
/// had no symbols or is borrowable, and now types only its one symbol.
fn unchanged(original: &xkb::Keymap, extended: &xkb::Keymap, added: &[xkb::Keycode]) -> bool {
    let mods = |map: &xkb::Keymap| -> Vec<String> {
        (0..map.num_mods())
            .map(|index| map.mod_get_name(index).to_owned())
            .collect()
    };
    original.num_layouts() == extended.num_layouts()
        && mods(original) == mods(extended)
        && (original.min_keycode().raw()..=original.max_keycode().raw())
            .map(xkb::Keycode::new)
            .filter(|code| !added.contains(code))
            .all(|code| same_key(original, extended, code))
        && added.iter().all(|&code| {
            (original.num_layouts_for_key(code) == 0 || evdev_in(code, &[BORROWABLE]))
                && inert(extended, code)
        })
}

fn same_key(original: &xkb::Keymap, extended: &xkb::Keymap, code: xkb::Keycode) -> bool {
    let layouts = original.num_layouts_for_key(code);
    layouts == extended.num_layouts_for_key(code)
        && (0..layouts).all(|layout| {
            let levels = original.num_levels_for_key(code, layout);
            levels == extended.num_levels_for_key(code, layout)
                && (0..levels).all(|level| {
                    original.key_get_syms_by_level(code, layout, level)
                        == extended.key_get_syms_by_level(code, layout, level)
                        && level_mods(original, code, layout, level)
                            == level_mods(extended, code, layout, level)
                })
        })
}

fn level_mods(map: &xkb::Keymap, code: xkb::Keycode, layout: u32, level: u32) -> Vec<u32> {
    let mut masks = [0_u32; 32];
    let count = map.key_get_mods_for_level(code, layout, level, &mut masks);
    masks.iter().take(count).copied().collect()
}

/// An added key has one symbol, in one layout and level, and changes no
/// modifier or layout.
fn inert(map: &xkb::Keymap, code: xkb::Keycode) -> bool {
    let mut state = xkb::State::new(map);
    state.update_key(code, xkb::KeyDirection::Down);
    map.num_layouts_for_key(code) == 1
        && map.num_levels_for_key(code, 0) == 1
        && map.key_get_syms_by_level(code, 0, 0).len() == 1
        && state.serialize_mods(xkb::STATE_MODS_EFFECTIVE) == 0
        && state.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE) == 0
}

fn compile(map: &str) -> Result<xkb::Keymap> {
    let context =
        xkb::Context::new(xkb::CONTEXT_NO_DEFAULT_INCLUDES | xkb::CONTEXT_NO_ENVIRONMENT_NAMES);
    xkb::Keymap::new_from_string(
        &context,
        map.to_owned(),
        xkb::KEYMAP_FORMAT_TEXT_V1,
        xkb::KEYMAP_COMPILE_NO_FLAGS,
    )
    .ok_or_else(|| refused("niri supplied a keymap libxkbcommon cannot compile"))
}

/// The key and modifiers that type `sym` in layout `group`: one Chromium
/// receives first, then the fewest modifiers, then the lowest code.
fn find(map: &xkb::Keymap, group: u32, sym: xkb::Keysym) -> Option<Key> {
    let mut candidates = Vec::new();
    for raw in map.min_keycode().raw().max(8)..=map.max_keycode().raw() {
        let code = xkb::Keycode::new(raw);
        for level in 0..map.num_levels_for_key(code, group) {
            if map.key_get_syms_by_level(code, group, level) != [sym] {
                continue;
            }
            let mut masks = [0_u32; 32];
            let count = map.key_get_mods_for_level(code, group, level, &mut masks);
            candidates.extend(masks.iter().take(count).filter_map(|&modifiers| {
                let mut state = xkb::State::new(map);
                state.update_mask(modifiers, 0, 0, 0, 0, group);
                (state.key_get_one_sym(code) == sym).then_some(Key {
                    code: raw - 8,
                    modifiers,
                    group,
                })
            }));
        }
    }
    candidates.into_iter().min_by_key(|key| {
        (
            !chromium_receives(key.code),
            key.modifiers.count_ones(),
            key.code,
        )
    })
}

/// The key `name` (an X keysym name, exact or else in any case) with
/// `modifiers` held: in the active layout `group`, or else in the first other
/// layout that has it, so `ctrl+c` still copies while a layout without Latin
/// letters is active.
fn plan_key(map: &str, group: u32, modifiers: &[String], name: &str) -> Result<Key> {
    let keymap = compile(map)?;
    let sym = [xkb::KEYSYM_NO_FLAGS, xkb::KEYSYM_CASE_INSENSITIVE]
        .into_iter()
        .map(|flags| xkb::keysym_from_name(name, flags))
        .find(|sym| sym.raw() != 0)
        .ok_or_else(|| refused(format!("{name:?} is not a key name")))?;
    let mut key = find(&keymap, group, sym)
        .or_else(|| {
            (0..keymap.num_layouts())
                .filter(|&other| other != group)
                .find_map(|other| find(&keymap, other, sym))
        })
        .ok_or_else(|| refused(format!("key {name:?} is in no layout of niri's keymap")))?;
    for modifier in modifiers {
        let name = match modifier.as_str() {
            "ctrl" => xkb::MOD_NAME_CTRL,
            "shift" => xkb::MOD_NAME_SHIFT,
            "alt" => xkb::MOD_NAME_ALT,
            "logo" => xkb::MOD_NAME_LOGO,
            other => return Err(refused(format!("unsupported modifier {other:?}"))),
        };
        key.modifiers |= 1_u32
            .checked_shl(keymap.mod_get_index(name))
            .ok_or_else(|| refused(format!("niri's keymap has no {name} modifier")))?;
    }
    Ok(key)
}

// ── Typing ───────────────────────────────────────────────────────────────────

/// Type `text` into the niri window that has keyboard focus now, which must
/// be `expected` when the caller activated one, stopping if focus leaves it.
/// A stop after some characters went out reports how many, without the
/// `foreground_unavailable` code that means nothing happened.
pub(in crate::wayland) fn type_text(expected: Option<u64>, text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    let mut session = Session::begin(expected, text.chars().count())?;
    let parts = plan_text(session.keyboard.base_text(), session.group, text)?;
    let total: usize = parts.iter().map(|part| part.keys.len()).sum();
    let mut sent = Sent::default();
    let typed = session.send(&parts, &mut sent);
    outcome(sent, total, typed, session.finish())
}

/// Press the key `name` (an X keysym name) with `modifiers` (`ctrl`,
/// `shift`, `alt`, `logo`) held, into the niri window that has keyboard
/// focus, which must be `expected` when the caller activated one. The
/// modifiers are held through the keyboard's modifier state.
pub(in crate::wayland) fn press(
    expected: Option<u64>,
    modifiers: &[String],
    name: &str,
) -> Result<()> {
    let mut session = Session::begin(expected, 1)?;
    let key = plan_key(session.keyboard.base_text(), session.group, modifiers, name)?;
    let parts = [Part {
        keys: vec![key],
        extended: None,
    }];
    let mut sent = Sent::default();
    let typed = session.send(&parts, &mut sent);
    match (typed, session.finish()) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), _) if sent.underway => Err(anyhow!("the key may have gone out: {error:#}")),
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error.context("pressed the key, but could not finish")),
    }
}

/// One call's keyboard, bound to the window that had keyboard focus when it
/// began and to niri's active layout then.
struct Session {
    keyboard: Keyboard,
    target: u64,
    group: u32,
    deadline: Instant,
}

/// How long before its tool call's deadline a session stops: time for the
/// keyboard's cleanup and the focus restore after it, so the call returns
/// the keyboard's report of how far it got rather than a bare timeout.
const REPORT_MARGIN: Duration = Duration::from_secs(2);

/// When a session sending `keys` keys from `now` stops, given the deadline
/// of the tool call it serves.
fn session_deadline(now: Instant, keys: usize, call: Option<Instant>) -> Instant {
    let own = now + call_budget(keys);
    call.map_or(own, |call| {
        own.min(call.checked_sub(REPORT_MARGIN).unwrap_or(call))
    })
}

impl Session {
    fn begin(expected: Option<u64>, keys: usize) -> Result<Self> {
        let deadline = session_deadline(
            Instant::now(),
            keys,
            crate::input::action::current().map(|action| action.deadline()),
        );
        let focused = super::keyboard_focus().map_err(super::input_unavailable)?;
        let target = admitted_target(expected, focused)?;
        super::refuse_if_locked().map_err(super::input_unavailable)?;
        let group = super::keyboard_layout().map_err(super::input_unavailable)?;
        let keyboard = Keyboard::bind(deadline).map_err(|error| refused(format!("{error:#}")))?;
        Ok(Self {
            keyboard,
            target,
            group,
            deadline,
        })
    }

    fn send(&mut self, parts: &[Part], sent: &mut Sent) -> Result<()> {
        let target = self.target;
        type_parts(
            &mut self.keyboard,
            parts,
            self.group,
            self.deadline,
            sent,
            |sent| match super::keyboard_focus() {
                Ok(Some(focused)) if focused == target => Ok(()),
                Ok(focused) => Err(focus_moved(target, focused, sent)),
                Err(error) => Err(super::input_unavailable(error)),
            },
        )
    }

    /// Finish in the layout active now: the user may have switched layouts,
    /// or focus moved to a window with its own, during the call.
    fn finish(self) -> Result<()> {
        let group = closing_group(super::keyboard_layout(), self.group);
        self.keyboard.finish(group)
    }
}

/// The layout group a call leaves the keyboard in: niri's active one at the
/// end, or the one it started in when niri cannot say.
fn closing_group(current: Result<u32>, start: u32) -> u32 {
    current.unwrap_or(start)
}

/// The window a call types into: the one with keyboard focus, which must be
/// `expected` when the caller activated a window, since another window may
/// have taken focus after the activation was confirmed.
fn admitted_target(expected: Option<u64>, focused: Option<u64>) -> Result<u64> {
    match (expected, focused) {
        (_, None) => Err(anyhow!(
            "foreground_unavailable: no niri window has keyboard focus (the screen may be \
             locked, or the overview, a layer-shell surface or a niri dialog has it); nothing \
             was typed"
        )),
        (Some(expected), Some(focused)) if focused != expected => Err(anyhow!(
            "foreground_unavailable: niri keyboard focus is on window {focused}, not window \
             {expected}; nothing was typed"
        )),
        (_, Some(focused)) => Ok(focused),
    }
}

/// Focus left `target` for `focused`. niri can move focus between its answer
/// and the next key, so once keys went out the last one may have reached the
/// window that has focus now; niri offers no check that is atomic with a key.
fn focus_moved(target: u64, focused: Option<u64>, sent: usize) -> anyhow::Error {
    let now = focused.map_or_else(|| "no window".to_owned(), |id| format!("window {id}"));
    if sent == 0 {
        return anyhow!(
            "foreground_unavailable: niri keyboard focus moved from window {target} to {now}"
        );
    }
    anyhow!(
        "foreground_unavailable: niri keyboard focus moved from window {target} to {now}; \
         the last key sent may have reached {now}"
    )
}

/// How far a call got: the keys niri confirmed, and whether one more was
/// sent but not confirmed, so may have gone out.
#[derive(Clone, Copy, Debug, Default)]
struct Sent {
    done: usize,
    underway: bool,
}

/// A stop before any key keeps its refusal; a stop after some keys went out,
/// or with one that may have, says how many, without the
/// `foreground_unavailable` code that would report no effect, as a partial
/// scroll does.
fn outcome(sent: Sent, total: usize, typed: Result<()>, finished: Result<()>) -> Result<()> {
    let done = sent.done;
    match (typed, finished) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), _) if sent.underway => Err(anyhow!(
            "type_text stopped after {done} of {total} characters, and character {} may have \
             gone out: {error:#}",
            done + 1
        )),
        (Err(error), _) if done == 0 => Err(error),
        (Err(error), _) => Err(anyhow!(
            "type_text stopped after {done} of {total} characters: {error:#}"
        )),
        (Ok(()), Err(error)) => Err(error.context(format!(
            "typed all {total} characters, but could not put niri's keymap back"
        ))),
    }
}

/// What sends keys: the virtual keyboard, or a recorder in tests.
trait Strokes {
    fn upload(&mut self, map: &str) -> Result<()>;
    /// Send `key` in its layout, then return to the active layout `group`.
    fn stroke(&mut self, key: Key, group: u32, deadline: Instant) -> Result<()>;
}

/// Send each key of `parts`, uploading a part's extended keymap before its
/// first key, once `still_focused` (told how many keys went out) admits the
/// key and the deadline has not passed. `sent` counts the keys that went out.
fn type_parts(
    keyboard: &mut impl Strokes,
    parts: &[Part],
    group: u32,
    deadline: Instant,
    sent: &mut Sent,
    mut still_focused: impl FnMut(usize) -> Result<()>,
) -> Result<()> {
    for part in parts {
        for (index, key) in part.keys.iter().enumerate() {
            if Instant::now() >= deadline {
                bail!("foreground_unavailable: niri text input ran out of time");
            }
            still_focused(sent.done)?;
            if let (0, Some(map)) = (index, &part.extended) {
                keyboard.upload(map)?;
            }
            sent.underway = true;
            keyboard.stroke(*key, group, deadline)?;
            sent.underway = false;
            sent.done += 1;
        }
    }
    Ok(())
}

/// A keymap niri sent: the file to upload it again from, and its text.
struct Keymap {
    file: File,
    size: u32,
    text: String,
}

#[derive(Default)]
struct State {
    seats: super::super::primary_seat::Seats<wl_seat::WlSeat>,
    manager: Option<ZwpVirtualKeyboardManagerV1>,
    keymap: Option<Keymap>,
    error: Option<String>,
    synced: bool,
}

/// A virtual keyboard on the attested niri, holding niri's keymap.
struct Keyboard {
    connection: Connection,
    queue: EventQueue<State>,
    state: State,
    device: ZwpVirtualKeyboardV1,
    /// The compositor's keymap, uploaded when the device was made.
    base: Keymap,
    /// Whether the device holds an extended keymap.
    extended: bool,
    pressed: Vec<u32>,
}

impl Keyboard {
    fn bind(deadline: Instant) -> Result<Self> {
        let socket = super::attested_wayland_socket()?;
        let connection =
            Connection::from_socket(socket).context("connect to niri's Wayland display")?;
        let mut queue = connection.new_event_queue::<State>();
        let handle = queue.handle();
        connection.display().get_registry(&handle, ());
        let mut state = State::default();
        super::super::bounded_roundtrip(&connection, &mut queue, &mut state, deadline)?;
        // The seats' names.
        super::super::bounded_roundtrip(&connection, &mut queue, &mut state, deadline)?;
        let seat = state.seats.selected().context("niri exposes no wl_seat")?;
        let manager = state
            .manager
            .clone()
            .context("niri exposes no zwp_virtual_keyboard_manager_v1")?;
        let wl_keyboard = seat.get_keyboard(&handle, ());
        super::super::bounded_roundtrip(&connection, &mut queue, &mut state, deadline)?;
        wl_keyboard.release();
        if let Some(error) = state.error.take() {
            bail!("{error}");
        }
        let base = state
            .keymap
            .take()
            .context("niri sent no keyboard keymap")?;
        let device = manager.create_virtual_keyboard(&seat, &handle, ());
        device.keymap(1, base.file.as_fd(), base.size);
        Ok(Self {
            connection,
            queue,
            state,
            device,
            base,
            extended: false,
            pressed: Vec::new(),
        })
    }

    fn base_text(&self) -> &str {
        &self.base.text
    }

    /// Release anything a failed stroke may have left held, put niri's own
    /// keymap back for clients if the device sent another one, and destroy
    /// the device. Runs on its own short deadline, after the call's.
    fn finish(mut self, group: u32) -> Result<()> {
        let deadline = Instant::now() + CLEANUP_TIMEOUT;
        for code in std::mem::take(&mut self.pressed) {
            self.device
                .key(super::super::event_time_ms(), code, KEY_RELEASED);
        }
        if self.extended {
            self.device
                .keymap(1, self.base.file.as_fd(), self.base.size);
        }
        self.device.modifiers(0, 0, 0, group);
        let restored = super::super::bounded_roundtrip(
            &self.connection,
            &mut self.queue,
            &mut self.state,
            deadline,
        );
        self.device.destroy();
        let flushed = self.queue.flush().context("flush to niri");
        restored.and(flushed)
    }
}

impl Strokes for Keyboard {
    /// Upload `map` for the following keys. niri sends it to clients with
    /// the next key.
    fn upload(&mut self, map: &str) -> Result<()> {
        let file = keymap_file(map)?;
        let size = u32::try_from(map.len() + 1)
            .ok()
            .filter(|size| *size <= MAX_KEYMAP)
            .context("the extended keymap is too large")?;
        self.device.keymap(1, file.as_fd(), size);
        self.extended = true;
        Ok(())
    }

    fn stroke(&mut self, key: Key, group: u32, deadline: Instant) -> Result<()> {
        // Nothing goes out once the tool call is over (see
        // `crate::input::action`).
        let (device, queue) = (&self.device, &self.queue);
        crate::input::action::admit(|| {
            device.modifiers(key.modifiers, 0, 0, key.group);
            device.key(super::super::event_time_ms(), key.code, KEY_PRESSED);
            device.key(super::super::event_time_ms(), key.code, KEY_RELEASED);
            device.modifiers(0, 0, 0, group);
            queue.flush()
        })?
        .context("flush to niri")?;
        self.pressed.push(key.code);
        super::super::bounded_roundtrip(
            &self.connection,
            &mut self.queue,
            &mut self.state,
            deadline,
        )?;
        self.pressed.clear();
        Ok(())
    }
}

fn keymap_file(map: &str) -> Result<File> {
    let name = std::ffi::CString::new("cua-driver-niri-keymap")?;
    // SAFETY: memfd_create with a valid C string; the fd is owned below.
    let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: fd is a freshly created, owned memfd.
    let mut file = unsafe { <File as std::os::fd::FromRawFd>::from_raw_fd(fd) };
    file.write_all(map.as_bytes())?;
    file.write_all(&[0])?;
    file.seek(SeekFrom::Start(0))?;
    Ok(file)
}

fn read_keymap(fd: OwnedFd, size: u32) -> Result<Keymap, String> {
    if size == 0 || size > MAX_KEYMAP {
        return Err(format!(
            "niri's keymap size {size} is outside 1..={MAX_KEYMAP}"
        ));
    }
    let file = File::from(fd);
    let mut bytes = vec![0; size as usize];
    file.read_exact_at(&mut bytes, 0)
        .map_err(|error| format!("read niri's keymap: {error}"))?;
    if bytes.pop() != Some(0) {
        return Err("niri's keymap lacks its NUL terminator".into());
    }
    let text = String::from_utf8(bytes).map_err(|error| format!("niri's keymap: {error}"))?;
    Ok(Keymap { file, size, text })
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        handle: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            if interface == wl_seat::WlSeat::interface().name {
                state
                    .seats
                    .add(registry.bind(name, version.min(7), handle, ()));
            } else if interface == ZwpVirtualKeyboardManagerV1::interface().name {
                state.manager = Some(registry.bind(name, 1, handle, ()));
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Name { name } = event {
            state.seats.name(seat, name);
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Keymap { format, fd, size } = event {
            if format != WEnum::Value(wl_keyboard::KeymapFormat::XkbV1) {
                state.error = Some("niri sent a keymap that is not xkb v1".into());
                return;
            }
            match read_keymap(fd, size) {
                Ok(keymap) => state.keymap = Some(keymap),
                Err(error) => state.error = Some(error),
            }
        }
    }
}

impl super::super::Syncs for State {
    fn synced(&mut self) -> &mut bool {
        &mut self.synced
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.synced = true;
        }
    }
}

wayland_client::delegate_noop!(State: ZwpVirtualKeyboardManagerV1);
wayland_client::delegate_noop!(State: ZwpVirtualKeyboardV1);

#[cfg(test)]
mod tests {
    use super::*;

    /// niri 26.04's keymap for `layout "us,de"`, as its `wl_keyboard` sends it.
    const US_DE: &str = include_str!("../../../tests/fixtures/niri-us-de.xkb");

    // Evdev codes `wtype` gives the first, 14th and 28th distinct characters
    // of a call, which Chromium reads as Escape, Backspace and Enter.
    const ESC: u32 = 1;
    const BACKSPACE: u32 = 14;
    const ENTER: u32 = 28;

    fn codes(group: u32, text: &str) -> Vec<u32> {
        let parts = plan_text(US_DE, group, text).unwrap();
        assert!(
            parts.iter().all(|part| part.extended.is_none()),
            "{text:?} is all in layout {group}"
        );
        parts
            .iter()
            .flat_map(|part| part.keys.iter().map(|key| key.code))
            .collect()
    }

    #[test]
    fn every_character_goes_out_on_its_own_key_in_the_layout() {
        // `wtype` would put `.` on code 14 (Backspace) here, deleting the m.
        let typed = codes(0, "abcdefghijklm.n");
        assert_eq!(
            typed,
            [30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 52, 49]
        );
        // A second call starting with a space: `wtype` puts it on code 1
        // (Escape), so Chromium drops it.
        let typed = codes(0, " a. b, c");
        assert_eq!(typed, [57, 30, 52, 57, 48, 51, 57, 46]);
        assert!(!typed.contains(&ESC) && !typed.contains(&BACKSPACE) && !typed.contains(&ENTER));
    }

    #[test]
    fn text_uses_the_active_layout_and_its_shift_level() {
        let shift = plan_text(US_DE, 0, "Z").unwrap()[0].keys[0].modifiers;
        assert_ne!(shift, 0);
        // German swaps Y and Z.
        assert_eq!(codes(0, "yz"), [21, 44]);
        assert_eq!(codes(1, "yz"), [44, 21]);
        assert_eq!(codes(1, "ä"), [40]);
    }

    #[test]
    fn line_feed_and_tab_are_the_return_and_tab_keys() {
        assert_eq!(codes(0, "a\nb\t"), [30, ENTER, 48, 15]);
    }

    #[test]
    fn a_control_character_refuses_before_anything_is_typed() {
        let error = plan_text(US_DE, 0, "ok\u{7}").unwrap_err();
        assert_eq!(
            crate::input::foreground::error_code(&error),
            Some(crate::input::foreground::CODE_UNAVAILABLE)
        );
        assert!(error.to_string().ends_with("nothing was typed"), "{error}");
    }

    #[test]
    fn a_layout_niri_does_not_have_refuses() {
        assert!(plan_text(US_DE, 2, "a").is_err());
    }

    #[test]
    fn a_symbol_the_layout_lacks_goes_on_a_spare_key_and_every_other_key_stays() {
        let original = compile(US_DE).unwrap();
        let [part] = <[Part; 1]>::try_from(plan_text(US_DE, 0, "a→😀").unwrap()).unwrap();
        let extended = compile(part.extended.as_deref().unwrap()).unwrap();
        assert_eq!(part.keys[0].code, 30);
        let state = xkb::State::new(&extended);
        for (key, c) in part.keys.iter().zip("a→😀".chars()).skip(1) {
            let code = xkb::Keycode::new(key.code + 8);
            assert!(evdev_in(code, &CHROMIUM_CODES), "{c} on code {}", key.code);
            assert_eq!(state.key_get_one_sym(code), text_sym(c).unwrap());
        }
        let used: Vec<xkb::Keycode> = part.keys[1..]
            .iter()
            .map(|key| xkb::Keycode::new(key.code + 8))
            .collect();
        assert!(unchanged(&original, &extended, &used));
    }

    #[test]
    fn a_symbol_only_on_a_key_chromium_drops_goes_on_a_spare_key() {
        // US has `EuroSign` only on <I443> (evdev 435): past Xwayland's last
        // keycode and without a DOM code, so Chromium dropped the €.
        let [part] = <[Part; 1]>::try_from(plan_text(US_DE, 0, "a€").unwrap()).unwrap();
        assert!(part.extended.is_some());
        let euro = xkb::Keycode::new(part.keys[1].code + 8);
        assert!(
            chromium_receives(part.keys[1].code),
            "€ on code {}",
            part.keys[1].code
        );
        let extended = compile(part.extended.as_deref().unwrap()).unwrap();
        assert_eq!(
            xkb::State::new(&extended).key_get_one_sym(euro),
            text_sym('€').unwrap()
        );
        // German has it on AltGr+E, which every client receives.
        let [part] = <[Part; 1]>::try_from(plan_text(US_DE, 1, "€").unwrap()).unwrap();
        assert!(part.extended.is_none());
        assert_eq!(part.keys[0].code, 18);
    }

    #[test]
    fn more_missing_symbols_than_spare_keys_take_several_keymaps_in_order() {
        let spare = spare_keys(&compile(US_DE).unwrap()).len();
        let text: String = (0..=spare as u32)
            .map(|i| char::from_u32(0x2190 + i).unwrap())
            .collect();
        let parts = plan_text(US_DE, 0, &text).unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(
            parts.iter().map(|part| part.keys.len()).sum::<usize>(),
            spare + 1
        );
        assert!(parts.iter().all(|part| part.extended.is_some()));
    }

    fn ctrl() -> u32 {
        let keymap = compile(US_DE).unwrap();
        1 << keymap.mod_get_index(xkb::MOD_NAME_CTRL)
    }

    #[test]
    fn a_chord_goes_out_on_the_layouts_key_with_its_modifier() {
        let copy = plan_key(US_DE, 0, &["ctrl".into()], "c").unwrap();
        assert_eq!((copy.code, copy.modifiers, copy.group), (46, ctrl(), 0));
        // German has Z where US has Y.
        let undo = plan_key(US_DE, 1, &["ctrl".into()], "z").unwrap();
        assert_eq!((undo.code, undo.group), (21, 1));
        let enter = plan_key(US_DE, 0, &[], "Return").unwrap();
        assert_eq!((enter.code, enter.modifiers), (ENTER, 0));
    }

    #[test]
    fn a_key_the_active_layout_lacks_goes_out_in_one_that_has_it() {
        // German has no plain `grave` keysym (only a dead key).
        let grave = plan_key(US_DE, 1, &[], "grave").unwrap();
        assert_eq!((grave.code, grave.group), (41, 0));
    }

    #[test]
    fn an_unknown_key_or_modifier_refuses() {
        for error in [
            plan_key(US_DE, 0, &[], "NoSuchKey").unwrap_err(),
            plan_key(US_DE, 0, &["hyper".into()], "a").unwrap_err(),
        ] {
            assert_eq!(
                crate::input::foreground::error_code(&error),
                Some(crate::input::foreground::CODE_UNAVAILABLE),
                "{error}"
            );
        }
    }

    #[derive(Default)]
    struct Recorder {
        events: Vec<String>,
        /// The stroke whose round trip fails, as when niri does not answer.
        fails: Option<usize>,
    }

    impl Strokes for Recorder {
        fn upload(&mut self, _: &str) -> Result<()> {
            self.events.push("keymap".into());
            Ok(())
        }
        fn stroke(&mut self, key: Key, _: u32, _: Instant) -> Result<()> {
            let index = self.events.len();
            self.events.push(key.code.to_string());
            anyhow::ensure!(
                self.fails != Some(index),
                "niri did not answer a Wayland round trip in time"
            );
            Ok(())
        }
    }

    fn later() -> Instant {
        Instant::now() + Duration::from_secs(60)
    }

    #[test]
    fn an_extended_keymap_goes_up_before_its_first_key() {
        let parts = plan_text(US_DE, 0, "a→").unwrap();
        let mut recorder = Recorder::default();
        let mut sent = Sent::default();
        type_parts(&mut recorder, &parts, 0, later(), &mut sent, |_| Ok(())).unwrap();
        assert_eq!(recorder.events[0], "keymap");
        assert_eq!(recorder.events.len(), 3);
        assert_eq!(sent.done, 2);
    }

    #[test]
    fn typing_stops_when_focus_leaves_and_reports_the_partial_effect() {
        let parts = plan_text(US_DE, 0, "hello").unwrap();
        let mut recorder = Recorder::default();
        let mut sent = Sent::default();
        let mut checks = 0;
        let typed = type_parts(&mut recorder, &parts, 0, later(), &mut sent, |_| {
            checks += 1;
            anyhow::ensure!(
                checks <= 2,
                "foreground_unavailable: niri keyboard focus moved from window 1 to window 2"
            );
            Ok(())
        });
        assert_eq!(recorder.events, ["35", "18"]);
        let stopped = outcome(sent, 5, typed, Ok(())).unwrap_err();
        assert_eq!(
            stopped.to_string(),
            "type_text stopped after 2 of 5 characters: foreground_unavailable: niri keyboard \
             focus moved from window 1 to window 2"
        );
        assert_eq!(crate::input::foreground::error_code(&stopped), None);

        // Focus gone before the first key: nothing went out.
        let mut sent = Sent::default();
        let typed = type_parts(&mut recorder, &parts, 0, later(), &mut sent, |_| {
            anyhow::bail!("foreground_unavailable: niri keyboard focus moved")
        });
        let refused = outcome(sent, 5, typed, Ok(())).unwrap_err();
        assert_eq!(
            crate::input::foreground::error_code(&refused),
            Some(crate::input::foreground::CODE_UNAVAILABLE)
        );
    }

    #[test]
    fn a_focus_move_after_some_keys_says_the_last_may_have_reached_the_new_window() {
        let before = focus_moved(3, Some(4), 0).to_string();
        assert!(!before.contains("may have reached"), "{before}");
        let after = outcome(
            Sent {
                done: 7,
                underway: false,
            },
            400,
            Err(focus_moved(3, Some(4), 7)),
            Ok(()),
        )
        .unwrap_err();
        assert!(
            after
                .to_string()
                .ends_with("the last key sent may have reached window 4"),
            "{after}"
        );
    }

    #[test]
    fn a_window_scoped_call_types_only_into_the_window_it_activated() {
        assert_eq!(admitted_target(Some(5), Some(5)).unwrap(), 5);
        assert_eq!(admitted_target(None, Some(6)).unwrap(), 6);
        for (expected, focused) in [(Some(5), Some(6)), (Some(5), None), (None, None)] {
            let error = admitted_target(expected, focused).unwrap_err();
            assert_eq!(
                crate::input::foreground::error_code(&error),
                Some(crate::input::foreground::CODE_UNAVAILABLE),
                "{error}"
            );
            assert!(error.to_string().ends_with("nothing was typed"), "{error}");
        }
        assert_eq!(
            admitted_target(Some(5), Some(6)).unwrap_err().to_string(),
            "foreground_unavailable: niri keyboard focus is on window 6, not window 5; \
             nothing was typed"
        );
    }

    #[test]
    fn a_call_ends_in_the_layout_active_when_it_ends() {
        assert_eq!(closing_group(Ok(1), 0), 1);
        assert_eq!(closing_group(Err(anyhow!("niri did not answer")), 1), 1);
    }

    #[test]
    fn a_stroke_that_fails_after_sending_says_its_key_may_have_gone_out() {
        let parts = plan_text(US_DE, 0, "hello").unwrap();
        for (fails, message) in [
            (
                2,
                "type_text stopped after 2 of 5 characters, and character 3 may have gone out: \
                 niri did not answer a Wayland round trip in time",
            ),
            (
                0,
                "type_text stopped after 0 of 5 characters, and character 1 may have gone out: \
                 niri did not answer a Wayland round trip in time",
            ),
        ] {
            let mut recorder = Recorder {
                fails: Some(fails),
                ..Recorder::default()
            };
            let mut sent = Sent::default();
            let typed = type_parts(&mut recorder, &parts, 0, later(), &mut sent, |_| Ok(()));
            let stopped = outcome(sent, 5, typed, Ok(())).unwrap_err();
            assert_eq!(stopped.to_string(), message);
            assert_eq!(crate::input::foreground::error_code(&stopped), None);
        }
    }

    #[test]
    fn typing_stops_and_cleans_up_before_its_call_times_out() {
        // A call that waited for another's focus transaction has less time
        // left than the keyboard's own budget. The keyboard must stop, put
        // niri's keymap back and say how far it got before the call's
        // timeout replaces that with a bare foreground_timeout.
        let now = Instant::now();
        let call = now + Duration::from_secs(5);
        assert!(session_deadline(now, 1_000, Some(call)) + CLEANUP_TIMEOUT < call);
        // With time to spare, the keyboard keeps its own budget.
        let far = now + Duration::from_secs(600);
        assert_eq!(session_deadline(now, 10, Some(far)), now + call_budget(10));
        assert_eq!(session_deadline(now, 10, None), now + call_budget(10));
    }

    #[test]
    fn typing_stops_at_the_deadline() {
        let parts = plan_text(US_DE, 0, "hello").unwrap();
        let mut recorder = Recorder::default();
        let mut sent = Sent::default();
        let typed = type_parts(&mut recorder, &parts, 0, Instant::now(), &mut sent, |_| {
            Ok(())
        });
        assert!(recorder.events.is_empty());
        assert!(typed.unwrap_err().to_string().contains("ran out of time"));
    }
}
