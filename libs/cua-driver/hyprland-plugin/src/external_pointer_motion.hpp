#pragma once

#include <cmath>
#include <optional>

namespace cua::hyprland {
// Tells the user's pointer motion apart from Hyprland's own refocus passes.
//
// Hyprland emits input.mouse.move from mouseMoveUnified. A device motion only
// reaches that emission when the floored cursor position changed. Hyprland
// also runs the same function with no device event at all, through
// simulateMouseMovement() and refocus(): when a layer surface, popup or
// subsurface maps, unmaps or commits under the pointer, when a workspace or
// monitor changes, and so on. Those passes carry the unchanged position.
//
// Treating every emission as the user's input made foreground typing fail
// part-way: a notification or bar mapping during session start cancelled the
// lane, and the next key of a still-valid target was refused. Only a position
// change that the plugin did not make itself is the user's motion.
class ExternalPointerMotion {
  public:
    struct Point {
        double x = 0, y = 0;
        bool operator==(const Point&) const = default;
    };
    // Record a position that is not user motion: the position when listening
    // starts, and the position right after the plugin warps the cursor.
    void rebase(Point position) { last_ = floored(position); }
    // Called for every input.mouse.move emission. True when the user moved the
    // pointer. Without a known previous position it cannot tell, so it answers
    // true and keeps the conservative behaviour.
    bool user_moved(Point position) {
        const auto now = floored(position);
        const bool moved = !last_ || *last_ != now;
        last_ = now;
        return moved;
    }

  private:
    static Point floored(Point position) { return {std::floor(position.x), std::floor(position.y)}; }
    std::optional<Point> last_;
};
} // namespace cua::hyprland
