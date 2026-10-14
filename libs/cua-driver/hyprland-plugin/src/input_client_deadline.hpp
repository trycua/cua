#pragma once

#include <chrono>

namespace cua::hyprland {
// Pre-HELLO traffic cannot renew the acceptance deadline. Once initialized,
// the connection retains the existing rolling idle timeout.
class InputClientDeadline {
  public:
    using Clock = std::chrono::steady_clock;
    explicit InputClientDeadline(Clock::time_point accepted = Clock::now())
        : accepted_(accepted), activity_(accepted) {}

    bool expired(bool initialized, Clock::time_point now) const {
        return initialized ? now - activity_ >= std::chrono::seconds(60)
                           : now - accepted_ >= std::chrono::seconds(5);
    }
    void touch(Clock::time_point now) { activity_ = now; }

  private:
    const Clock::time_point accepted_;
    Clock::time_point activity_;
};
} // namespace cua::hyprland
