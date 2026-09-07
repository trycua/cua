#pragma once

#include <array>
#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <string_view>

namespace cua::hyprland {
enum class InputRoute { unbound, independent, primary_foreground };

struct ForegroundSeatBindings {
    unsigned primary_candidates = 0;

    void observe(std::string_view resource_class, bool plugin_owned) {
        if (resource_class == "wl_seat" && !plugin_owned && primary_candidates < 2)
            ++primary_candidates;
    }
    bool unique() const { return primary_candidates == 1; }
};

inline bool foreground_key_modifiers_supported(const std::array<std::uint32_t, 4>& modifiers) {
    // The KEY mapping assumes a neutral US state, including layout group zero.
    return modifiers == std::array<std::uint32_t, 4>{};
}

template <typename Resources>
bool foreground_resources_match(const Resources& current, const Resources& captured) {
    std::size_t live = 0;
    for (const auto& weak : current) {
        const auto resource = weak.lock();
        if (!resource || !resource->good()) continue;
        ++live;
        if (std::none_of(captured.begin(), captured.end(), [&](const auto& saved) {
            return saved.lock() == resource;
        })) return false;
    }
    return live == captured.size();
}

inline bool bind_input_route(InputRoute& bound, InputRoute requested) {
    if (bound != InputRoute::unbound && bound != requested) return false;
    bound = requested;
    return true;
}

struct ForegroundGuard {
    bool exact_root = false;
    bool peer_conflict = false;
    bool physical_keys = false;
    bool physical_buttons = false;
    bool grab = false;
    bool dnd = false;
    bool constraint = false;
    bool exact_keyboard_focus = false;
    bool exact_pointer_focus = false;

    bool can_activate() const {
        return exact_root && !peer_conflict && !physical_keys && !physical_buttons && !grab && !dnd && !constraint;
    }
    bool can_dispatch(bool needs_pointer = true) const {
        return can_activate() && exact_keyboard_focus && (!needs_pointer || exact_pointer_focus);
    }
};
} // namespace cua::hyprland
