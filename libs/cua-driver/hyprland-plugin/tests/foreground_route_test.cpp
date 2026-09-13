#include "foreground_route.hpp"
#include <cstdlib>
#include <initializer_list>
#include <memory>
#include <vector>

using namespace cua::hyprland;
void check(bool value) { if (!value) std::abort(); }
int main() {
    constexpr std::array labels{
        "foreground_none", "foreground_exact_root", "foreground_primary_binding", "foreground_peer_conflict",
        "foreground_physical_keys", "foreground_physical_buttons", "foreground_grab", "foreground_dnd",
        "foreground_constraint", "foreground_keyboard_focus", "foreground_pointer_focus", "foreground_lease",
        "foreground_client_dead", "foreground_session_unavailable", "foreground_unsupported_layout",
        "foreground_lease_expired", "foreground_physical_keyboard", "foreground_keyboard_state",
        "foreground_physical_pointer", "foreground_pointer_target", "foreground_seat_resource",
        "foreground_pointer_resources", "foreground_keyboard_resources", "foreground_keyboard_depressed",
        "foreground_keyboard_latched", "foreground_keyboard_locked", "foreground_keyboard_group",
    };
    static_assert(labels.size() == static_cast<unsigned>(ForegroundFailureReason::keyboard_group) + 1);
    for (unsigned i = 0; i < labels.size(); ++i)
        check(ForegroundFailure{static_cast<ForegroundFailureReason>(i)}.detail() == labels[i]);
    check(ForegroundFailure::code(false) == "primary_target_busy");
    check(ForegroundFailure::code(true) == "foreground_partial_unknown");

    // Exhaust every guard combination against the original admission predicates.
    for (unsigned bits = 0; bits < 1024; ++bits) {
        const ForegroundGuard candidate{
            .exact_root = bool(bits & 1), .primary_binding = bool(bits & 2),
            .peer_conflict = bool(bits & 4), .physical_keys = bool(bits & 8),
            .physical_buttons = bool(bits & 16), .grab = bool(bits & 32),
            .dnd = bool(bits & 64), .constraint = bool(bits & 128),
            .exact_keyboard_focus = bool(bits & 256), .exact_pointer_focus = bool(bits & 512),
        };
        const bool activate = candidate.exact_root && candidate.primary_binding && !candidate.peer_conflict &&
            !candidate.physical_keys && !candidate.physical_buttons && !candidate.grab && !candidate.dnd && !candidate.constraint;
        check(candidate.can_activate() == activate);
        check(candidate.can_dispatch(false) == (activate && candidate.exact_keyboard_focus));
        check(candidate.can_dispatch(true) == (activate && candidate.exact_keyboard_focus && candidate.exact_pointer_focus));
        const auto expected = !candidate.exact_root ? ForegroundFailureReason::exact_root :
            !candidate.primary_binding ? ForegroundFailureReason::primary_binding :
            candidate.peer_conflict ? ForegroundFailureReason::peer_conflict :
            candidate.physical_keys ? ForegroundFailureReason::physical_keys :
            candidate.physical_buttons ? ForegroundFailureReason::physical_buttons :
            candidate.grab ? ForegroundFailureReason::grab : candidate.dnd ? ForegroundFailureReason::dnd :
            candidate.constraint ? ForegroundFailureReason::constraint : ForegroundFailureReason::none;
        check(candidate.activation_failure() == expected);
        for (const bool needs_pointer : {false, true}) {
            const auto dispatch = expected != ForegroundFailureReason::none ? expected :
                !candidate.exact_keyboard_focus ? ForegroundFailureReason::keyboard_focus :
                needs_pointer && !candidate.exact_pointer_focus ? ForegroundFailureReason::pointer_focus : ForegroundFailureReason::none;
            check(candidate.dispatch_failure(needs_pointer) == dispatch);
        }
    }
    struct Resource {
        bool valid = true;
        bool good() const { return valid; }
    };
    auto first = std::make_shared<Resource>();
    auto second = std::make_shared<Resource>();
    std::vector<std::weak_ptr<Resource>> current{first}, captured{first};
    check(foreground_resources_match(current, captured));
    current.push_back(second);
    check(!foreground_resources_match(current, captured));
    current = {second};
    check(!foreground_resources_match(current, captured));
    current = {first};
    first->valid = false;
    check(!foreground_resources_match(current, captured));
    first.reset();
    check(!foreground_resources_match(current, captured));
    current = {second};
    captured = current;
    check(foreground_resources_match(current, captured));
    current.clear();
    check(!foreground_resources_match(current, captured));

    ForegroundSeatBindings bindings;
    bindings.observe("wl_surface", false);
    bindings.observe("wl_keyboard", false);
    bindings.observe("wl_seat", true); // Cua-Agent
    bindings.observe("wl_seat", true); // Cua-Agent-2
    check(!bindings.unique());
    bindings.observe("wl_seat", false); // Primary binding
    check(bindings.unique());
    // A second binding, including one created during a drag, must refuse.
    bindings.observe("wl_seat", false);
    check(!bindings.unique());
    bindings.observe("wl_seat", true);
    check(!bindings.unique());
    // Saturation cannot make an arbitrarily large resource count unique.
    for (unsigned i = 0; i < 1000; ++i) bindings.observe("wl_seat", false);
    check(!bindings.unique());

    std::array<std::uint32_t, 4> modifiers{};
    constexpr std::array modifier_reasons{ForegroundFailureReason::keyboard_depressed,
        ForegroundFailureReason::keyboard_latched, ForegroundFailureReason::keyboard_locked, ForegroundFailureReason::keyboard_group};
    check(foreground_key_modifiers_supported(modifiers));
    check(foreground_key_modifier_failure(modifiers) == ForegroundFailureReason::none);
    for (unsigned i = 0; i < modifiers.size(); ++i) {
        // Depressed, latched, locked, and nonzero layout group each refuse.
        modifiers[i] = 1;
        check(!foreground_key_modifiers_supported(modifiers));
        check(foreground_key_modifier_failure(modifiers) == modifier_reasons[i]);
        modifiers[i] = 0x80000000u;
        check(!foreground_key_modifiers_supported(modifiers));
        check(foreground_key_modifier_failure(modifiers) == modifier_reasons[i]);
        modifiers[i] = 0;
    }
    check(foreground_key_modifiers_supported(modifiers));

    InputRoute route = InputRoute::unbound;
    check(bind_input_route(route, InputRoute::primary_foreground));
    check(bind_input_route(route, InputRoute::primary_foreground));
    check(!bind_input_route(route, InputRoute::independent));
    check(route == InputRoute::primary_foreground);
    route = InputRoute::independent;
    check(!bind_input_route(route, InputRoute::primary_foreground));

    ForegroundGuard guard{.exact_root = true};
    check(guard.can_activate());
    check(!guard.can_dispatch());
    guard.exact_keyboard_focus = true;
    check(!guard.can_dispatch());
    check(guard.can_dispatch(false));
    guard.exact_pointer_focus = true;
    check(guard.can_dispatch());
    for (auto member : {&ForegroundGuard::peer_conflict, &ForegroundGuard::physical_keys,
                       &ForegroundGuard::physical_buttons, &ForegroundGuard::grab,
                       &ForegroundGuard::dnd, &ForegroundGuard::constraint}) {
        guard.*member = true;
        check(!guard.can_activate());
        check(!guard.can_dispatch());
        guard.*member = false;
    }
    guard.exact_root = false;
    check(!guard.can_activate());
    check(!guard.can_dispatch());
    guard.exact_root = true;
    guard.exact_pointer_focus = false;
    check(!guard.can_dispatch());
    guard.exact_pointer_focus = true;
    guard.exact_keyboard_focus = false;
    check(!guard.can_dispatch());
    check(!guard.can_dispatch(false));
}
