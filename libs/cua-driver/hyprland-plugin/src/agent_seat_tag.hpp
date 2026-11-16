#pragma once

#include <algorithm>
#include <cstddef>
#include <cstdio>
#include <optional>
#include <string>
#include <string_view>

namespace cua::hyprland {

// A client the driver launched for the agent runs inside a transient systemd
// user scope named cua-agent-seat-<lane>-<token>.scope, lane 1 or 2. When a
// client in such a scope binds a wl_seat, the plugin gives it that lane's agent
// seat instead of the user's seat, so a toolkit that binds only its first
// wl_seat (Chromium's Ozone Wayland backend, and so Electron) lands on the
// agent seat.
//
// The tag is the process's cgroup, read from /proc/<pid>/cgroup. A cgroup is
// set before the program starts, is inherited by every helper process, and
// cannot be lost the way an environment variable can: Chromium and Electron
// overwrite their own environment block at startup (setproctitle), so
// /proc/<pid>/environ is empty by the time they connect to Wayland.
//
// Where there is no systemd user manager the driver tags the app with
// CUA_AGENT_SEAT=<1|2> in its environment instead. That works for apps that keep
// their environment (GTK, Qt, and the test client in requalify/), not for
// Chromium and Electron, so it is only the fallback and the scope wins.
//
// The tag is a same-user compatibility hint, not a security boundary: a process
// that moves itself into such a scope only gives up the user's input to itself.
inline constexpr std::string_view kAgentSeatScopePrefix = "cua-agent-seat-";
inline constexpr std::string_view kAgentSeatScopeSuffix = ".scope";
inline constexpr std::string_view kAgentSeatEnv = "CUA_AGENT_SEAT";
inline constexpr unsigned kAgentSeatLanes = 2;
// A larger cgroup file is read only up to this size.
inline constexpr std::size_t kMaxCgroupBytes = 64 * 1024;
inline constexpr std::size_t kMaxEnvironBytes = 512 * 1024;

// The lane index (0 or 1) named by one cgroup path component, or none. Only
// cua-agent-seat-<1|2>-<token>.scope with a 1 to 32 character alphanumeric
// token names a lane.
inline std::optional<unsigned> agent_seat_lane_from_unit(std::string_view unit) {
    if (!unit.starts_with(kAgentSeatScopePrefix) || !unit.ends_with(kAgentSeatScopeSuffix)) return std::nullopt;
    unit.remove_prefix(kAgentSeatScopePrefix.size());
    unit.remove_suffix(kAgentSeatScopeSuffix.size());
    if (unit.size() < 3 || unit[1] != '-' || (unit[0] != '1' && unit[0] != '2')) return std::nullopt;
    const auto token = unit.substr(2);
    if (token.size() > 32) return std::nullopt;
    for (const char c : token)
        if (!((c >= '0' && c <= '9') || (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z'))) return std::nullopt;
    return unit[0] == '1' ? 0u : 1u;
}

// The lane named by a /proc/<pid>/cgroup listing: the first line whose path has
// a lane scope component. Lines look like "0::/user.slice/.../app.slice/x.scope".
inline std::optional<unsigned> agent_seat_lane_from_cgroup(std::string_view listing) {
    std::size_t start = 0;
    while (start < listing.size()) {
        auto end = listing.find('\n', start);
        if (end == std::string_view::npos) end = listing.size();
        auto line = listing.substr(start, end - start);
        start = end + 1;
        const auto first = line.find(':');
        const auto second = first == std::string_view::npos ? first : line.find(':', first + 1);
        if (second == std::string_view::npos) continue;
        auto path = line.substr(second + 1);
        std::size_t at = 0;
        while (at <= path.size()) {
            auto slash = path.find('/', at);
            if (slash == std::string_view::npos) slash = path.size();
            if (const auto lane = agent_seat_lane_from_unit(path.substr(at, slash - at))) return lane;
            at = slash + 1;
        }
    }
    return std::nullopt;
}

// The lane (0 or 1) named by the first CUA_AGENT_SEAT entry of a NUL-separated
// environment block. Only exactly "1" or "2" names a lane; the first entry wins,
// as in getenv.
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

// At most `cap` bytes of a file, empty when it cannot be opened.
inline std::string read_capped(const std::string& path, std::size_t cap) {
    std::FILE* file = std::fopen(path.c_str(), "rb");
    if (!file) return {};
    std::string text;
    char chunk[4096];
    while (text.size() < cap) {
        const auto got = std::fread(chunk, 1, std::min(sizeof chunk, cap - text.size()), file);
        if (got == 0) break;
        text.append(chunk, got);
    }
    std::fclose(file);
    return text;
}

// The lane tag of process `pid`: its cgroup scope, else its environment, read
// from <proc_root>/<pid>/. A process that has gone has no tag.
inline std::optional<unsigned> agent_seat_lane_for_pid(long pid, const std::string& proc_root = "/proc") {
    if (pid <= 0) return std::nullopt;
    const auto base = proc_root + "/" + std::to_string(pid);
    if (const auto lane = agent_seat_lane_from_cgroup(read_capped(base + "/cgroup", kMaxCgroupBytes))) return lane;
    return agent_seat_lane_from_environ(read_capped(base + "/environ", kMaxEnvironBytes));
}

} // namespace cua::hyprland
