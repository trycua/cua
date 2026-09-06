#include "passive_pointer_target.hpp"

#include <cstdlib>
#include <iostream>
#include <memory>

namespace {
struct Window {};
struct Surface {
    int application;
    int client() const { return application; }
};
using Target = cua::hyprland::PassivePointerTarget<std::weak_ptr<Window>, std::weak_ptr<Surface>>;
using cua::hyprland::InputLaneActivity;
struct Owner {
    std::shared_ptr<Window> window;
    std::shared_ptr<Surface> surface;
};

void check(bool condition, const char* message) {
    if (!condition) {
        std::cerr << "FAIL: " << message << '\n';
        std::exit(1);
    }
}
}

int main() {
    const Target::Geometry geometry{20, 30, 500, 400, 20, 30};
    auto window = std::make_shared<Window>();
    auto surface = std::make_shared<Surface>(Surface{1});
    const auto sibling = std::make_shared<Surface>(Surface{1});
    const auto unrelated = std::make_shared<Surface>(Surface{2});
    Target hover;
    check(!hover.entered() && !hover.same_client(surface), "unused lane has no hover conflict");
    {
        auto owner = std::make_unique<Owner>(Owner{window, surface});
        hover.capture(owner->window, owner->surface, geometry);
        check(hover.matches(window, surface, geometry), "capture binds exact target and geometry");
    }
    check(hover.matches(window, surface, geometry), "owner destruction preserves live target hover");
    check(hover.same_client(sibling), "orphan hover still conflicts with another window of its application");
    check(!hover.same_client(unrelated), "orphan hover does not reserve unrelated applications");
    check(hover.reclaimable_for(sibling, {}), "valid new target can retire a matching orphan hover");
    check(!hover.reclaimable_for(unrelated, {}), "fresh target cannot evict unrelated passive hover");
    for (const auto member : {&InputLaneActivity::reserved, &InputLaneActivity::leased,
             &InputLaneActivity::dragging, &InputLaneActivity::button, &InputLaneActivity::keys,
             &InputLaneActivity::keyboard_focus, &InputLaneActivity::capabilities,
             &InputLaneActivity::grant, &InputLaneActivity::expiry}) {
        InputLaneActivity activity;
        activity.*member = true;
        check(!hover.reclaimable_for(sibling, activity),
            "any remaining ownership, authority, or held input prevents sibling eviction");
    }
    Target lane_a, lane_b;
    lane_a.capture(window, surface, geometry);
    lane_b.capture(window, unrelated, geometry);
    // Both previous owners have gone. Driver claims lane A first, but requests
    // B's target; then the next claimant uses lane B for A's former target.
    lane_a.reset();
    check(lane_b.reclaimable_for(unrelated, {}), "opposite-order target B can leave its old orphan lane");
    lane_b.reset();
    lane_a.capture(window, unrelated, geometry);
    const InputLaneActivity active_a{.reserved = true, .leased = true};
    check(!lane_a.reclaimable_for(unrelated, active_a), "active replacement owner stays protected");
    check(!lane_a.same_client(surface), "opposite-order target A remains available to second claimant");
    lane_b.capture(window, surface, geometry);
    check(lane_a.matches(window, unrelated, geometry) && lane_b.matches(window, surface, geometry),
        "both lanes can be reused in reverse target order");
    const Owner next_owner{window, surface};
    check(hover.matches(next_owner.window, next_owner.surface, geometry),
        "fresh owner can reuse unchanged target without inheriting an owner identity");
    auto moved = geometry;
    ++moved[0];
    check(!hover.matches(window, surface, moved), "geometry change invalidates retained hover");
    check(!hover.matches(window, sibling, geometry), "target change invalidates retained hover");
    check(hover.same_target(window, surface), "same target can be checked before geometry refresh");
    check(!hover.same_target(window, sibling), "different target retires old hover even if new geometry fails");
    hover.reset();
    check(!hover.entered() && !hover.surface() && !hover.window() && !hover.same_client(sibling),
        "conflict cleanup removes all target state");
    hover.capture(window, unrelated, geometry);
    check(hover.same_client(unrelated) && !hover.same_client(surface),
        "retargeting the same lane replaces the old application conflict");
    {
        auto ephemeral_window = std::make_shared<Window>();
        auto ephemeral_surface = std::make_shared<Surface>(Surface{3});
        hover.capture(ephemeral_window, ephemeral_surface, geometry);
        check(ephemeral_window.use_count() == 1 && ephemeral_surface.use_count() == 1,
            "passive hover never extends window or surface lifetime");
    }
    check(hover.entered() && !hover.window() && !hover.surface(),
        "destroyed target remains detectable for protocol focus cleanup");
    check(!hover.same_client(surface), "expired target cannot create a stale application conflict");
    check(!hover.matches(std::shared_ptr<Window>{}, std::shared_ptr<Surface>{}, geometry),
        "expired weak references never compare as a usable target");
    hover.reset();
    std::cout << "passive pointer target tests passed\n";
}
