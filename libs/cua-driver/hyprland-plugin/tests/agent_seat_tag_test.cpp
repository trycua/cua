#include "agent_seat_tag.hpp"

#include <array>
#include <cstdio>
#include <cstdlib>
#include <fstream>
#include <string>
#include <string_view>
#include <sys/stat.h>
#include <unistd.h>

namespace {

void check(bool condition, const char* what) {
    if (!condition) {
        std::fprintf(stderr, "FAIL: %s\n", what);
        std::exit(1);
    }
}

using namespace std::string_view_literals;
using cua::hyprland::agent_seat_lane_for_pid;
using cua::hyprland::agent_seat_lane_from_environ;

} // namespace

int main() {
    // The environment block is NUL-separated, with a trailing NUL.
    check(agent_seat_lane_from_environ("HOME=/home/u\0CUA_AGENT_SEAT=1\0PATH=/bin\0"sv) == 0u, "lane 1 names lane index 0");
    check(agent_seat_lane_from_environ("CUA_AGENT_SEAT=2\0"sv) == 1u, "lane 2 names lane index 1");
    check(agent_seat_lane_from_environ("CUA_AGENT_SEAT=2"sv) == 1u, "a final entry without a NUL still counts");
    check(!agent_seat_lane_from_environ(""sv), "an empty environment has no tag");
    check(!agent_seat_lane_from_environ("HOME=/home/u\0PATH=/bin\0"sv), "no entry, no tag");

    // Only the exact name and exact values 1 and 2 are a tag.
    check(!agent_seat_lane_from_environ("CUA_AGENT_SEAT=0\0"sv), "lane 0 is not a lane");
    check(!agent_seat_lane_from_environ("CUA_AGENT_SEAT=3\0"sv), "lane 3 is not a lane");
    check(!agent_seat_lane_from_environ("CUA_AGENT_SEAT=\0"sv), "an empty value is not a tag");
    check(!agent_seat_lane_from_environ("CUA_AGENT_SEAT=11\0"sv), "a longer value is not a tag");
    check(!agent_seat_lane_from_environ("CUA_AGENT_SEAT= 1\0"sv), "a padded value is not a tag");
    check(!agent_seat_lane_from_environ("CUA_AGENT_SEAT\0"sv), "a name without a value is not a tag");
    check(!agent_seat_lane_from_environ("XCUA_AGENT_SEAT=1\0"sv), "a longer name is not the tag");
    check(!agent_seat_lane_from_environ("CUA_AGENT_SEATS=1\0"sv), "a name with a suffix is not the tag");
    check(!agent_seat_lane_from_environ("X=CUA_AGENT_SEAT=1\0"sv), "the tag inside another value is not a tag");

    // Like getenv, the first entry wins, and an invalid first entry hides a later valid one.
    check(agent_seat_lane_from_environ("CUA_AGENT_SEAT=1\0CUA_AGENT_SEAT=2\0"sv) == 0u, "first entry wins");
    check(!agent_seat_lane_from_environ("CUA_AGENT_SEAT=9\0CUA_AGENT_SEAT=2\0"sv), "invalid first entry wins");

    // Reading a process's environment through a proc-style tree.
    std::array<char, 64> pattern{};
    const std::string seed = "/tmp/cua-agent-seat-tag-XXXXXX";
    seed.copy(pattern.data(), seed.size());
    const auto* created = mkdtemp(pattern.data());
    check(created != nullptr, "create isolated proc root");
    const std::string root(created);
    const auto process = root + "/4242";
    check(mkdir(process.c_str(), 0700) == 0, "create process directory");
    const auto environ_path = process + "/environ";
    {
        std::ofstream file(environ_path, std::ios::binary);
        file.write("A=b\0CUA_AGENT_SEAT=2\0"sv.data(), static_cast<std::streamsize>("A=b\0CUA_AGENT_SEAT=2\0"sv.size()));
    }
    check(agent_seat_lane_for_pid(4242, root) == 1u, "tag read from the process environment");
    check(!agent_seat_lane_for_pid(4243, root), "a missing process has no tag");
    check(!agent_seat_lane_for_pid(0, root) && !agent_seat_lane_for_pid(-1, root), "pid zero and negative have no tag");

    // An environment larger than the cap is read only up to the cap: a tag after it is not seen.
    {
        std::ofstream file(environ_path, std::ios::binary | std::ios::trunc);
        const std::string filler(cua::hyprland::kMaxEnvironBytes, 'A');
        file << filler << '\0' << "CUA_AGENT_SEAT=1" << '\0';
    }
    check(!agent_seat_lane_for_pid(4242, root), "a tag beyond the read cap is ignored");
    {
        std::ofstream file(environ_path, std::ios::binary | std::ios::trunc);
        const std::string filler(cua::hyprland::kMaxEnvironBytes / 2, 'A');
        file << "CUA_AGENT_SEAT=1" << '\0' << filler << '\0';
    }
    check(agent_seat_lane_for_pid(4242, root) == 0u, "a tag inside the read cap is seen");

    check(unlink(environ_path.c_str()) == 0, "remove environ file");
    check(rmdir(process.c_str()) == 0, "remove process directory");
    check(rmdir(root.c_str()) == 0, "remove proc root");
    std::puts("agent seat tag: ok");
    return 0;
}
