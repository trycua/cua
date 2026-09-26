#pragma once

#include <memory>
#include <string>
#include <array>
#include <cstddef>

namespace cua::hyprland {
// Independent agent lanes (virtual wl_seats + sockets). Lane 0 is
// `cua-input-v3.sock`/`Cua-Agent`; lane N>0 is `cua-input-v3-<N+1>.sock`/
// `Cua-Agent-<N+1>`. Driver's `hyprland_input::MAX_LANES` must match.
inline constexpr std::size_t kInputLanes = 4;
class PrimaryTrace;
// Shared native seat/lifetime implementation. Build options select production
// v3 admission or the historical signed experiment; never both.
class InputExperiment {
  public:
    explicit InputExperiment(const std::string& instance_directory, void* plugin);
    ~InputExperiment();
    InputExperiment(const InputExperiment&) = delete;
    InputExperiment& operator=(const InputExperiment&) = delete;
    // Config toggles change admission, not client-owned Wayland resources.
    void suspend();
    void resume();
    std::string status_json() const;

  private:
    struct Impl;
    struct DesktopListeners;
    std::array<std::unique_ptr<Impl>, kInputLanes> lanes_;
    std::unique_ptr<PrimaryTrace> trace_;
    std::unique_ptr<DesktopListeners> desktop_listeners_;
};
} // namespace cua::hyprland
