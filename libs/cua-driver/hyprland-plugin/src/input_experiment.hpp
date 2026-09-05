#pragma once

#include <memory>
#include <string>

namespace cua::hyprland {
// Compiled only with the explicit VM-test option. Not the production v3 API.
class InputExperiment {
  public:
    explicit InputExperiment(const std::string& instance_directory);
    ~InputExperiment();
    InputExperiment(const InputExperiment&) = delete;
    InputExperiment& operator=(const InputExperiment&) = delete;
    std::string status_json() const;

  private:
    struct Impl;
    std::unique_ptr<Impl> impl_;
};
} // namespace cua::hyprland
