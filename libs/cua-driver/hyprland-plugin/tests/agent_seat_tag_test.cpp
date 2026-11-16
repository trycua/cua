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

using cua::hyprland::agent_seat_lane_for_pid;
using cua::hyprland::agent_seat_lane_from_cgroup;
using cua::hyprland::agent_seat_lane_from_environ;
using cua::hyprland::agent_seat_lane_from_unit;
using namespace std::string_view_literals;

} // namespace

int main() {
    check(agent_seat_lane_from_unit("cua-agent-seat-1-abc123.scope") == 0u, "lane 1 names lane index 0");
    check(agent_seat_lane_from_unit("cua-agent-seat-2-ABC.scope") == 1u, "lane 2 names lane index 1");
    check(agent_seat_lane_from_unit("cua-agent-seat-1-0.scope") == 0u, "a one character token is enough");
    check(!agent_seat_lane_from_unit("cua-agent-seat-0-abc.scope"), "lane 0 is not a lane");
    check(!agent_seat_lane_from_unit("cua-agent-seat-3-abc.scope"), "lane 3 is not a lane");
    check(!agent_seat_lane_from_unit("cua-agent-seat-12-abc.scope"), "lane 12 is not a lane");
    check(!agent_seat_lane_from_unit("cua-agent-seat-1-.scope"), "an empty token is not a tag");
    check(!agent_seat_lane_from_unit("cua-agent-seat-1.scope"), "no token is not a tag");
    check(!agent_seat_lane_from_unit("cua-agent-seat-1-a_b.scope"), "a token with punctuation is not a tag");
    check(!agent_seat_lane_from_unit("cua-agent-seat-1-a b.scope"), "a token with a space is not a tag");
    check(agent_seat_lane_from_unit("cua-agent-seat-1-" + std::string(32, 'a') + ".scope") == 0u, "32 characters fit");
    check(!agent_seat_lane_from_unit(("cua-agent-seat-1-" + std::string(33, 'a') + ".scope").c_str()), "33 characters do not");
    check(!agent_seat_lane_from_unit("cua-agent-seat-1-abc.service"), "only a scope is a tag");
    check(!agent_seat_lane_from_unit("xcua-agent-seat-1-abc.scope"), "a longer name is not the tag");
    check(!agent_seat_lane_from_unit(""), "an empty unit is not a tag");

    const std::string agent = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/cua-agent-seat-2-9f3c.scope\n";
    check(agent_seat_lane_from_cgroup(agent) == 1u, "a scope in a cgroup v2 path names its lane");
    check(agent_seat_lane_from_cgroup("12:cpu:/x/cua-agent-seat-1-ab.scope\n0::/user.slice\n") == 0u, "a v1 style line works");
    check(!agent_seat_lane_from_cgroup("0::/user.slice/user-1000.slice/session-3.scope\n"), "an ordinary scope is no tag");
    check(!agent_seat_lane_from_cgroup("0::/user.slice/app-chromium-1234.scope\n"), "an app scope is no tag");
    check(!agent_seat_lane_from_cgroup(""), "an empty listing is no tag");
    check(!agent_seat_lane_from_cgroup("garbage without colons cua-agent-seat-1-ab.scope\n"), "a line without a path is no tag");
    check(!agent_seat_lane_from_cgroup("0::/a/cua-agent-seat-1-ab.scope.d/b\n"), "a component that only contains the name is no tag");
    check(agent_seat_lane_from_cgroup("0::/a/cua-agent-seat-1-ab.scope") == 0u, "no trailing newline needed");

    // The environment fallback: exact names and values only, first entry wins.
    check(agent_seat_lane_from_environ("HOME=/h\0CUA_AGENT_SEAT=1\0PATH=/bin\0"sv) == 0u, "env lane 1");
    check(agent_seat_lane_from_environ("CUA_AGENT_SEAT=2"sv) == 1u, "env lane 2 without a final NUL");
    for (const auto bad : {""sv, "CUA_AGENT_SEAT=0\0"sv, "CUA_AGENT_SEAT=3\0"sv, "CUA_AGENT_SEAT=\0"sv, "CUA_AGENT_SEAT=11\0"sv,
                           "CUA_AGENT_SEAT= 1\0"sv, "CUA_AGENT_SEAT\0"sv, "XCUA_AGENT_SEAT=1\0"sv, "CUA_AGENT_SEATS=1\0"sv,
                           "X=CUA_AGENT_SEAT=1\0"sv})
        check(!agent_seat_lane_from_environ(bad), "env: not a tag");
    check(agent_seat_lane_from_environ("CUA_AGENT_SEAT=1\0CUA_AGENT_SEAT=2\0"sv) == 0u, "env: first entry wins");
    check(!agent_seat_lane_from_environ("CUA_AGENT_SEAT=9\0CUA_AGENT_SEAT=2\0"sv), "env: invalid first entry wins");

    std::array<char, 64> pattern{};
    const std::string seed = "/tmp/cua-agent-seat-tag-XXXXXX";
    seed.copy(pattern.data(), seed.size());
    const auto* created = mkdtemp(pattern.data());
    check(created != nullptr, "create isolated proc root");
    const std::string root(created);
    const auto process = root + "/4242";
    check(mkdir(process.c_str(), 0700) == 0, "create process directory");
    const auto cgroup_path = process + "/cgroup";
    {
        std::ofstream file(cgroup_path);
        file << agent;
    }
    check(agent_seat_lane_for_pid(4242, root) == 1u, "tag read from the process cgroup");
    check(!agent_seat_lane_for_pid(4243, root), "a missing process has no tag");
    check(!agent_seat_lane_for_pid(0, root) && !agent_seat_lane_for_pid(-1, root), "pid zero and negative have no tag");
    {
        std::ofstream file(cgroup_path, std::ios::trunc);
        file << std::string(cua::hyprland::kMaxCgroupBytes, 'A') << "\n0::/cua-agent-seat-1-ab.scope\n";
    }
    check(!agent_seat_lane_for_pid(4242, root), "a tag beyond the read cap is ignored");

    // No scope in the cgroup: the environment is read instead; a scope wins over it.
    {
        std::ofstream file(cgroup_path, std::ios::trunc);
        file << "0::/user.slice/session-3.scope\n";
        std::ofstream env(process + "/environ", std::ios::binary);
        env.write("A=b\0CUA_AGENT_SEAT=2\0"sv.data(), static_cast<std::streamsize>("A=b\0CUA_AGENT_SEAT=2\0"sv.size()));
    }
    check(agent_seat_lane_for_pid(4242, root) == 1u, "env fallback when the cgroup has no scope");
    {
        std::ofstream file(cgroup_path, std::ios::trunc);
        file << "0::/x/cua-agent-seat-1-ab.scope\n";
    }
    check(agent_seat_lane_for_pid(4242, root) == 0u, "the scope wins over the environment");
    check(unlink((process + "/environ").c_str()) == 0, "remove environ file");
    check(unlink(cgroup_path.c_str()) == 0, "remove cgroup file");
    check(rmdir(process.c_str()) == 0, "remove process directory");
    check(rmdir(root.c_str()) == 0, "remove proc root");
    std::puts("agent seat tag: ok");
    return 0;
}
