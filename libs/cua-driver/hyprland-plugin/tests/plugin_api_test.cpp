#include "mock-hyprland/mock.hpp"

#include <algorithm>
#include <array>
#include <cerrno>
#include <cstdlib>
#include <iostream>
#include <stdexcept>
#include <string>
#include <sys/stat.h>
#include <unistd.h>

extern "C" std::string pluginAPIVersion();
extern "C" PLUGIN_DESCRIPTION_INFO pluginInit(HANDLE handle);
extern "C" void pluginExit();

namespace {

void check(bool condition, const std::string& message) {
    if (condition)
        return;
    std::cerr << "FAIL: " << message << '\n';
    std::exit(1);
}

void reset_registered_command() {
#if CUA_HYPRLAND_SOCKET1_API
    HyprlandAPI::registered_command.reset();
#else
    HyprlandAPI::registered_legacy_command.reset();
#endif
}

bool status_command_registered() {
#if CUA_HYPRLAND_SOCKET1_API
    return HyprlandAPI::registered_command &&
           HyprlandAPI::registered_command->name == "cua:status" &&
           HyprlandAPI::registered_command->match ==
               IPC::Socket1::COMMAND_MATCH_EXACT;
#else
    return HyprlandAPI::registered_legacy_command &&
           HyprlandAPI::registered_legacy_command->name == "cua:status" &&
           HyprlandAPI::registered_legacy_command->exact;
#endif
}

std::string json_status() {
#if CUA_HYPRLAND_SOCKET1_API
    const auto response = HyprlandAPI::registered_command->handler(
        IPC::Socket1::SRequest{.format = IPC::Socket1::FORMAT_JSON});
    return std::get<std::string>(response.result);
#else
    return HyprlandAPI::registered_legacy_command->fn(FORMAT_JSON, {});
#endif
}

struct RuntimeTree {
    std::string root;
    std::string hypr;
    std::string instance;
    std::string socket;
};

RuntimeTree make_runtime_tree() {
    std::array<char, 64> pattern{};
    const auto seed = std::string{"/tmp/cua-hyprland-plugin-api-XXXXXX"};
    std::copy(seed.begin(), seed.end(), pattern.begin());
    const auto* root = mkdtemp(pattern.data());
    check(root != nullptr, "plugin API test runtime directory created");

    RuntimeTree tree{
        .root = root,
        .hypr = std::string{root} + "/hypr",
        .instance = std::string{root} + "/hypr/mock-instance",
        .socket = std::string{root} +
                  "/hypr/mock-instance/cua-inject-v2.sock",
    };
    check(mkdir(tree.hypr.c_str(), 0700) == 0,
          "plugin API test Hyprland directory created");
    check(mkdir(tree.instance.c_str(), 0700) == 0,
          "plugin API test instance directory created");
    return tree;
}

void remove_runtime_tree(const RuntimeTree& tree) {
    check(rmdir(tree.instance.c_str()) == 0,
          "plugin API test instance directory removed");
    check(rmdir(tree.hypr.c_str()) == 0,
          "plugin API test Hyprland directory removed");
    check(rmdir(tree.root.c_str()) == 0,
          "plugin API test runtime directory removed");
}

} // namespace

int main() {
    check(pluginAPIVersion() == HYPRLAND_API_VERSION,
          "plugin exports the expected Hyprland API version");

    HyprlandAPI::runtime_abi_hash = __hyprland_api_get_client_hash();
    const auto description = pluginInit(nullptr);
    check(description.name == "cua-hyprland-plugin" &&
              description.version == "0.1.0",
          "matching ABI initializes plugin metadata");
    check(status_command_registered(),
          "plugin registers the exact status command");

    const auto json_text = json_status();
    check(json_text.find(R"("state":"discovery_only")") !=
              std::string::npos &&
              json_text.find(R"("configured":false)") !=
                  std::string::npos &&
              json_text.find(R"("compositor_epoch":0)") !=
                  std::string::npos &&
              json_text.find(R"("enabled":[])") != std::string::npos,
          "disabled plugin status cannot advertise mutation or transport readiness");
    check(Event::bus()->m_events.config.reloaded.active_count() == 1,
          "plugin owns one config-reload listener while loaded");
    pluginExit();
    const auto deliveries_after_exit =
        Event::bus()->m_events.config.reloaded.delivery_count();
    check(Event::bus()->m_events.config.reloaded.active_count() == 0,
          "plugin unload unregisters its config-reload listener");
    Event::bus()->m_events.config.reloaded.emit();
    check(Event::bus()->m_events.config.reloaded.delivery_count() ==
              deliveries_after_exit,
          "config reload after unload cannot call plugin code");

    reset_registered_command();
    HyprlandAPI::runtime_abi_hash = "different-hyprland-abi";
    bool fingerprint_mismatch_rejected = false;
    try {
        static_cast<void>(pluginInit(nullptr));
    } catch (const std::runtime_error& error) {
        fingerprint_mismatch_rejected =
            std::string{error.what()}.find("ABI fingerprint mismatch") !=
            std::string::npos;
    }
    check(fingerprint_mismatch_rejected,
          "runtime Hyprland ABI fingerprint mismatch fails initialization");
    check(!status_command_registered(),
          "ABI fingerprint mismatch registers no status command");

    HyprlandAPI::runtime_abi_hash = __hyprland_api_get_client_hash();
    Config::Values::configured_bool_override = true;
    const auto runtime = make_runtime_tree();
    check(setenv("XDG_RUNTIME_DIR", runtime.root.c_str(), 1) == 0 &&
              setenv("HYPRLAND_INSTANCE_SIGNATURE", "mock-instance", 1) == 0,
          "plugin API test runtime environment configured");
    static_cast<void>(pluginInit(nullptr));
    Event::bus()->m_events.config.reloaded.emit();
    const auto ready_text = json_status();
    struct stat socket_metadata{};
    check(ready_text.find(R"("configured":true)") != std::string::npos &&
              ready_text.find(R"("ready":true)") != std::string::npos &&
              ready_text.find(R"("compositor_epoch":0)") == std::string::npos &&
              lstat(runtime.socket.c_str(), &socket_metadata) == 0 &&
              S_ISSOCK(socket_metadata.st_mode),
          "post-init config reload starts the enabled discovery transport");
    pluginExit();
    check(lstat(runtime.socket.c_str(), &socket_metadata) != 0 && errno == ENOENT,
          "plugin unload removes its transport socket");
    remove_runtime_tree(runtime);

    reset_registered_command();
    const auto insecure_runtime = make_runtime_tree();
    check(chmod(insecure_runtime.hypr.c_str(), 0755) == 0,
          "plugin API test made an insecure runtime component");
    check(setenv("XDG_RUNTIME_DIR", insecure_runtime.root.c_str(), 1) == 0,
          "plugin API test selected the insecure runtime");
    static_cast<void>(pluginInit(nullptr));
    Event::bus()->m_events.config.reloaded.emit();
    const auto refused_text = json_status();
    check(refused_text.find(R"("ready":false)") != std::string::npos &&
              refused_text.find(R"("compositor_epoch":0)") !=
                  std::string::npos &&
              refused_text.find("not a private, same-user directory") !=
                  std::string::npos &&
              lstat(insecure_runtime.socket.c_str(), &socket_metadata) != 0 &&
              errno == ENOENT,
          "insecure runtime path fails closed without creating a socket");
    pluginExit();
    check(chmod(insecure_runtime.hypr.c_str(), 0700) == 0,
          "plugin API test restored runtime permissions for cleanup");
    remove_runtime_tree(insecure_runtime);

    Config::Values::configured_bool_override.reset();
    unsetenv("XDG_RUNTIME_DIR");
    unsetenv("HYPRLAND_INSTANCE_SIGNATURE");

    std::cout << "cua-hyprland-plugin API tests passed\n";
    return 0;
}
