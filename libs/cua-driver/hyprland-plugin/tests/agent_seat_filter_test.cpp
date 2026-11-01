#include "agent_seat_filter.hpp"

#include <cstdlib>
#include <iostream>

namespace {
void check(bool value, const char* message) {
    if (!value) {
        std::cerr << "FAIL: " << message << '\n';
        std::exit(1);
    }
}
}

int main() {
    using cua::hyprland::agent_global_is_visible;
    using cua::hyprland::is_cua_driver_process;

    check(!is_cua_driver_process("imv"), "ordinary client is not the Cua Driver");
    check(!is_cua_driver_process("zeditor"), "Zed is not the Cua Driver");
    check(!is_cua_driver_process("cua-driver-helper"), "process match is exact");
    check(is_cua_driver_process("cua-driver"), "Cua Driver is recognized");

    check(!agent_global_is_visible(true, true, false),
          "ordinary clients cannot discover agent seats");
    check(agent_global_is_visible(true, true, true),
          "Cua Driver retains access to agent seats");
    check(agent_global_is_visible(true, false, false),
          "ordinary globals remain visible to ordinary clients");
    check(!agent_global_is_visible(false, false, true),
          "Hyprland global restrictions remain authoritative");
}
