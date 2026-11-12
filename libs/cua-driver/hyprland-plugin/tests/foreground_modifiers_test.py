"""Exercise production foreground keyboard admission, dispatch and unwind with XKB.

The method bodies come from input_experiment.cpp, so a changed production call
site changes the behavior under test. The compositor, seat and client objects
are fakes: this is policy coverage, not evidence of Wayland delivery.

Adapted from tests/foreground_modifiers_test.py in omacom/omarchy-pkgs#473
(pkgbuilds/cua-hyprland-plugin/independent-keymaps.patch), by Spencer Bull.
Copyright (c) David Heinemeier Hansson. Used under the MIT License, whose
permission notice is the same as this repository's LICENSE.md.
"""
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
REQUIRED = bool(os.environ.get('CUA_HYPRLAND_REQUIRE_XKBCOMMON'))

FIXTURE = r'''
#include "keymap_equivalence.hpp"
#include <algorithm>
#include <chrono>
#include <cstdlib>
#include <iostream>
#include <memory>
#include <string>
#include <utility>
#include <vector>
using namespace cua::hyprland;
using Clock = std::chrono::steady_clock;
void check(bool ok, const char* why) { if (!ok) { std::cerr << why << '\n'; std::exit(1); } }
struct Vector2D { double x, y; };
constexpr int WL_KEYBOARD_KEY_STATE_PRESSED = 1, WL_KEYBOARD_KEY_STATE_RELEASED = 0;
constexpr int WL_POINTER_BUTTON_STATE_RELEASED = 0;
struct Root : std::enable_shared_from_this<Root> {
    bool good() const { return true; }
    int client() const { return 0; }
    auto at(Vector2D, bool) { return std::pair{shared_from_this(), 0}; }
};
struct Resource {
    xkb_state* client = nullptr;
    std::vector<xkb_keysym_t> symbols;
    std::vector<std::array<unsigned, 4>> history;
    bool good() const { return true; }
    void sendKey(unsigned, unsigned code, unsigned down) {
        if (down) symbols.push_back(xkb_state_key_get_one_sym(client, code + 8));
    }
    void sendMods(unsigned d, unsigned l, unsigned k, unsigned g) {
        history.push_back({d, l, k, g});
        xkb_state_update_mask(client, d, l, k, 0, 0, g);
    }
    void sendButton(unsigned, unsigned, unsigned) {}
    void sendFrame() {}
};
struct Keyboard {
    struct { unsigned depressed = 0, latched = 0, locked = 0, group = 0; } m_modifiersState;
    xkb_keymap* m_xkbKeymap = nullptr;
    bool m_enabled = true, shared = true, virt = false;
    bool shareStates() const { return shared; }
    bool isVirtual() const { return virt; }
};
struct Seat {
    std::vector<std::shared_ptr<Resource>> m_keyboards, m_pointers;
    bool good() const { return true; }
};
struct SeatManager {
    std::weak_ptr<Keyboard> m_keyboard;
    bool m_mouse = true;
    struct { std::shared_ptr<Root> keyboardFocus, pointerFocus; } m_state;
    std::shared_ptr<Seat> seat;
    auto seatResourceForClient(int) { return seat; }
    void setPointerFocus(std::shared_ptr<Root> root, Vector2D) { m_state.pointerFocus = root; }
} manager, *g_pSeatManager = &manager;
struct InputManager {
    std::vector<std::shared_ptr<Keyboard>> m_keyboards;
    bool shouldIgnoreVirtualKeyboard(const std::shared_ptr<Keyboard>&) { return false; }
} input, *g_pInputManager = &input;
namespace Desktop {
constexpr int FOCUS_REASON_OTHER = 0;
struct Focus {
    std::shared_ptr<Root> root;
    std::shared_ptr<Keyboard> change_on_focus;
    auto window() { return root; }
    auto surface() { return root; }
    void fullWindowFocus(std::shared_ptr<Root>, int, std::shared_ptr<Root> r) {
        root = r; manager.m_state.keyboardFocus = r;
        if (change_on_focus) change_on_focus->m_modifiersState.locked = 0;
    }
} focus;
auto focusState() { return &focus; }
}
namespace Pointer { struct Manager { void warpTo(Vector2D) {} } pointer; auto mgr() { return &pointer; } }
struct Client {
    InputRoute route = InputRoute::primary_foreground;
    bool dead = false, foreground_attempted = false;
    std::weak_ptr<Root> surface, window;
    std::array<double, 6> geometry{100, 100, 400, 300, 100, 100};
};
struct Lane {
    xkb_keymap* physical_keymap = nullptr;
    xkb_keymap* keymap = nullptr;
    xkb_state* physical_keyboard_state = nullptr;
    std::array<std::uint32_t, 4> foreground_modifiers{};
    std::vector<std::weak_ptr<Resource>> foreground_keyboards, foreground_pointers;
    std::weak_ptr<Root> foreground_surface;
    std::weak_ptr<Seat> foreground_seat;
    std::vector<std::uint32_t> held_keys;
    std::uint32_t held_button = 0;
    bool foreground_started = false, foreground_activating = false, foreground_keyboard_used = false;
    bool foreground_needs_keyboard = false, foreground_needs_pointer = false;
    bool layout_ready = true, physical_held = false;
    Client* lease = nullptr;
    Clock::time_point expires = Clock::now() + std::chrono::hours(1);
    unsigned consumed = 0, motions = 0;
    bool available() const { return true; }
    bool layout_qualified() const { return layout_ready; }
    bool point(Client&, double, double) { return true; }
    std::uint32_t event_ms() { return 0; }
    void foreground_motion(Client&, double, double) { ++motions; }
    ForegroundGuard foreground_guard(const Client&) const {
        return {.exact_root = true, .physical_keys = physical_held, .exact_keyboard_focus = true,
                .exact_pointer_focus = true};
    }
    bool consume_grant(Client&, std::uint64_t) { ++consumed; return true; }
    // METHODS
    void preflight(Client& c, const std::string& command, std::uint64_t code, std::uint64_t mods) {
        const std::uint64_t cap = command == "KEY" ? 2 : 1;
        const double x = 10, y = 10;
        // PREFLIGHT
    }
    void key(Client& c, std::uint32_t code, std::uint32_t mods) {
        preflight(c, "KEY", code, mods);
        const std::array<std::uint32_t, 4> keys{42, 29, 56, 125};
        for (unsigned i = 0; i < 4; ++i) if ((mods & (1u << i)) && keys[i] != code) foreground_key(c, keys[i], true);
        foreground_key(c, code, true); foreground_key(c, code, false);
        for (int i = 3; i >= 0; --i) if ((mods & (1u << i)) && keys[i] != code) foreground_key(c, keys[i], false);
        finish();
    }
    void finish() {
        finish_foreground();
        if (physical_keyboard_state) xkb_state_unref(physical_keyboard_state);
        physical_keyboard_state = xkb_state_new(physical_keymap);
    }
    ~Lane() { if (physical_keyboard_state) xkb_state_unref(physical_keyboard_state); }
};
ForegroundFailureReason refused(Lane& lane, Client& client, const std::string& command, unsigned code, unsigned mods) {
    client.foreground_attempted = false; // TARGET admission resets this in production.
    try { lane.preflight(client, command, code, mods); }
    catch (const ForegroundFailure& failure) { lane.finish(); return failure.reason; }
    lane.finish();
    return ForegroundFailureReason::none;
}
int main() {
    auto context = xkb_context_new(XKB_CONTEXT_NO_FLAGS);
    const xkb_rule_names names{"evdev", "pc105", "us", "", "compose:caps,shift:both_capslock_cancel"};
    auto physical = xkb_keymap_new_from_names(context, &names, XKB_KEYMAP_COMPILE_NO_FLAGS);
    const xkb_rule_names agent{"evdev", "pc105", "us", "", ""};
    auto canonical = xkb_keymap_new_from_names(context, &agent, XKB_KEYMAP_COMPILE_NO_FLAGS);
    if (!physical || !canonical) { std::cout << "SKIP: XKB data unavailable\n"; return 77; }
    auto keyboard = std::make_shared<Keyboard>(); keyboard->m_xkbKeymap = physical;
    manager.m_keyboard = keyboard; input.m_keyboards = {keyboard};
    auto root = std::make_shared<Root>();
    manager.m_state.keyboardFocus = root; manager.m_state.pointerFocus = root; Desktop::focus.root = root;
    manager.seat = std::make_shared<Seat>();
    auto resource = std::make_shared<Resource>();
    auto pointer = std::make_shared<Resource>();
    resource->client = xkb_state_new(physical);
    manager.seat->m_keyboards = {resource}; manager.seat->m_pointers = {pointer};
    Client client; client.surface = root; client.window = root;
    Lane lane; lane.physical_keymap = physical; lane.keymap = canonical;
    lane.physical_keyboard_state = xkb_state_new(physical); lane.lease = &client;
    const auto num = foreground_numlock_mask(physical);
    const auto caps = foreground_caps_mask(physical);
    check(num != 0 && caps != 0, "lock encodings unresolved");
    using Reason = ForegroundFailureReason;

    // Omarchy default: Num Lock on. Text types, and the client never sees
    // Num Lock cleared: no modifier change around the action.
    keyboard->m_modifiersState.locked = num;
    xkb_state_update_mask(resource->client, 0, 0, num, 0, 0, 0);
    lane.preflight(client, "KEY", 30, 1);
    check(lane.foreground_started && lane.foreground_modifiers[2] == num,
          "actual Num Lock state was not captured for delivery");
    check(resource->history.empty(), "a modifier event was sent before the first key");
    lane.foreground_key(client, 42, true); lane.foreground_key(client, 30, true);
    lane.foreground_key(client, 30, false); lane.foreground_key(client, 42, false);
    check(resource->symbols.back() == XKB_KEY_A, "client did not receive intended shifted text");
    for (const auto& state : resource->history)
        check(state[2] == num, "dispatch transiently cleared Num Lock");
    lane.finish();
    check(resource->history.back() == std::array<unsigned, 4>{0, 0, num, 0} &&
              keyboard->m_modifiersState.locked == num,
          "completion changed real or client Num Lock state");
    lane.key(client, 2, 0);
    check(resource->symbols.back() == XKB_KEY_1, "digit changed under Num Lock");

    // Keypad keys whose meaning Num Lock changes refuse before the grant is used.
    auto consumed = lane.consumed;
    const auto sent = resource->history.size();
    check(refused(lane, client, "KEY", 79, 0) == Reason::keyboard_numlock_keypad,
          "Num Lock keypad semantics admitted");
    check(lane.consumed == consumed && !client.foreground_attempted && resource->history.size() == sent,
          "keypad refusal consumed the grant, activated, or sent input");
    keyboard->m_modifiersState.locked = 0;
    xkb_state_update_mask(resource->client, 0, 0, 0, 0, 0, 0);
    lane.key(client, 79, 0);
    check(resource->symbols.back() == XKB_KEY_KP_End, "neutral keypad contract changed");

    // Caps Lock refuses keyboard actions by name, but never pointer actions.
    keyboard->m_modifiersState.locked = caps;
    consumed = lane.consumed;
    check(refused(lane, client, "KEY", 30, 0) == Reason::keyboard_caps_lock, "Caps Lock admitted text");
    check(refused(lane, client, "KEY", 2, 0) == Reason::keyboard_caps_lock, "Caps Lock admitted a digit");
    check(lane.consumed == consumed, "Caps Lock refusal consumed the grant");
    keyboard->m_modifiersState.locked = caps | num;
    check(refused(lane, client, "KEY", 30, 0) == Reason::keyboard_caps_lock, "Caps and Num Lock admitted");
    const auto motions = lane.motions;
    check(refused(lane, client, "CLICK", 272, 0) == Reason::none && lane.motions == motions + 1,
          "Caps Lock gated a pointer-only action");
    keyboard->m_modifiersState.locked = 0x80;
    check(refused(lane, client, "KEY", 30, 0) == Reason::keyboard_locked, "unknown lock admitted");

    // A human lock change during the action refuses the rest of it, and the
    // unwind restores the current human state rather than a stale one.
    keyboard->m_modifiersState.locked = num;
    lane.preflight(client, "KEY", 30, 1); lane.foreground_key(client, 42, true);
    keyboard->m_modifiersState.locked = 0;
    try { lane.foreground_key(client, 30, true); check(false, "ambient state change accepted"); }
    catch (const ForegroundFailure& f) { check(f.reason == Reason::keyboard_state, "wrong state change refusal"); }
    lane.finish();
    check(resource->history.back() == std::array<unsigned, 4>{}, "cancellation restored stale Num Lock");
    keyboard->m_modifiersState.locked = num; lane.preflight(client, "KEY", 30, 1); lane.foreground_key(client, 42, true);
    keyboard->m_modifiersState.locked = caps;
    keyboard->m_modifiersState.depressed = xkb_keymap_mod_get_index(physical, XKB_MOD_NAME_CTRL) < 32 ?
        1u << xkb_keymap_mod_get_index(physical, XKB_MOD_NAME_CTRL) : 0;
    lane.finish();
    check(resource->history.back()[0] == keyboard->m_modifiersState.depressed &&
              resource->history.back()[2] == keyboard->m_modifiersState.locked,
          "cancellation failed to restore current human Caps Lock and held Control");
    keyboard->m_modifiersState.depressed = 0; keyboard->m_modifiersState.locked = num;

    // Held physical keys still refuse before activation.
    lane.physical_held = true;
    check(refused(lane, client, "KEY", 30, 0) == Reason::physical_keys, "held physical key admitted");
    lane.physical_held = false;

    // Shared keyboards merge into the state the client sees.
    auto shared = std::make_shared<Keyboard>(); shared->m_xkbKeymap = physical;
    shared->m_modifiersState.locked = num;
    keyboard->m_modifiersState.locked = 0;
    input.m_keyboards.push_back(shared);
    lane.preflight(client, "KEY", 30, 0);
    check(lane.foreground_modifiers[2] == num, "shared Num Lock was ignored");
    lane.foreground_key(client, 30, true); lane.foreground_key(client, 30, false); lane.finish();
    check(resource->history.back()[2] == num, "shared Num Lock not restored");
    shared->m_modifiersState.locked = caps;
    check(refused(lane, client, "KEY", 30, 0) == Reason::keyboard_caps_lock, "shared Caps Lock admitted");
    shared->shared = false;
    check(refused(lane, client, "KEY", 30, 0) == Reason::none, "unshared keyboard state gated input");

    // A differently encoded lock on another keyboard is not Num Lock.
    const xkb_rule_names other{"evdev", "pc105", "us", "", ""};
    auto other_map = xkb_keymap_new_from_names(context, &other, XKB_KEYMAP_COMPILE_NO_FLAGS);
    shared->shared = true; shared->m_xkbKeymap = other_map; shared->m_modifiersState.locked = 0x40;
    check(refused(lane, client, "KEY", 30, 0) == Reason::keyboard_locked, "foreign lock encoding admitted");
    input.m_keyboards.pop_back();

    // Focus callbacks can change real state between admission and the first event.
    keyboard->m_modifiersState.locked = num;
    Desktop::focus.root.reset(); Desktop::focus.change_on_focus = keyboard;
    const auto before_focus = resource->history.size();
    check(refused(lane, client, "KEY", 30, 0) == Reason::keyboard_state, "focus-time state change accepted");
    check(resource->history.size() == before_focus, "unused keyboard sent a modifier restore");
    Desktop::focus.change_on_focus.reset();

    // The up-front layout gate still applies to foreground keys.
    lane.layout_ready = false; keyboard->m_modifiersState.locked = 0;
    check(refused(lane, client, "KEY", 30, 0) == Reason::unsupported_layout, "unqualified layout admitted");
    lane.layout_ready = true;

    xkb_state_unref(resource->client); xkb_keymap_unref(other_map);
    xkb_keymap_unref(physical); xkb_keymap_unref(canonical); xkb_context_unref(context);
    std::cout << "foreground modifier tests passed\n";
}
'''


def production_fixture(source):
    methods = []
    for name in ('capture_foreground_modifiers', 'require_foreground', 'finish_foreground',
                 'start_foreground', 'foreground_key'):
        body = re.search(r'^    \S[^\n]*\b' + name + r'\(.*?^    }', source, re.M | re.S)
        if not body:
            raise AssertionError(f'production method not found: {name}')
        methods.append(body.group())
    preflight = re.search(r'        std::array<std::uint32_t, 4> modifiers\{\};\n        if \(command == "KEY"\) \{.*?'
                          r'        start_foreground\([^\n]*;', source, re.S)
    if not preflight:
        raise AssertionError('production foreground KEY preflight not found')
    return FIXTURE.replace('// METHODS', '\n'.join(methods)).replace('// PREFLIGHT', preflight.group())


def xkbcommon_flags():
    if shutil.which('pkg-config') is None:
        return None
    result = subprocess.run(['pkg-config', '--cflags', '--libs', 'xkbcommon'], capture_output=True, text=True)
    return shlex.split(result.stdout) if result.returncode == 0 else None


class ForegroundModifiersTest(unittest.TestCase):
    def test_production_modifiers(self):
        fixture = production_fixture((ROOT / 'src/input_experiment.cpp').read_text())
        flags = xkbcommon_flags()
        if flags is None:
            if REQUIRED:
                self.fail('xkbcommon development files are required')
            self.skipTest('xkbcommon development files unavailable')
        compiler = shlex.split(os.environ.get('CXX', '')) or [
            shutil.which('clang++-18') or shutil.which('clang++') or 'c++']
        with tempfile.TemporaryDirectory(prefix='cua-foreground-modifiers-') as directory:
            cpp, binary = Path(directory) / 'fixture.cpp', Path(directory) / 'fixture'
            cpp.write_text(fixture)
            build = subprocess.run([*compiler, '-std=c++20', '-Wall', '-Wextra', '-Wpedantic', '-Werror',
                                    '-I', str(ROOT / 'src'), str(cpp), '-o', str(binary), *flags],
                                   capture_output=True, text=True, timeout=120)
            self.assertEqual(build.returncode, 0, build.stdout + build.stderr)
            result = subprocess.run([str(binary)], capture_output=True, text=True, timeout=30)
            if result.returncode == 77 and not REQUIRED:
                self.skipTest('XKB data unavailable')
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == '__main__':
    unittest.main()
