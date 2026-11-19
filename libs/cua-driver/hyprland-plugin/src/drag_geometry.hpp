#pragma once

#include <cstdint>

namespace cua::hyprland {
// A drag is bound to the geometry accepted at dispatch. Other requests may
// refresh the client's cached revision, but cannot rebase an in-flight drag.
class DragGeometry {
  public:
    explicit constexpr DragGeometry(std::uint64_t revision) : accepted_(revision) {}
    [[nodiscard]] constexpr bool matches(std::uint64_t current) const { return current == accepted_; }

  private:
    const std::uint64_t accepted_;
};
} // namespace cua::hyprland
