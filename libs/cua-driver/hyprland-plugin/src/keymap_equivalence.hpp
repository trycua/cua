#pragma once

// The chord comparison below (foreground_numlock_mask through
// foreground_chord_failure) is adapted from omacom/omarchy-pkgs#473,
// pkgbuilds/cua-hyprland-plugin/independent-keymaps.patch, by Spencer Bull.
// Copyright (c) David Heinemeier Hansson. Used under the MIT License, whose
// permission notice is the same as this repository's LICENSE.md.

#include "foreground_route.hpp"

#include <xkbcommon/xkbcommon.h>
#include <xkbcommon/xkbcommon-names.h>

#include <array>
#include <cstdint>
#include <memory>

namespace cua::hyprland {

// Lock keys are refused by the KEY command itself, so they never need to match.
inline constexpr bool foreground_lock_key(std::uint32_t code) {
    return code == 58 || code == 69 || code == 70;
}

inline constexpr bool foreground_modifier_key(std::uint32_t code) {
    switch (code) {
    case 29: case 42: case 54: case 56: case 97: case 100: case 125: case 126:
        return true;
    default:
        return false;
    }
}

// True when typing through `physical` produces the same keysyms as `canonical`
// for every key the foreground KEY command can press (evdev 1-247, minus the
// lock keys): at the base level, and with Shift held for non-modifier keys.
// Options that only change Caps Lock or modifier chords, such as compose:caps
// and shift:both_capslock_cancel, qualify. A different layout, or a remap of a
// letter, digit, punctuation, or modifier key, does not. This compares fresh
// states, so the human keyboard's current locks do not affect it.
inline bool typing_keymap_equivalent(xkb_keymap* physical, xkb_keymap* canonical) {
    if (!physical || !canonical) return false;
    auto* actual = xkb_state_new(physical);
    auto* expected = xkb_state_new(canonical);
    bool equal = actual && expected;
    constexpr xkb_keycode_t left_shift = 42 + 8;
    for (int shifted = 0; equal && shifted < 2; ++shifted) {
        if (shifted) {
            xkb_state_update_key(actual, left_shift, XKB_KEY_DOWN);
            xkb_state_update_key(expected, left_shift, XKB_KEY_DOWN);
        }
        for (std::uint32_t code = 1; equal && code <= 247; ++code) {
            if (foreground_lock_key(code) || (shifted && foreground_modifier_key(code))) continue;
            equal = xkb_state_key_get_one_sym(actual, code + 8) == xkb_state_key_get_one_sym(expected, code + 8);
        }
    }
    if (actual) xkb_state_unref(actual);
    if (expected) xkb_state_unref(expected);
    return equal;
}

using ForegroundXkbState = std::unique_ptr<xkb_state, decltype(&xkb_state_unref)>;

inline ForegroundXkbState foreground_xkb_state(xkb_keymap* map) {
    return {map ? xkb_state_new(map) : nullptr, xkb_state_unref};
}

inline std::uint32_t foreground_caps_mask(xkb_keymap* map) {
    const auto index = map ? xkb_keymap_mod_get_index(map, XKB_MOD_NAME_CAPS) : XKB_MOD_INVALID;
    return index < 32 ? std::uint32_t{1} << index : 0;
}

// Resolve Num Lock through this map's own Num Lock key rather than assuming
// Mod2. An encoding that is not a single modifier, or that overlaps Caps Lock,
// is not admitted.
inline std::uint32_t foreground_numlock_mask(xkb_keymap* map) {
    auto state = foreground_xkb_state(map);
    if (!state) return 0;
    constexpr xkb_keycode_t num_lock = 69 + 8;
    xkb_state_update_key(state.get(), num_lock, XKB_KEY_DOWN);
    xkb_state_update_key(state.get(), num_lock, XKB_KEY_UP);
    const auto mask = xkb_state_serialize_mods(state.get(), XKB_STATE_MODS_LOCKED);
    return mask && !(mask & (mask - 1)) && !(mask & foreground_caps_mask(map)) ? mask : 0;
}

inline bool foreground_same_key_symbols(xkb_state* actual, xkb_state* expected, xkb_keycode_t key) {
    const xkb_keysym_t *a = nullptr, *b = nullptr;
    const int na = xkb_state_key_get_syms(actual, key, &a);
    const int nb = xkb_state_key_get_syms(expected, key, &b);
    if (na != nb) return false;
    for (int i = 0; i < na; ++i) if (a[i] != b[i]) return false;
    return true;
}

// Toolkits subtract consumed modifiers when matching shortcuts, so equal
// symbols and modifier state alone do not preserve a shortcut's meaning.
inline bool foreground_same_consumed_modifiers(xkb_state* actual, xkb_state* expected, xkb_keycode_t key) {
    for (auto* state : {actual, expected}) {
        auto* map = xkb_state_get_keymap(state);
        for (xkb_mod_index_t i = 0; i < xkb_keymap_num_mods(map); ++i) {
            const char* name = xkb_keymap_mod_get_name(map, i);
            if (!name || xkb_state_mod_name_is_active(state, name, XKB_STATE_MODS_EFFECTIVE) <= 0) continue;
            const auto ai = xkb_keymap_mod_get_index(xkb_state_get_keymap(actual), name);
            const auto bi = xkb_keymap_mod_get_index(xkb_state_get_keymap(expected), name);
            for (auto mode : {XKB_CONSUMED_MODE_XKB, XKB_CONSUMED_MODE_GTK})
                if ((xkb_state_mod_index_is_consumed2(actual, key, ai, mode) > 0) !=
                    (xkb_state_mod_index_is_consumed2(expected, key, bi, mode) > 0)) return false;
        }
    }
    return true;
}

// Modifier indices can differ between maps. Compare names in both directions,
// including nonstandard modifiers and lock or latch changes on key release.
inline bool foreground_same_modifier_state(xkb_state* actual, xkb_state* expected) {
    for (auto* state : {actual, expected}) {
        auto* map = xkb_state_get_keymap(state);
        for (xkb_mod_index_t i = 0; i < xkb_keymap_num_mods(map); ++i) {
            const char* name = xkb_keymap_mod_get_name(map, i);
            if (!name) continue;
            for (auto component : {XKB_STATE_MODS_DEPRESSED, XKB_STATE_MODS_LATCHED,
                                   XKB_STATE_MODS_LOCKED, XKB_STATE_MODS_EFFECTIVE}) {
                const bool a = xkb_state_mod_name_is_active(actual, name, component) > 0;
                const bool b = xkb_state_mod_name_is_active(expected, name, component) > 0;
                if (a != b) return false;
            }
        }
    }
    for (auto component : {XKB_STATE_LAYOUT_DEPRESSED, XKB_STATE_LAYOUT_LATCHED,
                           XKB_STATE_LAYOUT_LOCKED, XKB_STATE_LAYOUT_EFFECTIVE})
        if (xkb_state_serialize_layout(actual, component) != xkb_state_serialize_layout(expected, component))
            return false;
    return true;
}

// Simulate a complete KEY chord (modifiers, key press and release, modifier
// release) before any event is delivered or focus changes. The human keyboard's
// modifiers and locks stay as they are; the chord is admitted only when it means
// under the live keymap and lock state what the wire chord means on a neutral
// canonical US keyboard. This compares meaning; it never translates key
// positions. Three states are compared:
//   actual:   the live physical keymap with the human keyboard's locks;
//   expected: the canonical keymap with the same (Num Lock) lock;
//   intended: the canonical keymap in the neutral state the wire assumes.
// actual differing from expected is a layout difference. expected differing
// from intended can only come from the admitted Num Lock, which on the
// canonical keymap changes only keypad keys.
inline ForegroundFailureReason foreground_chord_failure(xkb_keymap* physical, xkb_keymap* canonical,
                                                        std::uint32_t code, std::uint32_t mods,
                                                        const std::array<std::uint32_t, 4>& modifiers) {
    if (!physical || !canonical) return ForegroundFailureReason::keyboard_state;
    const auto state_failure = foreground_key_modifier_failure(modifiers, foreground_numlock_mask(physical),
        foreground_caps_mask(physical));
    if (state_failure != ForegroundFailureReason::none) return state_failure;
    auto actual = foreground_xkb_state(physical);
    auto expected = foreground_xkb_state(canonical);
    auto intended = foreground_xkb_state(canonical);
    if (!actual || !expected || !intended) return ForegroundFailureReason::keyboard_state;
    if (modifiers[2]) {
        const auto canonical_lock = foreground_numlock_mask(canonical);
        if (!canonical_lock) return ForegroundFailureReason::keyboard_locked;
        xkb_state_update_mask(actual.get(), 0, 0, modifiers[2], 0, 0, 0);
        xkb_state_update_mask(expected.get(), 0, 0, canonical_lock, 0, 0, 0);
        if (!foreground_same_modifier_state(actual.get(), expected.get()))
            return ForegroundFailureReason::unsupported_layout;
    }
    auto failure = ForegroundFailureReason::none;
    const auto event = [&](std::uint32_t key, xkb_key_direction direction) {
        const xkb_keycode_t keycode = key + 8;
        // A client interprets a press using the preceding modifiers event.
        // Stock shift:both_capslock_cancel gives Shift a Caps_Lock symbol at
        // its shifted level; that level is not another press. Releases pair
        // by keycode, but their lock, latch, and group effects must agree.
        if (direction == XKB_KEY_DOWN) {
            if (!foreground_same_key_symbols(actual.get(), expected.get(), keycode) ||
                !foreground_same_consumed_modifiers(actual.get(), expected.get(), keycode)) {
                failure = ForegroundFailureReason::unsupported_layout;
                return false;
            }
            if (!foreground_same_key_symbols(expected.get(), intended.get(), keycode) ||
                !foreground_same_consumed_modifiers(expected.get(), intended.get(), keycode)) {
                failure = ForegroundFailureReason::keyboard_numlock_keypad;
                return false;
            }
        }
        xkb_state_update_key(actual.get(), keycode, direction);
        xkb_state_update_key(expected.get(), keycode, direction);
        xkb_state_update_key(intended.get(), keycode, direction);
        if (!foreground_same_modifier_state(actual.get(), expected.get())) {
            failure = ForegroundFailureReason::unsupported_layout;
            return false;
        }
        return true;
    };
    constexpr std::array<std::uint32_t, 4> keys{42, 29, 56, 125};
    for (unsigned i = 0; i < keys.size(); ++i)
        if ((mods & (1u << i)) && keys[i] != code && !event(keys[i], XKB_KEY_DOWN)) return failure;
    if (!event(code, XKB_KEY_DOWN) || !event(code, XKB_KEY_UP)) return failure;
    for (int i = 3; i >= 0; --i)
        if ((mods & (1u << i)) && keys[i] != code && !event(keys[i], XKB_KEY_UP)) return failure;
    return ForegroundFailureReason::none;
}

} // namespace cua::hyprland
