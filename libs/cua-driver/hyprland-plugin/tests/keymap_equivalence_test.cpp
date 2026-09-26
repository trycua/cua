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
    auto* context = xkb_context_new(XKB_CONTEXT_NO_FLAGS);
    auto* us = context ? compile(context, "us", "", "") : nullptr;
    if (!us) {
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
        {"us", "", "altwin:swap_alt_win", false, "Alt/Super swap"},
        {"us", "", "ctrl:swapcaps", false, "Ctrl/Caps swap"},
    };
    for (const auto& test : cases) {
        auto* map = compile(context, test.layout, test.variant, test.options);
        check(map != nullptr, test.what);
        check(typing_keymap_equivalent(map, us) == test.equivalent, test.what);
        xkb_keymap_unref(map);
    }
    check(!typing_keymap_equivalent(nullptr, us), "missing keymap");
    xkb_keymap_unref(us);
    xkb_context_unref(context);
    return 0;
}
