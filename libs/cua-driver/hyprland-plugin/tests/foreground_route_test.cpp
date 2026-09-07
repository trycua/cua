#include "foreground_route.hpp"
#include <cstdlib>
#include <initializer_list>
#include <memory>
#include <vector>

using namespace cua::hyprland;
void check(bool value) { if (!value) std::abort(); }
int main() {
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
    check(foreground_key_modifiers_supported(modifiers));
    for (unsigned i = 0; i < modifiers.size(); ++i) {
        // Depressed, latched, locked, and nonzero layout group each refuse.
        modifiers[i] = 1;
        check(!foreground_key_modifiers_supported(modifiers));
        modifiers[i] = 0x80000000u;
        check(!foreground_key_modifiers_supported(modifiers));
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
