#include "ime_popup_guard.hpp"

#include <src/managers/input/InputMethodPopup.hpp>
#include <src/plugins/PluginAPI.hpp>
#include <src/protocols/core/Compositor.hpp>

namespace cua::hyprland {
namespace {

void* g_plugin = nullptr;
CFunctionHook* g_update_box = nullptr;
CFunctionHook* g_should_be_rendered = nullptr;

// Hyprland 0.56.2 leaves an input-method popup mapped after its wl_surface is
// destroyed, then places and renders it through the missing surface. fcitx5
// registers an input method per wl_seat, which is how the agent seats make that
// order reachable when it exits. A popup without a surface is neither placed
// nor rendered.
void guarded_update_box(void* popup) {
    if (!static_cast<CInputPopup*>(popup)->getSurface())
        return;
    reinterpret_cast<void (*)(void*)>(g_update_box->m_original)(popup);
}

bool guarded_should_be_rendered(void* popup) {
    if (!static_cast<CInputPopup*>(popup)->getSurface())
        return false;
    return reinterpret_cast<bool (*)(void*)>(g_should_be_rendered->m_original)(popup);
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

bool install_ime_popup_guard(void* plugin) {
    if (ime_popup_guard_active())
        return true;
    g_plugin = plugin;
    g_update_box = hook_exact(plugin, "_ZN11CInputPopup9updateBoxEv", reinterpret_cast<void*>(&guarded_update_box));
    g_should_be_rendered = hook_exact(plugin, "_ZN11CInputPopup16shouldBeRenderedEv",
        reinterpret_cast<void*>(&guarded_should_be_rendered));
    if (!ime_popup_guard_active())
        remove_ime_popup_guard();
    return ime_popup_guard_active();
}

void remove_ime_popup_guard() {
    for (auto* hook : {g_update_box, g_should_be_rendered})
        if (hook)
            HyprlandAPI::removeFunctionHook(g_plugin, hook);
    g_update_box = g_should_be_rendered = nullptr;
    g_plugin = nullptr;
}

bool ime_popup_guard_active() {
    return g_update_box && g_should_be_rendered;
}

} // namespace cua::hyprland
