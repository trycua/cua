"""Exercise the agent keyboard's repeat_info and KEY hold with production bodies.

The method bodies come from input_experiment.cpp, so a changed production call
site changes the behavior under test. Seat, keyboard, surface and XKB objects
are fakes: this is policy coverage, not evidence of Wayland delivery or of how
a client schedules key repeat.
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

FIXTURE = r'''
#include <algorithm>
#include <cstdint>
#include <cstdlib>
#include <functional>
#include <iostream>
#include <memory>
#include <string>
#include <utility>
#include <vector>

template <class T> using SP = std::shared_ptr<T>;
template <class T> SP<T> makeShared(auto&&... args) { return std::make_shared<T>(args...); }
template <class T> struct WP {
    std::weak_ptr<T> value;
    WP& operator=(const SP<T>& target) { value = target; return *this; }
    SP<T> lock() const { return value.lock(); }
    void reset() { value.reset(); }
    explicit operator bool() const { return !value.expired(); }
    bool operator==(const SP<T>& target) const { return !value.expired() && value.lock() == target; }
};
struct wl_array {};
constexpr int WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1 = 1;
constexpr int WL_KEYBOARD_KEY_STATE_RELEASED = 0, WL_KEYBOARD_KEY_STATE_PRESSED = 1;
enum xkb_key_direction { XKB_KEY_UP, XKB_KEY_DOWN };
enum xkb_state_component {
    XKB_STATE_MODS_DEPRESSED = 1, XKB_STATE_MODS_LATCHED = 2, XKB_STATE_MODS_LOCKED = 4,
    XKB_STATE_LAYOUT_EFFECTIVE = 128,
};
struct xkb_keymap {};
struct xkb_state {};
xkb_keymap canonical_map;
xkb_state canonical_state;
xkb_state* xkb_state_new(xkb_keymap*) { return &canonical_state; }
void xkb_state_unref(xkb_state*) {}
int xkb_state_update_key(xkb_state*, std::uint32_t, xkb_key_direction) { return 0; }
std::uint32_t xkb_state_serialize_mods(xkb_state*, xkb_state_component) { return 0; }
std::uint32_t xkb_state_serialize_layout(xkb_state*, xkb_state_component) { return 0; }
std::uint32_t event_ms() { return 0; }
std::uint64_t number(const std::string& value) { return std::stoull(value); }
std::string refusal(const char* code) { return code; }
constexpr std::size_t kMaxResources = 512;

void check(bool ok, const std::string& why) {
    if (!ok) { std::cerr << "FAIL: " << why << '\n'; std::exit(1); }
}

// A user keyboard as Hyprland's seat manager exposes it.
struct IKeyboard { int m_repeatRate = 0, m_repeatDelay = 0; };
struct Surface {
    SP<int> resource = std::make_shared<int>(0);
    bool good() const { return true; }
    int client() const { return 7; }
    SP<int> getResource() const { return resource; }
};
struct CWlKeyboard {
    int owner, protocol_version;
    std::vector<std::string> log;
    std::function<void(CWlKeyboard*)> release;
    CWlKeyboard(int client, int version, std::uint32_t) : owner(client), protocol_version(version) {}
    bool resource() const { return true; }
    int client() const { return owner; }
    int version() const { return protocol_version; }
    void setData(void*) {}
    void setRelease(std::function<void(CWlKeyboard*)> f) { release = std::move(f); }
    void setOnDestroy(std::function<void(CWlKeyboard*)>) {}
    void sendKeymap(int, int, std::size_t) { log.push_back("keymap"); }
    void sendRepeatInfo(std::int32_t rate, std::int32_t delay) {
        log.push_back("repeat_info " + std::to_string(rate) + " " + std::to_string(delay));
    }
    void sendEnter(std::uint32_t, int*, wl_array*) { log.push_back("enter"); }
    void sendLeave(std::uint32_t, int*) { log.push_back("leave"); }
    void sendKey(std::uint32_t, std::uint32_t, std::uint32_t code, int state) {
        log.push_back("key " + std::to_string(code) + (state == WL_KEYBOARD_KEY_STATE_PRESSED ? " down" : " up"));
    }
    void sendModifiers(std::uint32_t, std::uint32_t, std::uint32_t, std::uint32_t, std::uint32_t) {
        log.push_back("modifiers");
    }
};
struct CWlSeat {
    int protocol_version;
    int client() const { return 7; }
    int version() const { return protocol_version; }
    void noMemory() { check(false, "agent keyboard creation refused"); }
};
struct Client { WP<Surface> surface; };

struct Lane {
    struct Keyboard { SP<CWlKeyboard> wl; bool dead = false; WP<Surface> focus; };
    std::vector<std::unique_ptr<Keyboard>> keyboards;
    std::vector<std::uint32_t> held_keys;
    xkb_keymap* keymap = &canonical_map;
    xkb_keymap* physical_keymap = nullptr;
    xkb_state* keyboard_state = &canonical_state;
    xkb_state* physical_keyboard_state = nullptr;
    int keymap_fd = 3;
    std::string keymap_text = "canonical us";
    std::vector<std::string> responses;
    // PRODUCTION_STATE
    std::uint32_t serial() const { return 1; }
    void send(Client&, std::string response) { responses.push_back(std::move(response)); }
    bool consume_grant(Client&, std::uint64_t) { return true; }
    void finish_foreground() {}
    void retire_grant() {}
    // PRODUCTION_METHODS
    void key_request(Client& c, const std::vector<std::string>& f, std::uint64_t cap) {
        // PRODUCTION_KEY
        complete_action();
    }
    CWlKeyboard& bind(int version) {
        CWlSeat seat{version};
        add_keyboard(&seat, static_cast<std::uint32_t>(keyboards.size() + 1));
        return *keyboards.back()->wl;
    }
};

using Log = std::vector<std::string>;

int main() {
    Lane lane;
    // Before an active keyboard reports the user's values, the agent seat still
    // advertises a usable rate. Hyprland's own defaults are 25 Hz after 600 ms.
    auto& early = lane.bind(9);
    check(early.log == Log{"keymap", "repeat_info 25 600"}, "default repeat_info before a user keyboard");
    lane.sync_key_repeat(nullptr);
    check(early.log.size() == 2, "no active keyboard keeps the advertised values, as the user seat does");

    // The agent seat mirrors the user seat (foot saw repeat_info(40, 250) there in #4257).
    const auto user = std::make_shared<IKeyboard>(IKeyboard{40, 250});
    lane.sync_key_repeat(user);
    check(early.log.back() == "repeat_info 40 250" && early.log.size() == 3,
          "an existing agent keyboard follows the user seat once");
    auto& fresh = lane.bind(9);
    check(fresh.log == Log{"keymap", "repeat_info 40 250"}, "a new agent keyboard gets the user seat's values");
    auto& legacy = lane.bind(3);
    check(legacy.log == Log{"keymap"}, "repeat_info is never sent below wl_keyboard version 4");
    lane.sync_key_repeat(user);
    check(early.log.size() == 3 && fresh.log.size() == 2, "unchanged user values are not resent");

    // A changed user setting reaches live agent keyboards, never released ones.
    auto& released = lane.bind(9);
    released.release(&released);
    user->m_repeatRate = 30; user->m_repeatDelay = 500;
    lane.sync_key_repeat(user);
    check(early.log.back() == "repeat_info 30 500" && fresh.log.back() == "repeat_info 30 500",
          "a changed user setting is resent");
    check(legacy.log == Log{"keymap"} && released.log.back() == "repeat_info 40 250",
          "version 3 and released keyboards get no repeat_info");
    user->m_repeatRate = -1;
    lane.sync_key_repeat(user);
    check(fresh.log.back() == "repeat_info 0 500", "a negative rate is clamped, not wrapped");
    for (const auto& k : lane.keyboards)
        for (const auto& event : k->wl->log) check(event != "repeat_info 0 0", "repeat_info(0, 0) sent");

    // A real rate is only safe because KEY holds nothing past its own request:
    // shift+A is pressed, released and left inside one production handler.
    lane.keyboards.clear();
    auto& typing = lane.bind(9);
    const auto surface = std::make_shared<Surface>();
    Client client; client.surface = surface;
    lane.key_request(client, {"KEY", "1", "token", "1", "30", "1"}, 2);
    check(lane.responses.empty(), "KEY was refused");
    Log keys;
    for (const auto& event : typing.log) if (event.starts_with("key ") || event == "leave") keys.push_back(event);
    check(keys == Log{"key 42 down", "key 30 down", "key 30 up", "key 42 up", "leave"},
          "KEY releases every key it pressed before leaving focus");
    check(lane.held_keys.empty(), "no key is held after the KEY request returns");
    std::cout << "agent key repeat tests passed\n";
}
'''


def method(source, name):
    match = re.search(r'^    \S[^\n]*\b' + name + r'\(.*?^    }', source, re.M | re.S)
    if not match:
        raise AssertionError(f'production method not found: {name}')
    return match.group()


def production_fixture(source):
    state = re.search(r'^    int repeat_rate = \d+, repeat_delay = \d+;$', source, re.M)
    if not state:
        raise AssertionError('production repeat_info defaults not found')
    methods = '\n'.join(method(source, name) for name in (
        'sync_key_repeat', 'add_keyboard', 'keyboard_enter', 'key', 'leave_keyboard', 'complete_action'))
    # The independent-route KEY branch of request(), not the foreground one.
    branch = re.search(r'            foreground_request\(c, f, cap\);\n            return;\n        }\n'
                       r'        if \(command == "KEY"\) \{\n(.*?)^        } else \{', source, re.M | re.S)
    if not branch:
        raise AssertionError('production independent KEY branch not found')
    return (FIXTURE.replace('// PRODUCTION_STATE', state.group().strip())
            .replace('// PRODUCTION_METHODS', methods)
            .replace('// PRODUCTION_KEY', branch.group(1)))


class AgentKeyRepeatTest(unittest.TestCase):
    def test_production_repeat_info_and_key_hold(self):
        source = (ROOT / 'src/input_experiment.cpp').read_text()
        # Fail if the tested pieces are disconnected from the production paths.
        self.assertIn('sync_key_repeat(keyboard);', method(source, 'sync_keymap'))
        self.assertIn('leave_keyboard();', method(source, 'complete_action'))
        self.assertRegex(source, r'\+\+dispatches;\n[^\n]*\n        if \(kProduction\) complete_action\(\);\n'
                                 r'        send\(c, kDelivered\);')
        compiler = shlex.split(os.environ.get('CXX', '')) or [
            shutil.which('clang++-18') or shutil.which('clang++') or 'c++']
        with tempfile.TemporaryDirectory(prefix='cua-agent-key-repeat-') as directory:
            cpp, binary = Path(directory) / 'fixture.cpp', Path(directory) / 'fixture'
            cpp.write_text(production_fixture(source))
            build = subprocess.run([*compiler, '-std=c++20', '-Wall', '-Wextra', '-Wpedantic', '-Werror',
                                    str(cpp), '-o', str(binary)],
                                   capture_output=True, text=True, timeout=120)
            self.assertEqual(build.returncode, 0, build.stdout + build.stderr)
            result = subprocess.run([str(binary)], capture_output=True, text=True, timeout=30)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == '__main__':
    unittest.main()
