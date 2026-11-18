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
#if defined(CUA_HYPRLAND_TEST_INPUT) || defined(CUA_HYPRLAND_INPUT_TRACE)
    PrimaryTrace(void* plugin, std::function<unsigned(wl_resource*)> actor);
    ~PrimaryTrace();
    std::string request(const std::string& command, unsigned after = 0);
    void mark(const char* kind, unsigned actor);
  private:
    struct Impl;
    std::unique_ptr<Impl> impl_;
#else
    // Uninstrumented production has no hooks, trace buffer, or trace commands.
    ~PrimaryTrace() = default;
    std::string request(const std::string&, unsigned = 0) { return {}; }
    void mark(const char*, unsigned) {}
#endif
};
}
