#pragma once

#include <cstdint>

namespace cua::hyprland {
// Content-free count of input a person produces: keys from bus, physical-device
// and virtual-device listeners, pointer device motion, buttons, wheel and
// gestures, every touch and tablet event,
// lid and tablet-mode switches, and device arrivals except protocol virtual
// keyboards. Excluded sources are virtual keyboard creation and discovery, the
// unified motion notification, because Hyprland reports its own warps and
// refocus through it (real motion is counted per device instead), and
// modifier-only and keymap updates. Input methods (an fcitx5 virtual keyboard)
// send modifier-only updates whenever a text field gains focus. Callers compare two
// readings; no event content, position or key is retained.
enum class ExternalInputKind {
    key,
    pointer_motion,
    button,
    axis,
    gesture,
    touch,
    tablet,
    switch_toggle,
    device_added,
    unified_motion,
};

class ExternalInputCount {
  public:
    void note(ExternalInputKind kind) {
        if (kind != ExternalInputKind::unified_motion) ++count_;
    }
    std::uint64_t value() const { return count_; }

  private:
    std::uint64_t count_ = 0;
};
} // namespace cua::hyprland
