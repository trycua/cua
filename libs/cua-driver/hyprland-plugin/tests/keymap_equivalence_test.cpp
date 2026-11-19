#include "keymap_equivalence.hpp"

#include <cstdio>
#include <cstdlib>
#include <fstream>
#include <iterator>
#include <string>

using namespace cua::hyprland;

namespace {

void check(bool value, const char* what) {
    if (!value) {
        std::fprintf(stderr, "FAILED: %s\n", what);
        std::abort();
    }
}

xkb_keymap* compile(xkb_context* context, const char* layout, const char* variant, const char* options) {
    const xkb_rule_names names{"evdev", "pc105", layout, variant, options};
    return xkb_keymap_new_from_names(context, &names, XKB_KEYMAP_COMPILE_NO_FLAGS);
}

// A US keymap with hand-written key overrides, for remaps no stock option makes.
xkb_keymap* custom(xkb_context* context, const char* symbols, const char* types = "") {
    const std::string text = std::string("xkb_keymap { xkb_keycodes { include \"evdev+aliases(qwerty)\" }; ") +
        "xkb_types { include \"complete\" " + types + " }; xkb_compat { include \"complete\" }; " +
        "xkb_symbols { include \"pc+us+inet(evdev)\" " + symbols + " }; };";
    return xkb_keymap_new_from_string(context, text.c_str(), XKB_KEYMAP_FORMAT_TEXT_V1, XKB_KEYMAP_COMPILE_NO_FLAGS);
}

struct Case {
    const char* layout;
    const char* variant;
    const char* options;
    bool equivalent;
    const char* what;
};

}

// With a path argument, reports whether that keymap (for example the output of
// `xkbcli dump-keymap-wayland`) qualifies for foreground typing.
int main(int argc, char** argv) {
    check(foreground_lock_key(58) && foreground_lock_key(69) && foreground_lock_key(70), "lock keys");
    check(foreground_modifier_key(42) && !foreground_modifier_key(30), "modifier keys");
    check(foreground_text_key(30) && foreground_text_key(28) && foreground_text_key(57), "text keys");
    check(!foreground_text_key(42) && !foreground_text_key(29) && !foreground_text_key(86) && !foreground_text_key(1),
          "non-text keys");
    auto* context = xkb_context_new(XKB_CONTEXT_NO_FLAGS);
    auto* us = context ? compile(context, "us", "", "") : nullptr;
    if (!us) {
        check(!std::getenv("CUA_HYPRLAND_REQUIRE_XKBCOMMON"), "XKB data unavailable");
        std::puts("SKIP: XKB data unavailable");
        return 77;
    }
    if (argc > 1) {
        std::ifstream file(argv[1]);
        const std::string text{std::istreambuf_iterator<char>(file), std::istreambuf_iterator<char>()};
        auto* map = xkb_keymap_new_from_string(context, text.c_str(), XKB_KEYMAP_FORMAT_TEXT_V1,
            XKB_KEYMAP_COMPILE_NO_FLAGS);
        check(map != nullptr, "keymap file compiles");
        const bool equivalent = typing_keymap_equivalent(map, us);
        std::puts(equivalent ? "equivalent" : "different");
        xkb_keymap_unref(map);
        xkb_keymap_unref(us);
        xkb_context_unref(context);
        return equivalent ? 0 : 1;
    }
    const Case cases[] = {
        {"us", "", "", true, "canonical us"},
        {"us", "", "compose:caps,shift:both_capslock_cancel", true, "Omarchy default options"},
        {"us", "", "caps:escape", true, "Caps Lock remap"},
        {"de", "", "", false, "German layout"},
        {"us", "dvorak", "", false, "Dvorak variant"},
        {"us", "", "altwin:swap_alt_win", true, "Alt/Super swap"},
        {"us", "", "ctrl:swapcaps", true, "Ctrl/Caps swap"},
        {"us", "", "ctrl:swap_lalt_lctl", true, "Alt/Ctrl swap"},
        {"us", "", "compose:ralt", true, "Compose on Right Alt"},
        {"us", "", "compose:menu", true, "Compose on Menu"},
        {"us", "", "compose:102", true, "Compose on the 102nd key"},
        {"us", "", "lv3:lsgt_switch", true, "Level 3 on the 102nd key"},
        {"us", "", "keypad:oss", true, "keypad operators"},
        {"us", "", "caps:swapescape", true, "Caps/Escape swap"},
        {"us", "", "compose:caps,shift:both_capslock_cancel,grp:alts_toggle", true, "Omarchy non-Latin options"},
        {"us,ru", "", "grp:alts_toggle", true, "Omarchy non-Latin layout pair"},
        {"us", "intl", "", false, "US international dead keys"},
        {"fr", "", "", false, "French layout"},
        {"us", "colemak", "", false, "Colemak variant"},
    };
    for (const auto& test : cases) {
        auto* map = compile(context, test.layout, test.variant, test.options);
        check(map != nullptr, test.what);
        check(typing_keymap_equivalent(map, us) == test.equivalent, test.what);
        xkb_keymap_unref(map);
    }
    const struct { const char* symbols; bool equivalent; const char* what; } overrides[] = {
        {"", true, "unmodified custom keymap"},
        {"key <AE01> { [ 1, at ] };", false, "Shift level of a digit"},
        {"key <LFSH> { [ Shift_R ] };", false, "Shift key symbol"},
        {"key <RTRN> { [ Multi_key ] };", false, "Return as Compose"},
        {"key <TAB> { [ dead_acute ] };", false, "Tab as a dead key"},
        {"key <LSGT> { [ Multi_key ] };", true, "key text never uses"},
    };
    for (const auto& test : overrides) {
        auto* map = custom(context, test.symbols);
        check(map != nullptr, test.what);
        check(typing_keymap_equivalent(map, us) == test.equivalent, test.what);
        xkb_keymap_unref(map);
    }
    // A letter whose level changes under Num Lock alone must refuse up front, not at that letter.
    auto* numtext = custom(context, "key <AB05> { type=\"NUMTEXT\", [ b, B, x, X ] };",
        "type \"NUMTEXT\" { modifiers = Shift+Mod2; map[None] = Level1; map[Shift] = Level2; "
        "map[Mod2] = Level3; map[Shift+Mod2] = Level4; };");
    check(numtext != nullptr, "Num Lock text type compiles");
    check(!typing_keymap_equivalent(numtext, us), "letter changed by Num Lock");
    xkb_keymap_unref(numtext);
    check(!typing_keymap_equivalent(nullptr, us), "missing keymap");
    xkb_keymap_unref(us);
    xkb_context_unref(context);
    return 0;
}
