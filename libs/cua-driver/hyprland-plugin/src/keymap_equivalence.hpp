#pragma once

#include <xkbcommon/xkbcommon.h>

#include <cstdint>

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
// letter, digit, punctuation, or modifier key, does not.
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

}
