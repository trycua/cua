#pragma once

#include <string_view>

namespace cua::hyprland {

constexpr bool is_cua_driver_process(std::string_view process_name) noexcept {
    return process_name == "cua-driver";
}

constexpr bool agent_global_is_visible(bool compositor_allows,
                                       bool agent_seat,
                                       bool agent_process) noexcept {
    return compositor_allows && (!agent_seat || agent_process);
}

} // namespace cua::hyprland
