#pragma once

#include <memory>
#include <string>
#include <array>

namespace cua::hyprland {
class PrimaryTrace;
// Compiled only with the explicit VM-test option. Not the production v3 API.
class InputExperiment {
  public:
    explicit InputExperiment(const std::string& instance_directory, void* plugin);
    ~InputExperiment();
    InputExperiment(const InputExperiment&) = delete;
    InputExperiment& operator=(const InputExperiment&) = delete;
    std::string status_json() const;

  private:
    struct Impl;
    std::array<std::unique_ptr<Impl>, 2> lanes_;
    std::unique_ptr<PrimaryTrace> trace_;
};
} // namespace cua::hyprland
