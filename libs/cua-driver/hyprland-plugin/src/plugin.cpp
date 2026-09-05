#include "inject_server.hpp"

#include "cua_hyprland/protocol.hpp"
#include "cua_hyprland/status.hpp"

#include <src/config/values/types/BoolValue.hpp>
#include <src/plugins/PluginAPI.hpp>

#include <cerrno>
#include <cstdlib>
#include <cstring>
#include <filesystem>
#include <format>
#include <memory>
#include <random>
#include <stdexcept>
#include <string>
#include <string_view>
#include <sys/stat.h>
#include <unistd.h>

#ifdef __linux__
#include <sys/random.h>
#endif

#ifndef CUA_HYPRLAND_SOCKET1_API
#error CMake must select a supported Hyprland command API
#endif

namespace {

constexpr auto kPluginVersion = CUA_HYPRLAND_PLUGIN_VERSION;
constexpr auto kSocketName = "cua-inject-v2.sock";

SP<Config::Values::CBoolValue> g_enabled;
#if CUA_HYPRLAND_SOCKET1_API
SP<IPC::Socket1::SCommand> g_status_command;
#else
SP<SHyprCtlCommand> g_status_command;
#endif
CHyprSignalListener g_config_listener;
std::unique_ptr<cua::hyprland::InjectionServer> g_server;
std::string g_compiled_abi_hash;
std::string g_runtime_abi_hash;
std::uint64_t g_epoch = 0;
std::string g_socket_path;
std::string g_reconcile_error;

bool safe_instance_signature(const std::string& signature) {
    if (signature.empty())
        return false;
    for (const auto character : signature) {
        const auto value = static_cast<unsigned char>(character);
        const auto ascii_alphanumeric =
            (value >= 'a' && value <= 'z') || (value >= 'A' && value <= 'Z') ||
            (value >= '0' && value <= '9');
        if (!ascii_alphanumeric && character != '-' && character != '_' &&
            character != '.')
            return false;
    }
    return signature != "." && signature != "..";
}

std::string abi_version_without_patch(std::string_view version) {
    if (!version.contains('.'))
        return std::string{version};
    return std::string{version.substr(0, version.find_last_of('.'))};
}

std::string compiled_abi_fingerprint() {
    return std::format(
        "{}_aq_{}_hu_{}_hg_{}_hc_{}_hlg_{}", GIT_COMMIT_HASH,
        abi_version_without_patch(AQUAMARINE_VERSION),
        abi_version_without_patch(HYPRUTILS_VERSION),
        abi_version_without_patch(HYPRGRAPHICS_VERSION),
        abi_version_without_patch(HYPRCURSOR_VERSION),
        abi_version_without_patch(HYPRLANG_VERSION));
}

bool safe_runtime_path(const std::string& runtime) {
    if (runtime.empty() || runtime.front() != '/' ||
        !cua::hyprland::is_valid_utf8(runtime))
        return false;
    for (const auto character : runtime) {
        const auto value = static_cast<unsigned char>(character);
        if (value < 0x20 || value == 0x7f)
            return false;
    }
    return true;
}

std::string resolve_socket_path() {
    const auto* runtime = std::getenv("XDG_RUNTIME_DIR");
    const auto* signature = std::getenv("HYPRLAND_INSTANCE_SIGNATURE");
    if (!runtime || !signature || !safe_runtime_path(runtime) ||
        !safe_instance_signature(signature))
        throw std::runtime_error("invalid Hyprland runtime environment");

    const auto runtime_path = std::filesystem::path{runtime};
    const auto hypr_root = runtime_path / "hypr";
    const auto instance = hypr_root / signature;
    for (const auto& directory : {runtime_path, hypr_root, instance}) {
        struct stat metadata{};
        if (lstat(directory.c_str(), &metadata) != 0 || !S_ISDIR(metadata.st_mode) ||
            metadata.st_uid != getuid() || (metadata.st_mode & 0077) != 0)
            throw std::runtime_error(
                "Hyprland runtime path is not a private, same-user directory");
    }

    return (instance / kSocketName).string();
}

std::uint64_t make_epoch() {
#ifdef __linux__
    std::uint64_t epoch = 0;
    auto* destination = reinterpret_cast<std::byte*>(&epoch);
    std::size_t offset = 0;
    while (offset < sizeof(epoch)) {
        const auto received = getrandom(destination + offset,
                                        sizeof(epoch) - offset, 0);
        if (received < 0) {
            if (errno == EINTR)
                continue;
            throw std::runtime_error(std::string{"getrandom failed: "} +
                                     std::strerror(errno));
        }
        if (received == 0)
            throw std::runtime_error("getrandom returned no entropy");
        offset += static_cast<std::size_t>(received);
    }
#else
    // The non-Linux path exists only for the mock API compile and unit tests.
    std::random_device random;
    const auto high = static_cast<std::uint64_t>(random()) << 32;
    const auto low = static_cast<std::uint64_t>(random());
    const auto epoch = high | low;
#endif
    return epoch == 0 ? 1 : epoch;
}

std::string stop_server() {
    if (!g_server)
        return {};
    g_server->stop();
    auto error = g_server->snapshot().last_error;
    g_server.reset();
    return error;
}

void reconcile_server() {
    if (!g_enabled || !g_enabled->value()) {
        g_reconcile_error = stop_server();
        g_socket_path.clear();
        g_epoch = 0;
        return;
    }
    if (g_server && g_server->snapshot().running)
        return;

    static_cast<void>(stop_server());
    g_epoch = 0;
    g_reconcile_error.clear();
    g_socket_path = resolve_socket_path();
    const auto epoch = make_epoch();
    auto server = std::make_unique<cua::hyprland::InjectionServer>(
        g_socket_path,
        cua::hyprland::ServerInfo{
            .compositor_epoch = epoch,
            .supported_capabilities =
                cua::hyprland::capability(cua::hyprland::Capability::discovery),
            .enabled_capabilities =
                cua::hyprland::capability(cua::hyprland::Capability::discovery),
        });
    if (server->start())
        g_epoch = epoch;
    g_server = std::move(server);
}

std::string status_output(bool json) {
    const auto configured = g_enabled && g_enabled->value();
    const auto snapshot = g_server ? g_server->snapshot() : cua::hyprland::ServerSnapshot{};
    const auto last_error = snapshot.last_error.empty() ? g_reconcile_error : snapshot.last_error;
    const cua::hyprland::StatusReport report{
        .plugin_version = kPluginVersion,
        .compositor_epoch = snapshot.running ? g_epoch : 0,
        .compiled_hash = g_compiled_abi_hash,
        .runtime_hash = g_runtime_abi_hash,
        .configured = configured,
        .transport_ready = snapshot.running,
        .socket_path = g_socket_path,
        .last_error = last_error,
        .discovery_enabled = snapshot.running,
        .accepted_connections = snapshot.accepted_connections,
        .rejected_peers = snapshot.rejected_peers,
        .rejected_busy = snapshot.rejected_busy,
        .timed_out_handshakes = snapshot.timed_out_handshakes,
        .timed_out_idle = snapshot.timed_out_idle,
        .malformed_frames = snapshot.malformed_frames,
    };

    if (json)
        return cua::hyprland::render_status_json(report);

    return cua::hyprland::render_status_text(report);
}

#if CUA_HYPRLAND_SOCKET1_API
IPC::Socket1::SResponse status(const IPC::Socket1::SRequest& request) {
    return IPC::Socket1::SResponse{
        status_output(request.format == IPC::Socket1::FORMAT_JSON)};
}

SP<IPC::Socket1::SCommand> register_status_command(HANDLE handle) {
    return HyprlandAPI::registerHyprCtlCommand(
        handle,
        IPC::Socket1::SCommand{
            .name = "cua:status",
            .match = IPC::Socket1::COMMAND_MATCH_EXACT,
            .handler = status,
        });
}
#else
std::string status(eHyprCtlOutputFormat format, std::string) {
    return status_output(format == eHyprCtlOutputFormat::FORMAT_JSON);
}

SP<SHyprCtlCommand> register_status_command(HANDLE handle) {
    return HyprlandAPI::registerHyprCtlCommand(
        handle,
        SHyprCtlCommand{
            .name = "cua:status",
            .exact = true,
            .fn = status,
        });
}
#endif

} // namespace

APICALL EXPORT std::string PLUGIN_API_VERSION() {
    return HYPRLAND_API_VERSION;
}

APICALL EXPORT PLUGIN_DESCRIPTION_INFO PLUGIN_INIT(HANDLE handle) {
    // Hyprland skips PLUGIN_EXIT when initialization throws. Do not start any
    // worker thread until all potentially throwing registration is complete.
    const auto* runtime_abi_hash = __hyprland_api_get_hash();
    g_compiled_abi_hash = compiled_abi_fingerprint();
    if (!runtime_abi_hash || g_compiled_abi_hash != runtime_abi_hash) {
        throw std::runtime_error(std::format(
            "cua-hyprland-plugin ABI fingerprint mismatch: built for {}, running {}",
            g_compiled_abi_hash,
            runtime_abi_hash ? runtime_abi_hash : "<missing>"));
    }
    g_runtime_abi_hash = runtime_abi_hash;

    g_enabled = makeShared<Config::Values::CBoolValue>(
        "plugin:cua:enabled",
        "Enable Cua's same-user local capability transport. Background mutation remains disabled in this build.",
        false);
    if (!HyprlandAPI::addConfigValueV2(handle, g_enabled))
        throw std::runtime_error("failed to register plugin:cua:enabled");

    g_status_command = register_status_command(handle);
    if (!g_status_command)
        throw std::runtime_error("failed to register cua:status");

    g_config_listener = Event::bus()->m_events.config.reloaded.listen([] {
        try {
            reconcile_server();
        } catch (const std::exception& error) {
            static_cast<void>(stop_server());
            g_reconcile_error = error.what();
        }
    });

    return {
        "cua-hyprland-plugin",
        "Discovery foundation for isolated background computer use",
        "Cua",
        kPluginVersion,
    };
}

APICALL EXPORT void PLUGIN_EXIT() {
    g_config_listener.reset();
    static_cast<void>(stop_server());
    g_status_command.reset();
    g_enabled.reset();
    g_socket_path.clear();
    g_reconcile_error.clear();
    g_compiled_abi_hash.clear();
    g_runtime_abi_hash.clear();
    g_epoch = 0;
}
