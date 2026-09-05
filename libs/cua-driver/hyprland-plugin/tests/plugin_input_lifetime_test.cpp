#include "mock-hyprland/mock.hpp"
#include "input_experiment.hpp"

#include <array>
#include <cstdlib>
#include <iostream>
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
    Config::Values::configured_bool_override = true;
    static_cast<void>(pluginInit(nullptr));
    const auto toggle = [](bool enabled) {
        HyprlandAPI::registered_bool->set_mock_value(enabled);
        Event::bus()->m_events.config.reloaded.emit();
    };
    for (unsigned i = 0; i < 20; ++i) {
        toggle(true);
        check(created == 1 && destroyed == 0 && resumed == i + 1,
              "enable reuses the session's seat owner");
        toggle(true);
        check(resumed == i + 1, "unrelated config reload does not renew admission");
        toggle(false);
        check(destroyed == 0 && access(socket.c_str(), F_OK) != 0,
              "disable stops transport but retains seat owner");
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
    check(rmdir(instance.c_str()) == 0 && rmdir(hypr.c_str()) == 0 &&
              rmdir(directory) == 0,
          "remove test-owned runtime");
    std::cout << "plugin input lifetime API tests passed\n";
}
