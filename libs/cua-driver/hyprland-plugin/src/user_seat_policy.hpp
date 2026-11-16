#pragma once

struct wl_client;

namespace cua::hyprland {

// Two client requests take the user's whole seat in Hyprland 0.56 and 0.57,
// whatever seat and input serial they name:
//
// - xdg_popup.grab: the user's keyboard focus moves to the popup until it
//   closes;
// - wl_data_device.start_drag: the drag follows the user's pointer and drops
//   on the user's next button release, and ending it re-sends the user's
//   keyboard focus.
//
// A popup or drag the person did not start (one an accessibility action or an
// agent seat started in a background app) then takes over their keyboard and
// pointer. One the person starts always comes from the client that holds
// their keyboard or pointer focus, so only that client may take the seat.
inline bool user_seat_request_allowed(const wl_client* requester, const wl_client* keyboard_focus,
    const wl_client* pointer_focus) {
    return requester && (requester == keyboard_focus || requester == pointer_focus);
}

} // namespace cua::hyprland
