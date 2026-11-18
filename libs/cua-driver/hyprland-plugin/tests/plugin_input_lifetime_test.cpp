#include "mock-hyprland/mock.hpp"
#include "ime_popup_guard.hpp"
#include "input_experiment.hpp"
#include "user_seat_guard.hpp"

#include <array>
#include <cstdlib>
#include <iostream>
#include <string>
#include <sys/stat.h>
#include <unistd.h>

extern "C" PLUGIN_DESCRIPTION_INFO pluginInit(HANDLE);
extern "C" void pluginExit();

namespace {
unsigned created = 0, destroyed = 0, resumed = 0, suspended = 0;
bool fail_resume = false;
void check(bool value, const char* message) {
    if (!value) {
        std::cerr << "FAIL: " << message << '\n';
        std::exit(1);
    }
}
// Omarchy's Cua Input toggle checks both fields before enabling input, to tell
// this module from an older one still loaded. Keep these exact names.
[[maybe_unused]] bool advertises_keyboard_behavior(const std::string& status) {
    return status.find(R"("keyboard_layout_independent":true)") != std::string::npos &&
        status.find(R"("foreground_numlock_compatible":true)") != std::string::npos;
}
[[maybe_unused]] bool mentions_keyboard_behavior(const std::string& status) {
    return status.find("keyboard_layout_independent") != std::string::npos ||
        status.find("foreground_numlock_compatible") != std::string::npos;
}
}

// This mock tests the entry point's ownership decisions, not native wl_seat
// delivery. Native resource-count and surviving-client tests remain separate.
namespace cua::hyprland {
struct InputExperiment::Impl {};
struct InputExperiment::DesktopListeners {};
class PrimaryTrace {};
InputExperiment::InputExperiment(const std::string&, void*) { ++created; }
InputExperiment::~InputExperiment() { ++destroyed; }
void InputExperiment::suspend() { ++suspended; }
void InputExperiment::resume() {
    ++resumed;
    if (fail_resume) throw std::runtime_error("test transport failure");
}
std::string InputExperiment::status_json() const { return "{}"; }
unsigned seat_guard_installs = 0, seat_guard_removals = 0;
bool install_user_seat_guard(void*) { ++seat_guard_installs; return true; }
void remove_user_seat_guard() { ++seat_guard_removals; }
bool user_seat_guard_active() { return seat_guard_installs > seat_guard_removals; }
std::uint64_t popup_grabs_withheld() { return 0; }
std::uint64_t drags_withheld() { return 0; }
unsigned ime_guard_installs = 0, ime_guard_removals = 0;
bool install_ime_popup_guard(void*) { ++ime_guard_installs; return true; }
void remove_ime_popup_guard() { ++ime_guard_removals; }
bool ime_popup_guard_active() { return ime_guard_installs > ime_guard_removals; }
}

int main() {
    std::array<char, 64> pattern{};
    const std::string seed = "/tmp/cua-input-lifetime-api-XXXXXX";
    seed.copy(pattern.data(), seed.size());
    const auto* directory = mkdtemp(pattern.data());
    check(directory != nullptr, "create isolated runtime");
    const auto hypr = std::string(directory) + "/hypr";
    const auto instance = hypr + "/mock";
    const auto socket = instance + "/cua-inject-v2.sock";
    check(mkdir(hypr.c_str(), 0700) == 0 && mkdir(instance.c_str(), 0700) == 0,
          "create private instance path");
    check(setenv("XDG_RUNTIME_DIR", directory, 1) == 0 &&
              setenv("HYPRLAND_INSTANCE_SIGNATURE", "mock", 1) == 0,
          "select runtime");
    Config::Values::configured_bool_override = false;
    static_cast<void>(pluginInit(nullptr));
    const auto initial_status = HyprlandAPI::registered_legacy_command->fn(FORMAT_JSON, {});
    check(created == 0 && initial_status.find(R"("configured":false)") != std::string::npos,
          "plugin starts with input disabled");
    check(initial_status.find(R"("user_seat_guard":true,"popup_grabs_withheld":0,"drags_withheld":0,"ime_popup_guard":true)") != std::string::npos,
          "user seat and IME popup guards are installed with the plugin and reported");
#ifdef CUA_HYPRLAND_INPUT
    check(advertises_keyboard_behavior(initial_status),
          "disabled v3 module advertises its compiled keyboard behavior before enable");
#else
    check(!mentions_keyboard_behavior(initial_status), "experiment advertises v3 keyboard behavior");
#endif
    const auto toggle = [](bool enabled) {
        HyprlandAPI::registered_bool->set_mock_value(enabled);
        Event::bus()->m_events.config.reloaded.emit();
    };
    for (unsigned i = 0; i < 20; ++i) {
        toggle(true);
#ifdef CUA_HYPRLAND_INPUT
        const auto status = HyprlandAPI::registered_legacy_command->fn(FORMAT_JSON, {});
        check(status.find("\"state\":\"input_v3_candidate\"") != std::string::npos &&
                  status.find("trusted_local_per_action") != std::string::npos &&
                  status.find("\"input\":{}") != std::string::npos &&
                  status.find("operator") == std::string::npos,
              "v3 status advertises its actual admission mode");
        check(advertises_keyboard_behavior(status) && status.back() == '}' &&
                  status.find(R"("foreground_numlock_compatible":true,"input":{}})") != std::string::npos,
              "enabled v3 status keeps both keyboard fields in one JSON object");
#else
        check(!mentions_keyboard_behavior(HyprlandAPI::registered_legacy_command->fn(FORMAT_JSON, {})),
              "experiment advertises v3 keyboard behavior");
#endif
        check(created == 1 && destroyed == 0 && resumed == i + 1,
              "enable reuses the session's seat owner");
        toggle(true);
        check(resumed == i + 1, "unrelated config reload does not renew admission");
        toggle(false);
        check(destroyed == 0 && access(socket.c_str(), F_OK) != 0,
              "disable stops transport but retains seat owner");
#ifdef CUA_HYPRLAND_INPUT
        check(advertises_keyboard_behavior(HyprlandAPI::registered_legacy_command->fn(FORMAT_JSON, {})),
              "disabled v3 module keeps advertising its keyboard behavior");
#endif
    }
    fail_resume = true;
    toggle(true);
    check(created == 1 && destroyed == 0 && access(socket.c_str(), F_OK) != 0,
          "transport failure does not replace seats or leave discovery listener");
    fail_resume = false;
    toggle(true);
    check(created == 1 && access(socket.c_str(), F_OK) == 0,
          "retry after transport failure reuses seat owner");
    const auto suspended_before_exit = suspended;
    pluginExit();
    check(destroyed == 1 && suspended >= 20 && access(socket.c_str(), F_OK) != 0,
          "unload retires the single seat owner and stops transport");
    check(suspended == suspended_before_exit,
          "unload retires directly without first sending config-disable cancellation");
    check(Event::bus()->m_events.config.reloaded.active_count() == 0,
          "unload detaches configuration listener");
    check(!cua::hyprland::user_seat_guard_active(), "unload removes the user seat guard");
    check(!cua::hyprland::ime_popup_guard_active(), "unload removes the IME popup guard");
    check(rmdir(instance.c_str()) == 0 && rmdir(hypr.c_str()) == 0 &&
              rmdir(directory) == 0,
          "remove test-owned runtime");
    std::cout << "plugin input lifetime API tests passed\n";
}
