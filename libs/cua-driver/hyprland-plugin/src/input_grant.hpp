#pragma once

#include <chrono>
#include <cstdint>

namespace cua::hyprland {
// Technical per-dispatch lifetime, not an independent permission policy.
class InputGrant {
  public:
    using Clock = std::chrono::steady_clock;
    static constexpr auto lifetime = std::chrono::seconds(5);
    static constexpr bool single_operation(std::uint64_t capability) {
        return capability == 1 || capability == 2 || capability == 4 || capability == 8 || capability == 16;
    }
    bool arm(std::uint64_t capability, Clock::time_point now) {
        reset();
        if (!single_operation(capability)) return false;
        capability_ = capability;
        deadline_ = now + lifetime;
        return true;
    }
    bool permits(std::uint64_t capability, Clock::time_point now) const {
        return single_operation(capability) && capability_ == capability && now < deadline_;
    }
    bool consume(std::uint64_t capability, Clock::time_point now) {
        const bool permitted = permits(capability, now);
        // Even a mismatched or expired dispatch cannot retain authority.
        reset();
        return permitted;
    }
    void reset() { capability_ = 0; deadline_ = {}; }
    Clock::time_point deadline() const { return deadline_; }
  private:
    std::uint64_t capability_ = 0;
    Clock::time_point deadline_{};
};
} // namespace cua::hyprland
