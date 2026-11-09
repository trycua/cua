#pragma once

#include <cstdint>
#include <string>
#include <string_view>

namespace cua::hyprland {

struct StatusReport {
    std::string_view plugin_version{};
    std::uint64_t compositor_epoch = 0;
    std::string_view compiled_hash{};
    std::string_view runtime_hash{};
    bool configured = false;
    bool transport_ready = false;
    std::string_view socket_path{};
    std::string_view last_error{};
    bool discovery_enabled = false;
    std::uint64_t accepted_connections = 0;
    std::uint64_t rejected_peers = 0;
    std::uint64_t rejected_busy = 0;
    std::uint64_t timed_out_handshakes = 0;
    std::uint64_t timed_out_idle = 0;
    std::uint64_t malformed_frames = 0;
};

bool is_valid_utf8(std::string_view value);
std::string render_status_json(const StatusReport& report);
std::string render_status_text(const StatusReport& report);

} // namespace cua::hyprland
