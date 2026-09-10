#pragma once

#include "cua_hyprland/protocol.hpp"

#include <cstdint>

namespace cua::hyprland {

struct ServerInfo {
    std::uint64_t compositor_epoch = 0;
    std::uint64_t supported_capabilities = capability(Capability::discovery);
    std::uint64_t enabled_capabilities = capability(Capability::discovery);
};

class Session {
  public:
    Frame handle(const Frame& request, const ServerInfo& server);
    bool handshaken() const;

  private:
    bool m_handshaken = false;
    std::uint64_t m_negotiated_capabilities = 0;
};

} // namespace cua::hyprland
