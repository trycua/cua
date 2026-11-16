#include "user_seat_guard.hpp"

#include "user_seat_policy.hpp"

#include <src/managers/SeatManager.hpp>
#include <src/plugins/PluginAPI.hpp>
#include <src/protocols/XDGShell.hpp>
#include <src/protocols/core/Compositor.hpp>
#include <src/protocols/core/DataDevice.hpp>

#include <atomic>
#include <string>

namespace cua::hyprland {
namespace {

void* g_plugin = nullptr;
CFunctionHook* g_add_or_start_grab = nullptr;
CFunctionHook* g_initiate_drag = nullptr;
std::atomic<std::uint64_t> g_grabs_withheld = 0;
std::atomic<std::uint64_t> g_drags_withheld = 0;

// The one place an xdg_popup.grab request reaches the seat, and the one place a
// wl_data_device.start_drag request does, in Hyprland 0.56.2 and 0.57.
constexpr auto kAddOrStartGrab =
    "_ZN17CXDGShellProtocol14addOrStartGrabEN9Hyprutils6Memory14CSharedPointerI17CXDGPopupResourceEE";
constexpr auto kInitiateDrag =
    "_ZN21CWLDataDeviceProtocol12initiateDragEN9Hyprutils6Memory12CWeakPointerI21CWLDataSourceResourceEENS1_"
    "14CSharedPointerI18CWLSurfaceResourceEES7_";

using AddOrStartGrab = void (*)(CXDGShellProtocol*, SP<CXDGPopupResource>);
using InitiateDrag = void (*)(CWLDataDeviceProtocol*, WP<CWLDataSourceResource>, SP<CWLSurfaceResource>,
    SP<CWLSurfaceResource>);

const wl_client* client_of(const SP<CWLSurfaceResource>& surface) {
    return surface ? surface->client() : nullptr;
}

bool user_holds(const SP<CWLSurfaceResource>& surface) {
    return user_seat_request_allowed(client_of(surface), client_of(g_pSeatManager->m_state.keyboardFocus.lock()),
        client_of(g_pSeatManager->m_state.pointerFocus.lock()));
}

// A withheld popup still maps and works, without the grab.
void guarded_add_or_start_grab(CXDGShellProtocol* protocol, SP<CXDGPopupResource> popup) {
    const auto xdg = popup ? popup->m_surface.lock() : nullptr;
    if (!user_holds(xdg ? xdg->m_surface.lock() : nullptr)) {
        g_grabs_withheld.fetch_add(1, std::memory_order_relaxed);
        return;
    }
    reinterpret_cast<AddOrStartGrab>(g_add_or_start_grab->m_original)(protocol, popup);
}

// A withheld drag is cancelled at once, as wl_data_source.cancelled tells the
// app, rather than left to follow the user's pointer.
void guarded_initiate_drag(CWLDataDeviceProtocol* protocol, WP<CWLDataSourceResource> source,
    SP<CWLSurfaceResource> icon, SP<CWLSurfaceResource> origin) {
    if (!user_holds(origin)) {
        g_drags_withheld.fetch_add(1, std::memory_order_relaxed);
        if (const auto live = source.lock())
            live->cancelled();
        return;
    }
    reinterpret_cast<InitiateDrag>(g_initiate_drag->m_original)(protocol, source, icon, origin);
}

CFunctionHook* hook_exact(void* plugin, const std::string& symbol, void* replacement) {
    const auto matches = HyprlandAPI::findFunctionsByName(plugin, symbol);
    if (matches.size() != 1)
        return nullptr;
    auto* hook = HyprlandAPI::createFunctionHook(plugin, matches.front().address, replacement);
    if (hook && !hook->hook()) {
        HyprlandAPI::removeFunctionHook(plugin, hook);
        return nullptr;
    }
    return hook;
}

} // namespace

bool install_user_seat_guard(void* plugin) {
    if (user_seat_guard_active())
        return true;
    g_plugin = plugin;
    g_add_or_start_grab =
        hook_exact(plugin, kAddOrStartGrab, reinterpret_cast<void*>(&guarded_add_or_start_grab));
    g_initiate_drag = hook_exact(plugin, kInitiateDrag, reinterpret_cast<void*>(&guarded_initiate_drag));
    // All or nothing: half a guard would still let one kind of request through.
    if (!user_seat_guard_active())
        remove_user_seat_guard();
    return user_seat_guard_active();
}

void remove_user_seat_guard() {
    for (auto** hook : {&g_add_or_start_grab, &g_initiate_drag}) {
        if (*hook)
            HyprlandAPI::removeFunctionHook(g_plugin, *hook);
        *hook = nullptr;
    }
    g_plugin = nullptr;
}

bool user_seat_guard_active() {
    return g_add_or_start_grab && g_initiate_drag;
}

std::uint64_t popup_grabs_withheld() {
    return g_grabs_withheld.load(std::memory_order_relaxed);
}

std::uint64_t drags_withheld() {
    return g_drags_withheld.load(std::memory_order_relaxed);
}

} // namespace cua::hyprland
