#pragma once

#include <algorithm>
#include <cstddef>
#include <cstdio>
#include <optional>
#include <string>
#include <string_view>

namespace cua::hyprland {

// A client the driver launched for the agent carries CUA_AGENT_SEAT=<lane> in
// its environment, lane 1 or 2. When such a client binds a wl_seat, the plugin
// gives it that lane's agent seat instead of the user's seat, so a toolkit
// that binds only its first wl_seat (Chromium's Ozone Wayland backend, and so
// Electron) lands on the agent seat.
//
// The environment is inherited by wrapper scripts and helper processes, so the
// connecting process carries the tag however the app was started. The tag is a
// same-user compatibility hint, not a security boundary: a process that sets it
// on itself only gives up the user's input to itself.
inline constexpr std::string_view kAgentSeatEnv = "CUA_AGENT_SEAT";
inline constexpr unsigned kAgentSeatLanes = 2;
// A larger environment block is read only up to this size.
inline constexpr std::size_t kMaxEnvironBytes = 512 * 1024;

// The lane index (0 or 1) named by the first CUA_AGENT_SEAT entry of a
// NUL-separated environment block. Any value other than exactly "1" or "2"
// names no lane: a malformed tag never binds an agent seat.
inline std::optional<unsigned> agent_seat_lane_from_environ(std::string_view block) {
    std::size_t start = 0;
    while (start < block.size()) {
        auto end = block.find('\0', start);
        if (end == std::string_view::npos) end = block.size();
        const auto entry = block.substr(start, end - start);
        if (entry.size() > kAgentSeatEnv.size() && entry.starts_with(kAgentSeatEnv) &&
            entry[kAgentSeatEnv.size()] == '=') {
            const auto value = entry.substr(kAgentSeatEnv.size() + 1);
            if (value == "1") return 0u;
            if (value == "2") return 1u;
            return std::nullopt;
        }
        start = end + 1;
    }
    return std::nullopt;
}

// The lane tag of process `pid`, read from <proc_root>/<pid>/environ. A process
// that has gone or whose environment is unreadable has no tag.
inline std::optional<unsigned> agent_seat_lane_for_pid(long pid, const std::string& proc_root = "/proc") {
    if (pid <= 0) return std::nullopt;
    const auto path = proc_root + "/" + std::to_string(pid) + "/environ";
    std::FILE* file = std::fopen(path.c_str(), "rb");
    if (!file) return std::nullopt;
    std::string block;
    char chunk[4096];
    while (block.size() < kMaxEnvironBytes) {
        const auto want = std::min(sizeof chunk, kMaxEnvironBytes - block.size());
        const auto got = std::fread(chunk, 1, want, file);
        if (got == 0) break;
        block.append(chunk, got);
    }
    std::fclose(file);
    return agent_seat_lane_from_environ(block);
}

} // namespace cua::hyprland
