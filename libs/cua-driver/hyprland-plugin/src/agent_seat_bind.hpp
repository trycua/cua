#pragma once

#include <cstdint>
#include <functional>

struct wl_client;

namespace cua::hyprland {

// Called when a client binds the user's wl_seat global, with the client and the
// bind's version and new object id. It returns true when it bound an agent seat
// for the client instead; false leaves the bind to Hyprland.
using AgentSeatBinder = std::function<bool(wl_client*, std::uint32_t version, std::uint32_t id)>;

// Hooks Hyprland's wl_seat bind so a client the driver tagged (agent_seat_tag.hpp)
// gets an agent seat as its only wl_seat. No display-wide global filter is
// installed: every other client's bind is Hyprland's own, unchanged.
// Returns whether the hook is active.
bool install_agent_seat_bind(void* plugin);
void remove_agent_seat_bind();
bool agent_seat_bind_active();
// Empty clears. Set only while the owner of the binder is alive.
void set_agent_seat_binder(AgentSeatBinder binder);
// Binds the hook handed to an agent seat since it was installed.
std::uint64_t agent_seat_binds();

} // namespace cua::hyprland
