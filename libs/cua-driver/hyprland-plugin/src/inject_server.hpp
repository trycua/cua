#pragma once

#include "cua_hyprland/session.hpp"

#include <atomic>
#include <chrono>
#include <cstdint>
#include <mutex>
#include <string>
#include <sys/types.h>
#include <thread>

namespace cua::hyprland {

struct ServerSnapshot {
    bool running = false;
    std::uint64_t accepted_connections = 0;
    std::uint64_t rejected_peers = 0;
    std::uint64_t rejected_busy = 0;
    std::uint64_t timed_out_handshakes = 0;
    std::uint64_t timed_out_idle = 0;
    std::uint64_t malformed_frames = 0;
    std::string last_error;
};

struct ServerOptions {
    std::chrono::milliseconds handshake_timeout{5000};
    std::chrono::milliseconds idle_timeout{60000};
};

class InjectionServer {
  public:
    InjectionServer(std::string socket_path, ServerInfo info,
                    ServerOptions options = {});
    ~InjectionServer();

    InjectionServer(const InjectionServer&) = delete;
    InjectionServer& operator=(const InjectionServer&) = delete;

    bool start();
    void stop();
    ServerSnapshot snapshot() const;
    const std::string& socket_path() const;

  private:
    void run();
    void remove_owned_socket();
    void set_error(std::string error);

    std::string m_socket_path;
    ServerInfo m_info;
    ServerOptions m_options;
    int m_listen_fd = -1;
    int m_wake_read_fd = -1;
    int m_wake_write_fd = -1;
    bool m_owns_socket = false;
    dev_t m_socket_device = 0;
    ino_t m_socket_inode = 0;
    std::atomic<bool> m_stop_requested = false;
    std::atomic<bool> m_running = false;
    std::atomic<std::uint64_t> m_accepted_connections = 0;
    std::atomic<std::uint64_t> m_rejected_peers = 0;
    std::atomic<std::uint64_t> m_rejected_busy = 0;
    std::atomic<std::uint64_t> m_timed_out_handshakes = 0;
    std::atomic<std::uint64_t> m_timed_out_idle = 0;
    std::atomic<std::uint64_t> m_malformed_frames = 0;
    mutable std::mutex m_error_mutex;
    std::string m_last_error;
    std::thread m_thread;
};

} // namespace cua::hyprland
