#pragma once

#include <array>
#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <string_view>

namespace cua::hyprland {
enum class InputRoute { unbound, independent, primary_foreground };

enum class ForegroundFailureReason {
    none, exact_root, primary_binding, peer_conflict, physical_keys, physical_buttons,
    grab, dnd, constraint, keyboard_focus, pointer_focus, lease, client_dead,
    session_unavailable, unsupported_layout, lease_expired, physical_keyboard,
    keyboard_state, physical_pointer, pointer_target, seat_resource, pointer_resources,
    keyboard_resources, keyboard_depressed, keyboard_latched, keyboard_locked, keyboard_group,
};

struct ForegroundFailure {
    ForegroundFailureReason reason;

    std::string_view detail() const {
        switch (reason) {
        case ForegroundFailureReason::none: return "foreground_none";
        case ForegroundFailureReason::exact_root: return "foreground_exact_root";
        case ForegroundFailureReason::primary_binding: return "foreground_primary_binding";
        case ForegroundFailureReason::peer_conflict: return "foreground_peer_conflict";
        case ForegroundFailureReason::physical_keys: return "foreground_physical_keys";
        case ForegroundFailureReason::physical_buttons: return "foreground_physical_buttons";
        case ForegroundFailureReason::grab: return "foreground_grab";
        case ForegroundFailureReason::dnd: return "foreground_dnd";
        case ForegroundFailureReason::constraint: return "foreground_constraint";
        case ForegroundFailureReason::keyboard_focus: return "foreground_keyboard_focus";
        case ForegroundFailureReason::pointer_focus: return "foreground_pointer_focus";
        case ForegroundFailureReason::lease: return "foreground_lease";
        case ForegroundFailureReason::client_dead: return "foreground_client_dead";
        case ForegroundFailureReason::session_unavailable: return "foreground_session_unavailable";
        case ForegroundFailureReason::unsupported_layout: return "foreground_unsupported_layout";
        case ForegroundFailureReason::lease_expired: return "foreground_lease_expired";
        case ForegroundFailureReason::physical_keyboard: return "foreground_physical_keyboard";
        case ForegroundFailureReason::keyboard_state: return "foreground_keyboard_state";
        case ForegroundFailureReason::physical_pointer: return "foreground_physical_pointer";
        case ForegroundFailureReason::pointer_target: return "foreground_pointer_target";
        case ForegroundFailureReason::seat_resource: return "foreground_seat_resource";
        case ForegroundFailureReason::pointer_resources: return "foreground_pointer_resources";
        case ForegroundFailureReason::keyboard_resources: return "foreground_keyboard_resources";
        case ForegroundFailureReason::keyboard_depressed: return "foreground_keyboard_depressed";
        case ForegroundFailureReason::keyboard_latched: return "foreground_keyboard_latched";
        case ForegroundFailureReason::keyboard_locked: return "foreground_keyboard_locked";
        case ForegroundFailureReason::keyboard_group: return "foreground_keyboard_group";
        }
        return "foreground_unknown";
    }

    static std::string_view code(bool attempted) {
        return attempted ? "foreground_partial_unknown" : "primary_target_busy";
    }
};

struct ForegroundSeatBindings {
    unsigned primary_candidates = 0;

    void observe(std::string_view resource_class, bool plugin_owned) {
        if (resource_class == "wl_seat" && !plugin_owned && primary_candidates < 2)
            ++primary_candidates;
    }
    bool unique() const { return primary_candidates == 1; }
};

inline ForegroundFailureReason foreground_key_modifier_failure(const std::array<std::uint32_t, 4>& modifiers) {
    // The KEY mapping assumes a neutral US state, including layout group zero.
    if (modifiers[0]) return ForegroundFailureReason::keyboard_depressed;
    if (modifiers[1]) return ForegroundFailureReason::keyboard_latched;
    if (modifiers[2]) return ForegroundFailureReason::keyboard_locked;
    if (modifiers[3]) return ForegroundFailureReason::keyboard_group;
    return ForegroundFailureReason::none;
}

inline bool foreground_key_modifiers_supported(const std::array<std::uint32_t, 4>& modifiers) {
    return foreground_key_modifier_failure(modifiers) == ForegroundFailureReason::none;
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
    bool primary_binding = true;
    bool peer_conflict = false;
    bool physical_keys = false;
    bool physical_buttons = false;
    bool grab = false;
    bool dnd = false;
    bool constraint = false;
    bool exact_keyboard_focus = false;
    bool exact_pointer_focus = false;

    ForegroundFailureReason activation_failure() const {
        if (!exact_root) return ForegroundFailureReason::exact_root;
        if (!primary_binding) return ForegroundFailureReason::primary_binding;
        if (peer_conflict) return ForegroundFailureReason::peer_conflict;
        if (physical_keys) return ForegroundFailureReason::physical_keys;
        if (physical_buttons) return ForegroundFailureReason::physical_buttons;
        if (grab) return ForegroundFailureReason::grab;
        if (dnd) return ForegroundFailureReason::dnd;
        if (constraint) return ForegroundFailureReason::constraint;
        return ForegroundFailureReason::none;
    }
    ForegroundFailureReason dispatch_failure(bool needs_pointer = true) const {
        const auto failure = activation_failure();
        if (failure != ForegroundFailureReason::none) return failure;
        if (!exact_keyboard_focus) return ForegroundFailureReason::keyboard_focus;
        if (needs_pointer && !exact_pointer_focus) return ForegroundFailureReason::pointer_focus;
        return ForegroundFailureReason::none;
    }
    bool can_activate() const { return activation_failure() == ForegroundFailureReason::none; }
    bool can_dispatch(bool needs_pointer = true) const { return dispatch_failure(needs_pointer) == ForegroundFailureReason::none; }
};
} // namespace cua::hyprland
