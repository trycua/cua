#pragma once

namespace cua::hyprland {

// Installs the guard described in ime_popup_guard.cpp. Returns whether it is active.
bool install_ime_popup_guard(void* plugin);
void remove_ime_popup_guard();
bool ime_popup_guard_active();

} // namespace cua::hyprland
