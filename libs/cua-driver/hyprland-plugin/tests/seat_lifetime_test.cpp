#include "seat_lifetime.hpp"

#include <array>
#include <cstdlib>
#include <iostream>

namespace {
void check(bool condition, const char* message) {
    if (!condition) {
        std::cerr << "FAIL: " << message << '\n';
        std::exit(1);
    }
}
bool refused(const std::string& directory) {
    try {
        cua::hyprland::SeatLifetime second(directory);
        return false;
    } catch (const std::runtime_error&) {
        return true;
    }
}
}

int main() {
    std::array<char, 64> pattern{};
    const std::string seed = "/tmp/cua-seat-lifetime-XXXXXX";
    seed.copy(pattern.data(), seed.size());
    const auto* created = mkdtemp(pattern.data());
    check(created != nullptr, "create isolated instance directory");
    const std::string directory(created);
    const auto marker = directory + "/cua-input-seat-lifetime";
    {
        cua::hyprland::SeatLifetime failed_initialization(directory);
        check(refused(directory), "parallel initialization refuses");
    }
    check(access(marker.c_str(), F_OK) != 0, "failed initialization releases reservation");
    {
        cua::hyprland::SeatLifetime initialized(directory);
        initialized.publish();
    }
    check(refused(directory), "replacement after publication requires desktop restart");
    struct stat metadata{};
    check(lstat(marker.c_str(), &metadata) == 0 && S_ISDIR(metadata.st_mode) &&
              (metadata.st_mode & 0777) == 0700,
          "lifetime marker stays private");
    // This is the test-owned desktop teardown, never plugin hot unload.
    check(rmdir(marker.c_str()) == 0, "remove test instance marker");
    check(rmdir(directory.c_str()) == 0, "remove test instance directory");
    std::cout << "seat lifetime tests passed\n";
}
