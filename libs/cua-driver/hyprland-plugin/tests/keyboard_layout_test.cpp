// Adapted from tests/keyboard_layout_test.cpp in omacom/omarchy-pkgs#473
// (pkgbuilds/cua-hyprland-plugin/independent-keymaps.patch), by Spencer Bull.
// Copyright (c) David Heinemeier Hansson. Used under the MIT License, whose
// permission notice is the same as this repository's LICENSE.md.
#include "keymap_equivalence.hpp"

#include <cstdio>
#include <cstdlib>
#include <iostream>
#include <memory>
#include <string>

using namespace cua::hyprland;
namespace {
void check(bool condition, const char* message) {
    if (!condition) { std::cerr << "FAILED: " << message << '\n'; std::exit(1); }
}
using Reason = ForegroundFailureReason;
using Mods = std::array<std::uint32_t, 4>;
}

int main() {
    using Context = std::unique_ptr<xkb_context, decltype(&xkb_context_unref)>;
    using Map = std::unique_ptr<xkb_keymap, decltype(&xkb_keymap_unref)>;
    Context context{xkb_context_new(XKB_CONTEXT_NO_FLAGS), xkb_context_unref};
    const auto map = [&](const char* layout, const char* variant, const char* options) {
        const xkb_rule_names names{"evdev", "pc105", layout, variant, options};
        return Map{context ? xkb_keymap_new_from_names(context.get(), &names, XKB_KEYMAP_COMPILE_NO_FLAGS) : nullptr,
            xkb_keymap_unref};
    };
    const xkb_rule_names agent_names{kAgentKeymap.rules.data(), kAgentKeymap.model.data(),
        kAgentKeymap.layout.data(), kAgentKeymap.variant.data(), kAgentKeymap.options.data()};
    Map agent{context ? xkb_keymap_new_from_names(context.get(), &agent_names, XKB_KEYMAP_COMPILE_NO_FLAGS) : nullptr,
        xkb_keymap_unref};
    if (!agent) {
        if (std::getenv("CUA_HYPRLAND_REQUIRE_XKBCOMMON")) check(false, "XKB data unavailable");
        std::puts("SKIP: XKB data unavailable");
        return 77;
    }
    const auto chord = [&](xkb_keymap* physical, std::uint32_t code, std::uint32_t mods, const Mods& state) {
        return foreground_chord_failure(physical, agent.get(), code, mods, state);
    };
    auto stock = map("us", "", "compose:caps,shift:both_capslock_cancel");
    auto nocaps = map("us", "", "ctrl:nocaps");
    auto swapctrl = map("us", "", "ctrl:swapcaps");
    auto swapalt = map("us", "", "altwin:swap_alt_win");
    auto german = map("de", "", "");
    check(stock && nocaps && swapctrl && swapalt && german, "test keymaps unavailable");

    // Hyprland's numlock_by_default locks Mod2; the resolved mask must agree.
    for (auto* physical : {agent.get(), stock.get(), nocaps.get(), german.get()}) {
        check(foreground_numlock_mask(physical) == 0x10, "Num Lock does not resolve to Mod2");
        check(foreground_caps_mask(physical) == 0x2, "Caps Lock does not resolve to Lock");
    }
    check(foreground_numlock_mask(nullptr) == 0 && foreground_caps_mask(nullptr) == 0, "missing map resolved a lock");

    // Omarchy's default options pass the up-front typing qualification.
    check(typing_keymap_equivalent(stock.get(), agent.get()), "Omarchy default options refused");
    check(typing_keymap_equivalent(nocaps.get(), agent.get()), "Caps to Ctrl refused");

    // The chord check compares meaning only: unrelated remaps are harmless,
    // requested remaps fail closed.
    for (auto* physical : {stock.get(), nocaps.get(), swapctrl.get(), swapalt.get(), german.get()}) {
        check(chord(physical, 30, 0, {}) == Reason::none, "unrelated remap blocked A");
        check(chord(physical, 30, 1, {}) == Reason::none, "unrelated remap blocked Shift+A");
        check(chord(physical, 28, 0, {}) == Reason::none, "unrelated remap blocked Return");
    }
    check(chord(stock.get(), 30, 2, {}) == Reason::none, "stock Omarchy blocked Ctrl+A");
    check(chord(nocaps.get(), 30, 2, {}) == Reason::none, "Caps to Ctrl blocked Ctrl+A");
    check(chord(swapctrl.get(), 30, 2, {}) == Reason::unsupported_layout, "Ctrl/Caps swap sent wrong Ctrl+A");
    check(chord(swapctrl.get(), 29, 0, {}) == Reason::unsupported_layout, "remapped modifier key accepted");
    check(chord(swapalt.get(), 30, 4, {}) == Reason::unsupported_layout, "Alt/Super swap sent wrong Alt+A");
    check(chord(swapalt.get(), 30, 8, {}) == Reason::unsupported_layout, "Alt/Super swap sent wrong Super+A");
    check(chord(german.get(), 21, 0, {}) == Reason::unsupported_layout, "German Z accepted as US Y");
    check(chord(german.get(), 3, 1, {}) == Reason::unsupported_layout, "German shifted punctuation accepted");
    check(chord(nullptr, 30, 0, {}) == Reason::keyboard_state, "missing physical map accepted");

    for (auto* physical : {agent.get(), stock.get(), nocaps.get()}) {
        const auto numlock = foreground_numlock_mask(physical);
        for (const auto locked : {0u, numlock}) {
            const Mods ambient{0, 0, locked, 0};
            // Driver text and shortcuts keep working with Num Lock on.
            for (const auto key : {30u, 48u, 44u, 2u, 11u, 12u, 28u, 57u, 14u, 15u, 1u, 103u, 105u, 111u})
                for (const auto mods : {0u, 1u, 2u, 3u})
                    check(chord(physical, key, mods, ambient) == Reason::none,
                          "Num Lock blocked ordinary text, navigation, or shortcut");
            // Keypad keys whose meaning Num Lock changes refuse with their own reason.
            for (const auto key : {71u, 72u, 73u, 75u, 76u, 77u, 79u, 80u, 81u, 82u, 83u})
                check(chord(physical, key, 0, ambient) == (locked ? Reason::keyboard_numlock_keypad : Reason::none),
                      "Num Lock keypad semantic change was ignored");
            // Keypad operators mean the same thing either way.
            for (const auto key : {55u, 74u, 78u, 96u, 98u})
                check(chord(physical, key, 0, ambient) == Reason::none, "Num Lock blocked a keypad operator");
        }
        const auto caps = foreground_caps_mask(physical);
        check(chord(physical, 30, 0, {0, 0, caps, 0}) == Reason::keyboard_caps_lock, "Caps Lock accepted");
        check(chord(physical, 2, 0, {0, 0, caps, 0}) == Reason::keyboard_caps_lock, "Caps Lock accepted for a digit");
        check(chord(physical, 30, 0, {0, 0, caps | numlock, 0}) == Reason::keyboard_caps_lock,
              "Caps Lock with Num Lock accepted");
        check(chord(physical, 30, 0, {0, 0, 0x80000000u, 0}) == Reason::keyboard_locked, "unknown lock accepted");
        check(chord(physical, 30, 0, {0, 0, 0x80000000u | numlock, 0}) == Reason::keyboard_locked,
              "unknown lock with Num Lock accepted");
        check(chord(physical, 30, 0, {numlock, 0, numlock, 0}) == Reason::keyboard_depressed,
              "held modifier accepted with Num Lock");
        check(chord(physical, 30, 0, {0, numlock, numlock, 0}) == Reason::keyboard_latched,
              "latched modifier accepted with Num Lock");
        check(chord(physical, 30, 0, {0, 0, numlock, 1}) == Reason::keyboard_group,
              "nonzero group accepted with Num Lock");
    }
    const auto numlock = foreground_numlock_mask(agent.get());
    check(chord(swapctrl.get(), 30, 2, {0, 0, numlock, 0}) == Reason::unsupported_layout, "Num Lock hid Ctrl remap");
    check(chord(german.get(), 21, 0, {0, 0, numlock, 0}) == Reason::unsupported_layout, "Num Lock hid layout mismatch");

    // Every supported wire chord stays compatible on the canonical map.
    for (unsigned code = 1; code <= 247; ++code)
        if (!foreground_lock_key(code))
            for (unsigned mods = 0; mods < 16; ++mods)
                check(chord(agent.get(), code, mods, {}) == Reason::none, "canonical chord rejected");

    // Equal symbols can hide a changed XKB action. Simulate an A that locks Mod3.
    char* serialized = xkb_keymap_get_as_string(agent.get(), XKB_KEYMAP_FORMAT_TEXT_V1);
    check(serialized != nullptr, "canonical keymap cannot serialize");
    const std::string canonical_text = serialized;
    std::free(serialized);
    const auto compile = [&](const std::string& text) {
        return Map{xkb_keymap_new_from_string(context.get(), text.c_str(), XKB_KEYMAP_FORMAT_TEXT_V1,
            XKB_KEYMAP_COMPILE_NO_FLAGS), xkb_keymap_unref};
    };
    std::string changed = canonical_text;
    const auto at = changed.find("key <AC01>");
    const auto end = changed.find(';', at);
    check(at != std::string::npos && end != std::string::npos, "test action fixture unavailable");
    changed.replace(at, end - at + 1,
        "key <AC01> { type=\"ALPHABETIC\", symbols[Group1]=[a,A], "
        "actions[Group1]=[LockMods(modifiers=Mod3),LockMods(modifiers=Mod3)] };");
    auto lock = compile(changed);
    check(bool(lock), "action fixture failed to compile");
    check(chord(lock.get(), 30, 0, {}) == Reason::unsupported_layout, "hidden lock action accepted");
    check(chord(lock.get(), 30, 0, {0, 0, numlock, 0}) == Reason::unsupported_layout,
          "Num Lock hid an unexpected lock action");

    // Equal symbols and modifier state can still alter toolkit shortcut matching.
    changed = canonical_text;
    const auto type_at = changed.find("type \"ALPHABETIC\"");
    const auto type_end = changed.find("};", type_at);
    check(type_at != std::string::npos && type_end != std::string::npos, "key type fixture unavailable");
    changed.insert(type_end, "preserve[Shift] = Shift;\n");
    auto preserved = compile(changed);
    check(bool(preserved), "preserved modifier fixture failed to compile");
    check(chord(preserved.get(), 30, 1, {}) == Reason::unsupported_layout, "consumed modifier mismatch accepted");
    check(chord(preserved.get(), 30, 1, {0, 0, numlock, 0}) == Reason::unsupported_layout,
          "Num Lock hid changed shortcut consumption");

    // A modifier that sets Shift but emits another symbol must fail.
    changed = canonical_text;
    const auto shift_at = changed.find("key <LFSH>");
    const auto shift_end = changed.find(';', shift_at);
    check(shift_at != std::string::npos && shift_end != std::string::npos, "shift fixture unavailable");
    changed.replace(shift_at, shift_end - shift_at + 1,
        "key <LFSH> { symbols[Group1]=[Delete], actions[Group1]=[SetMods(modifiers=Shift)] };");
    auto badshift = compile(changed);
    check(bool(badshift), "shift fixture failed to compile");
    check(chord(badshift.get(), 30, 1, {}) == Reason::unsupported_layout, "modifier press symbols not checked");

    // A keymap whose Num Lock key locks something other than one modifier
    // resolves no Num Lock, so an ambient lock refuses instead of guessing.
    changed = canonical_text;
    const auto nmlk_at = changed.find("key <NMLK>");
    const auto nmlk_end = changed.find(';', nmlk_at);
    check(nmlk_at != std::string::npos && nmlk_end != std::string::npos, "Num Lock fixture unavailable");
    changed.replace(nmlk_at, nmlk_end - nmlk_at + 1,
        "key <NMLK> { symbols[Group1]=[Num_Lock], actions[Group1]=[LockMods(modifiers=Mod2+Mod3)] };");
    auto ambiguous = compile(changed);
    check(bool(ambiguous), "ambiguous Num Lock fixture failed to compile");
    check(foreground_numlock_mask(ambiguous.get()) == 0, "ambiguous Num Lock encoding admitted");
    check(chord(ambiguous.get(), 30, 0, {0, 0, 0x10, 0}) == Reason::keyboard_locked,
          "lock accepted without a resolved Num Lock");
    std::cout << "keyboard layout tests passed\n";
}
