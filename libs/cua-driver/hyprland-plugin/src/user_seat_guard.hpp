#pragma once

#include <cstdint>

namespace cua::hyprland {

// Hooks Hyprland's xdg_popup grab and wl_data_device drag start so they take
// the user's seat only under user_seat_request_allowed (user_seat_policy.hpp).
// Returns whether both hooks are active.
bool install_user_seat_guard(void* plugin);
void remove_user_seat_guard();
bool user_seat_guard_active();
// Requests withheld since the guard was installed.
std::uint64_t popup_grabs_withheld();
std::uint64_t drags_withheld();

} // namespace cua::hyprland
