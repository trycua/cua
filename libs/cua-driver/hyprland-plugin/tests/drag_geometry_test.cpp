#include "drag_geometry.hpp"

#include <array>
#include <cstdlib>
#include <cstdint>

namespace {
void check(bool condition) { if (!condition) std::abort(); }
}

int main() {
    using cua::hyprland::DragGeometry;
    std::array<int, 4> cached{10, 10, 640, 480};
    std::uint64_t revision = 2;
    const auto refresh = [&](const std::array<int, 4>& actual) {
        if (actual != cached) { cached = actual; ++revision; }
    };
    const DragGeometry drag{revision};
    check(drag.matches(revision));

    // Simulate a resize consumed by request()/approve() before their busy
    // refusal. The timer refresh sees no new change; the drag must still fail.
    const std::array<int, 4> resized{10, 10, 800, 600};
    refresh(resized);
    const auto before_tick = revision;
    refresh(resized);
    check(before_tick == revision);
    check(!drag.matches(revision));

    // Moving back cannot revive old coordinates or old authority.
    refresh({10, 10, 640, 480});
    check(!drag.matches(revision));
    const DragGeometry newly_authorized{revision};
    check(newly_authorized.matches(revision));
    refresh({20, 30, 640, 480});
    check(!newly_authorized.matches(revision));
}
