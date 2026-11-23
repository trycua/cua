#pragma once

#include <cmath>
#include <cstdint>

namespace cua::hyprland {
// The driver sends 10 logical units per wheel click. Some apps, such as Zed,
// ignore a wheel scroll that does not also say how many clicks it was:
// axis_value120 from wl_pointer version 8, axis_discrete for versions 5 to 7.
// Send it before the matching axis event, in the same frame.
template <typename Pointer, typename Axis>
void send_wheel_steps(Pointer& pointer, Axis axis, double value) {
    // Neither event may carry zero, so a scroll too small to round to a step
    // sends only the axis event.
    if (pointer.version() >= 8) {
        const auto value120 = static_cast<std::int32_t>(std::lround(value * 12));
        if (value120 != 0) pointer.sendAxisValue120(axis, value120);
    } else if (pointer.version() >= 5) {
        const auto clicks = static_cast<std::int32_t>(std::lround(value / 10));
        if (clicks != 0) pointer.sendAxisDiscrete(axis, clicks);
    }
}
}
