#pragma once

#include <memory>
#include <string>
#include <array>

namespace cua::hyprland {
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
    std::array<std::unique_ptr<Impl>, 2> lanes_;
    std::unique_ptr<PrimaryTrace> trace_;
    std::unique_ptr<DesktopListeners> desktop_listeners_;
};
} // namespace cua::hyprland
