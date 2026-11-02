#pragma once

// Hyprland 0.57 moved window state behind a backend object and renamed the
// exclusive layer-surface list. CMake defines CUA_HYPRLAND_WINDOW_BACKEND_API=1
// for 0.57 or newer headers and 0 for 0.56.x. Include after <src/Compositor.hpp>.
#ifndef CUA_HYPRLAND_WINDOW_BACKEND_API
#error CMake must define CUA_HYPRLAND_WINDOW_BACKEND_API
#endif

namespace cua::hyprland::compat {

inline bool window_mapped(const PHLWINDOW& window) {
#if CUA_HYPRLAND_WINDOW_BACKEND_API
    return window->mapped();
#else
    return window->m_isMapped;
#endif
}

inline bool window_is_x11(const PHLWINDOW& window) {
#if CUA_HYPRLAND_WINDOW_BACKEND_API
    return window->backend().isX11();
#else
    return window->m_isX11;
#endif
}

inline pid_t window_pid(const PHLWINDOW& window) {
#if CUA_HYPRLAND_WINDOW_BACKEND_API
    return window->backend().pid();
#else
    return window->getPID();
#endif
}

// Layer surfaces holding exclusive keyboard interactivity. 0.57 stopped letting
// them gate pointer focus, so the list is named for the keyboard; the plugin
// still treats any of them as a grab and refuses.
inline bool exclusive_layer_surface_active() {
#if CUA_HYPRLAND_WINDOW_BACKEND_API
    return !g_pInputManager->m_exclusiveKeyboardLSes.empty();
#else
    return !g_pInputManager->m_exclusiveLSes.empty();
#endif
}

} // namespace cua::hyprland::compat
