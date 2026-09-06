#pragma once

#include <array>

namespace cua::hyprland {
struct InputLaneActivity {
    bool reserved = false, leased = false, dragging = false;
    bool button = false, keys = false, keyboard_focus = false;
    bool capabilities = false, grant = false, expiry = false;

    [[nodiscard]] bool inert() const {
        return !reserved && !leased && !dragging && !button && !keys &&
            !keyboard_focus && !capabilities && !grant && !expiry;
    }
};

// Pointer presence outlives the transport owner, but never owns that owner,
// its grant, or the target. Window/surface references must be weak references.
template <typename WindowRef, typename SurfaceRef>
class PassivePointerTarget {
  public:
    using Geometry = std::array<double, 6>;

    template <typename Window, typename Surface>
    void capture(const Window& window, const Surface& surface, const Geometry& geometry) {
        window_ = window;
        surface_ = surface;
        geometry_ = geometry;
        entered_ = true;
    }
    void reset() {
        window_.reset();
        surface_.reset();
        geometry_ = {};
        entered_ = false;
    }
    [[nodiscard]] bool entered() const { return entered_; }
    [[nodiscard]] auto window() const { return window_.lock(); }
    [[nodiscard]] auto surface() const { return surface_.lock(); }
    [[nodiscard]] const Geometry& geometry() const { return geometry_; }

    template <typename Window, typename Surface>
    [[nodiscard]] bool same_target(const Window& window, const Surface& surface) const {
        return entered_ && window && surface && window_.lock() == window &&
            surface_.lock() == surface;
    }
    template <typename Window, typename Surface>
    [[nodiscard]] bool matches(const Window& window, const Surface& surface, const Geometry& geometry) const {
        return same_target(window, surface) && geometry_ == geometry;
    }
    template <typename Surface>
    [[nodiscard]] bool same_client(const Surface& candidate) const {
        const auto current = surface_.lock();
        return entered_ && current && candidate && current->client() == candidate->client();
    }
    template <typename Surface>
    [[nodiscard]] bool reclaimable_for(const Surface& candidate, const InputLaneActivity& activity) const {
        return activity.inert() && same_client(candidate);
    }

  private:
    WindowRef window_;
    SurfaceRef surface_;
    Geometry geometry_{};
    bool entered_ = false;
};
} // namespace cua::hyprland
