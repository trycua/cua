#pragma once

#include <stdexcept>
#include <string>
#include <sys/stat.h>
#include <unistd.h>

namespace cua::hyprland {

// The per-instance runtime directory is validated by the plugin entry point.
// A successful initialization leaves a marker until the desktop ends. This
// prevents even a different module filename/version from creating replacement
// seats after unload. It is a trusted-local lifecycle guard, not protection
// against another same-user process deleting the marker.
class SeatLifetime {
  public:
    explicit SeatLifetime(const std::string& instance_directory)
        : path_(instance_directory + "/cua-input-seat-lifetime") {
        if (mkdir(path_.c_str(), 0700) != 0)
            throw std::runtime_error("input seat lifetime unavailable; restart the desktop before loading a replacement plugin");
    }
    ~SeatLifetime() {
        // Before publication there are no client-owned seat resources. Only
        // remove our empty reservation after a failed initialization.
        if (!published_)
            static_cast<void>(rmdir(path_.c_str()));
    }
    SeatLifetime(const SeatLifetime&) = delete;
    SeatLifetime& operator=(const SeatLifetime&) = delete;
    void publish() noexcept { published_ = true; }

  private:
    std::string path_;
    bool published_ = false;
};

} // namespace cua::hyprland
