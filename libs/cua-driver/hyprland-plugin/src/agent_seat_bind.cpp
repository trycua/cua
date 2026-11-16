#include "agent_seat_bind.hpp"

#include <src/plugins/PluginAPI.hpp>
#include <src/protocols/core/Seat.hpp>

#include <atomic>
#include <string>

namespace cua::hyprland {
namespace {

void* g_plugin = nullptr;
CFunctionHook* g_bind_manager = nullptr;
AgentSeatBinder g_binder;
std::atomic<std::uint64_t> g_binds = 0;

// CWLSeatProtocol::bindManager(wl_client*, void*, uint32_t, uint32_t): the one
// place a client's bind of the user's wl_seat global reaches Hyprland, the same
// in 0.56.2 and 0.57.
constexpr auto kBindManager = "_ZN15CWLSeatProtocol11bindManagerEP9wl_clientPvjj";

using BindManager = void (*)(CWLSeatProtocol*, wl_client*, void*, std::uint32_t, std::uint32_t);

void guarded_bind_manager(CWLSeatProtocol* protocol, wl_client* client, void* data, std::uint32_t version,
    std::uint32_t id) {
    if (g_binder && g_binder(client, version, id)) {
        g_binds.fetch_add(1, std::memory_order_relaxed);
        return;
    }
    reinterpret_cast<BindManager>(g_bind_manager->m_original)(protocol, client, data, version, id);
}

} // namespace

bool install_agent_seat_bind(void* plugin) {
    if (agent_seat_bind_active())
        return true;
    const auto matches = HyprlandAPI::findFunctionsByName(plugin, kBindManager);
    if (matches.size() != 1)
        return false;
    auto* hook = HyprlandAPI::createFunctionHook(plugin, matches.front().address,
        reinterpret_cast<void*>(&guarded_bind_manager));
    if (!hook)
        return false;
    if (!hook->hook()) {
        HyprlandAPI::removeFunctionHook(plugin, hook);
        return false;
    }
    g_plugin = plugin;
    g_bind_manager = hook;
    return true;
}

void remove_agent_seat_bind() {
    g_binder = nullptr;
    if (g_bind_manager)
        HyprlandAPI::removeFunctionHook(g_plugin, g_bind_manager);
    g_bind_manager = nullptr;
    g_plugin = nullptr;
}

bool agent_seat_bind_active() {
    return g_bind_manager != nullptr;
}

void set_agent_seat_binder(AgentSeatBinder binder) {
    g_binder = std::move(binder);
}

std::uint64_t agent_seat_binds() {
    return g_binds.load(std::memory_order_relaxed);
}

} // namespace cua::hyprland
