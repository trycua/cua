#pragma once

#include <functional>
#include <memory>
#include <string>

struct wl_resource;
namespace cua::hyprland {
// Test instrumentation, never part of the discovery-only production build.
// No input is emitted. Capture is bounded, explicit, and kept in memory.
class PrimaryTrace {
  public:
    PrimaryTrace(void* plugin, std::function<unsigned(wl_resource*)> actor);
    ~PrimaryTrace();
    std::string request(const std::string& command, unsigned after = 0);
    void mark(const char* kind, unsigned actor);
  private:
    struct Impl;
    std::unique_ptr<Impl> impl_;
};
}
