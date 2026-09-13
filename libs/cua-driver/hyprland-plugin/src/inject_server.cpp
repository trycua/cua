#include "inject_server.hpp"

#include "cua_hyprland/protocol.hpp"

#include <array>
#include <cerrno>
#include <chrono>
#include <cstring>
#include <fcntl.h>
#include <poll.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <unistd.h>
#include <vector>

namespace cua::hyprland {
namespace {

constexpr std::size_t kMaxClients = 8;
constexpr std::size_t kMaxAcceptsPerPoll = 16;
constexpr int kPollTimeoutMs = 100;
constexpr auto kAcceptResourceBackoff = std::chrono::milliseconds{250};

struct Client {
    int fd = -1;
    Session session{};
    std::chrono::steady_clock::time_point accepted_at;
    std::chrono::steady_clock::time_point last_activity;
};

void close_client(Client& client) {
    if (client.fd >= 0)
        close(client.fd);
    client.fd = -1;
}

bool peer_is_owner(int fd) {
#ifdef __linux__
    ucred credentials{};
    socklen_t size = sizeof(credentials);
    if (getsockopt(fd, SOL_SOCKET, SO_PEERCRED, &credentials, &size) != 0)
        return false;
    return size == sizeof(credentials) && credentials.uid == getuid();
#else
    uid_t effective_uid = 0;
    gid_t effective_gid = 0;
    return getpeereid(fd, &effective_uid, &effective_gid) == 0 &&
           effective_uid == getuid();
#endif
}

#ifndef __linux__
bool set_nonblocking_cloexec(int fd) {
    const auto descriptor_flags = fcntl(fd, F_GETFD);
    const auto status_flags = fcntl(fd, F_GETFL);
    return descriptor_flags >= 0 && status_flags >= 0 &&
           fcntl(fd, F_SETFD, descriptor_flags | FD_CLOEXEC) == 0 &&
           fcntl(fd, F_SETFL, status_flags | O_NONBLOCK) == 0;
}
#endif

int create_socket() {
#ifdef __linux__
    return socket(AF_UNIX, SOCK_SEQPACKET | SOCK_NONBLOCK | SOCK_CLOEXEC, 0);
#else
    // The plugin only builds on Linux. SOCK_STREAM keeps the transport harness
    // executable on Unix hosts that do not implement AF_UNIX SOCK_SEQPACKET.
    constexpr int socket_type = SOCK_STREAM;
    const auto fd = socket(AF_UNIX, socket_type, 0);
    if (fd < 0)
        return -1;
    if (set_nonblocking_cloexec(fd))
        return fd;
    close(fd);
    return -1;
#endif
}

int accept_client(int listen_fd) {
#ifdef __linux__
    return accept4(listen_fd, nullptr, nullptr, SOCK_NONBLOCK | SOCK_CLOEXEC);
#else
    const auto fd = accept(listen_fd, nullptr, nullptr);
    if (fd < 0)
        return -1;
    if (set_nonblocking_cloexec(fd))
        return fd;
    close(fd);
    errno = EIO;
    return -1;
#endif
}

bool create_wake_pipe(int (&wake_fds)[2]) {
#ifdef __linux__
    return pipe2(wake_fds, O_NONBLOCK | O_CLOEXEC) == 0;
#else
    return pipe(wake_fds) == 0 && set_nonblocking_cloexec(wake_fds[0]) &&
           set_nonblocking_cloexec(wake_fds[1]);
#endif
}

bool send_frame(int fd, const Frame& frame) {
    const auto bytes = encode_frame(frame);
    if (bytes.empty())
        return false;
#ifdef MSG_NOSIGNAL
    constexpr int no_signal = MSG_NOSIGNAL;
#else
    constexpr int no_signal = 0;
#endif
    ssize_t sent = -1;
    do {
        sent = send(fd, bytes.data(), bytes.size(), MSG_DONTWAIT | no_signal);
    } while (sent < 0 && errno == EINTR);
    return sent == static_cast<ssize_t>(bytes.size());
}

std::uint64_t request_id_from_frame_prefix(std::span<const std::byte> bytes) {
    if (bytes.size() < kHeaderSize ||
        !std::equal(kMagic.begin(), kMagic.end(), bytes.begin()))
        return 0;

    std::uint64_t request_id = 0;
    for (std::size_t index = 12; index < 20; ++index)
        request_id =
            (request_id << 8) | std::to_integer<std::uint64_t>(bytes[index]);
    return request_id;
}

ErrorCode protocol_error_for(DecodeError error) {
    return error == DecodeError::unsupported_version ?
               ErrorCode::unsupported_version :
               ErrorCode::malformed_frame;
}

bool is_accept_resource_error(int error) {
    return error == EMFILE || error == ENFILE || error == ENOBUFS ||
           error == ENOMEM;
}

} // namespace

InjectionServer::InjectionServer(std::string socket_path, ServerInfo info,
                                 ServerOptions options)
    : m_socket_path(std::move(socket_path)), m_info(info), m_options(options) {}

InjectionServer::~InjectionServer() {
    stop();
}

bool InjectionServer::start() {
    if (m_running.load())
        return true;
    if (m_thread.joinable() || m_listen_fd >= 0 || m_wake_read_fd >= 0 ||
        m_wake_write_fd >= 0 || m_owns_socket)
        stop();
    set_error({});

    sockaddr_un address{};
    address.sun_family = AF_UNIX;
    if (m_socket_path.size() >= sizeof(address.sun_path)) {
        set_error("socket path exceeds sockaddr_un capacity");
        return false;
    }

    struct stat existing{};
    if (lstat(m_socket_path.c_str(), &existing) == 0) {
        set_error("socket path already exists");
        return false;
    }
    if (errno != ENOENT) {
        set_error(std::string{"cannot inspect socket path: "} + std::strerror(errno));
        return false;
    }

    m_listen_fd = create_socket();
    if (m_listen_fd < 0) {
        set_error(std::string{"cannot create socket: "} + std::strerror(errno));
        return false;
    }
    int wake_fds[2] = {-1, -1};
    if (!create_wake_pipe(wake_fds)) {
        set_error(std::string{"cannot create wake event: "} + std::strerror(errno));
        if (wake_fds[0] >= 0)
            close(wake_fds[0]);
        if (wake_fds[1] >= 0)
            close(wake_fds[1]);
        close(m_listen_fd);
        m_listen_fd = -1;
        return false;
    }
    m_wake_read_fd = wake_fds[0];
    m_wake_write_fd = wake_fds[1];

    std::strncpy(address.sun_path, m_socket_path.c_str(), sizeof(address.sun_path) - 1);
    if (bind(m_listen_fd, reinterpret_cast<sockaddr*>(&address), sizeof(address)) != 0) {
        set_error(std::string{"cannot bind socket: "} + std::strerror(errno));
        close(m_listen_fd);
        m_listen_fd = -1;
        close(m_wake_read_fd);
        close(m_wake_write_fd);
        m_wake_read_fd = -1;
        m_wake_write_fd = -1;
        return false;
    }

    struct stat path_metadata{};
    if (lstat(m_socket_path.c_str(), &path_metadata) != 0 ||
        !S_ISSOCK(path_metadata.st_mode) || path_metadata.st_uid != getuid()) {
        set_error("cannot verify ownership of bound socket path");
        close(m_listen_fd);
        m_listen_fd = -1;
        close(m_wake_read_fd);
        close(m_wake_write_fd);
        m_wake_read_fd = -1;
        m_wake_write_fd = -1;
        return false;
    }
    m_owns_socket = true;
    m_socket_device = path_metadata.st_dev;
    m_socket_inode = path_metadata.st_ino;
    if (chmod(m_socket_path.c_str(), S_IRUSR | S_IWUSR) != 0) {
        set_error(std::string{"cannot restrict socket permissions: "} +
                  std::strerror(errno));
        close(m_listen_fd);
        m_listen_fd = -1;
        close(m_wake_read_fd);
        close(m_wake_write_fd);
        m_wake_read_fd = -1;
        m_wake_write_fd = -1;
        remove_owned_socket();
        return false;
    }
    if (listen(m_listen_fd, static_cast<int>(kMaxClients)) != 0) {
        set_error(std::string{"cannot listen on socket: "} + std::strerror(errno));
        close(m_listen_fd);
        m_listen_fd = -1;
        close(m_wake_read_fd);
        close(m_wake_write_fd);
        m_wake_read_fd = -1;
        m_wake_write_fd = -1;
        remove_owned_socket();
        return false;
    }

    m_stop_requested.store(false);
    m_running.store(true);
    try {
        m_thread = std::thread([this] { run(); });
    } catch (const std::exception& error) {
        m_running.store(false);
        set_error(std::string{"cannot start socket thread: "} + error.what());
        close(m_listen_fd);
        m_listen_fd = -1;
        close(m_wake_read_fd);
        close(m_wake_write_fd);
        m_wake_read_fd = -1;
        m_wake_write_fd = -1;
        remove_owned_socket();
        return false;
    }
    return true;
}

void InjectionServer::stop() {
    m_stop_requested.store(true);
    if (m_wake_write_fd >= 0) {
        const std::byte wake{1};
        const auto written = write(m_wake_write_fd, &wake, sizeof(wake));
        if (written < 0 && errno != EAGAIN && errno != EINTR)
            set_error(std::string{"cannot signal socket shutdown: "} +
                      std::strerror(errno));
    }
    if (m_thread.joinable())
        m_thread.join();
    if (m_listen_fd >= 0) {
        close(m_listen_fd);
        m_listen_fd = -1;
    }
    if (m_wake_read_fd >= 0) {
        close(m_wake_read_fd);
        m_wake_read_fd = -1;
    }
    if (m_wake_write_fd >= 0) {
        close(m_wake_write_fd);
        m_wake_write_fd = -1;
    }
    remove_owned_socket();
    m_running.store(false);
}

ServerSnapshot InjectionServer::snapshot() const {
    std::lock_guard lock{m_error_mutex};
    return ServerSnapshot{
        .running = m_running.load(),
        .accepted_connections = m_accepted_connections.load(),
        .rejected_peers = m_rejected_peers.load(),
        .rejected_busy = m_rejected_busy.load(),
        .timed_out_handshakes = m_timed_out_handshakes.load(),
        .timed_out_idle = m_timed_out_idle.load(),
        .malformed_frames = m_malformed_frames.load(),
        .last_error = m_last_error,
    };
}

const std::string& InjectionServer::socket_path() const {
    return m_socket_path;
}

void InjectionServer::run() {
    std::vector<Client> clients;
    clients.reserve(kMaxClients);
    auto accept_retry_at = std::chrono::steady_clock::time_point::min();
    bool transport_failed = false;

    while (!m_stop_requested.load()) {
        const auto now = std::chrono::steady_clock::now();
        for (std::size_t index = clients.size(); index > 0; --index) {
            auto& client = clients[index - 1];
            const auto handshake_expired =
                !client.session.handshaken() &&
                now - client.accepted_at >= m_options.handshake_timeout;
            const auto idle_expired =
                client.session.handshaken() &&
                now - client.last_activity >= m_options.idle_timeout;
            if (!handshake_expired && !idle_expired)
                continue;
            close_client(client);
            clients.erase(clients.begin() +
                          static_cast<std::ptrdiff_t>(index - 1));
            if (handshake_expired)
                ++m_timed_out_handshakes;
            else
                ++m_timed_out_idle;
        }
        const auto accept_ready = now >= accept_retry_at;
        std::vector<pollfd> poll_fds;
        poll_fds.reserve(2 + clients.size());
        poll_fds.push_back(pollfd{
            .fd = accept_ready ? m_listen_fd : -1,
            .events = POLLIN,
            .revents = 0,
        });
        poll_fds.push_back(pollfd{.fd = m_wake_read_fd, .events = POLLIN, .revents = 0});
        for (const auto& client : clients)
            poll_fds.push_back(pollfd{.fd = client.fd, .events = POLLIN, .revents = 0});

        const auto ready = poll(poll_fds.data(), poll_fds.size(), kPollTimeoutMs);
        if (ready < 0) {
            if (errno == EINTR)
                continue;
            set_error(std::string{"socket poll failed: "} + std::strerror(errno));
            transport_failed = true;
            break;
        }
        if ((poll_fds[1].revents & POLLIN) != 0)
            break;

        if (!poll_fds.empty() && (poll_fds[0].revents & POLLIN) != 0) {
            for (std::size_t accepted = 0; accepted < kMaxAcceptsPerPoll;
                 ++accepted) {
                const auto fd = accept_client(m_listen_fd);
                if (fd < 0) {
                    const auto accept_error = errno;
                    if (accept_error == EINTR)
                        continue;
                    if (accept_error == EAGAIN || accept_error == EWOULDBLOCK)
                        break;
                    set_error(std::string{"socket accept failed: "} +
                              std::strerror(accept_error));
                    if (is_accept_resource_error(accept_error)) {
                        accept_retry_at = std::chrono::steady_clock::now() +
                                          kAcceptResourceBackoff;
                    } else {
                        transport_failed = true;
                    }
                    break;
                }
                if (!peer_is_owner(fd)) {
                    ++m_rejected_peers;
                    close(fd);
                    continue;
                }
                if (clients.size() >= kMaxClients) {
                    ++m_rejected_busy;
                    send_frame(fd, make_error(0, ErrorCode::server_busy,
                                              "concurrent client limit reached"));
                    close(fd);
                    continue;
                }
                clients.push_back(Client{
                    .fd = fd,
                    .accepted_at = std::chrono::steady_clock::now(),
                    .last_activity = std::chrono::steady_clock::now(),
                });
                ++m_accepted_connections;
            }
        }

        for (std::size_t index = clients.size(); index > 0; --index) {
            auto& client = clients[index - 1];
            const auto poll_index = index + 1;
            const auto revents = poll_fds.size() > poll_index ?
                                     poll_fds[poll_index].revents :
                                     0;
            const auto terminal_event =
                (revents & (POLLERR | POLLHUP | POLLNVAL)) != 0;
            if ((revents & POLLIN) == 0 && terminal_event) {
                close_client(client);
                clients.erase(clients.begin() + static_cast<std::ptrdiff_t>(index - 1));
                continue;
            }
            if ((revents & POLLIN) == 0)
                continue;

            std::array<std::byte, kMaxFrameSize + 1> buffer{};
            const auto received = recv(client.fd, buffer.data(), buffer.size(),
                                       MSG_DONTWAIT | MSG_TRUNC);
            if (received < 0 && errno == EINTR)
                continue;
            if (received < 0 &&
                (errno == EAGAIN || errno == EWOULDBLOCK)) {
                if (terminal_event) {
                    close_client(client);
                    clients.erase(clients.begin() +
                                  static_cast<std::ptrdiff_t>(index - 1));
                }
                continue;
            }
            if (received <= 0 || static_cast<std::size_t>(received) > kMaxFrameSize) {
                if (received > 0)
                    ++m_malformed_frames;
                close_client(client);
                clients.erase(clients.begin() + static_cast<std::ptrdiff_t>(index - 1));
                continue;
            }

            const auto decoded = decode_frame(
                std::span<const std::byte>{buffer.data(), static_cast<std::size_t>(received)});
            if (!std::holds_alternative<Frame>(decoded)) {
                ++m_malformed_frames;
                const auto error = std::get<DecodeError>(decoded);
                const auto response = make_error(
                    request_id_from_frame_prefix(std::span<const std::byte>{
                        buffer.data(), static_cast<std::size_t>(received)}),
                    protocol_error_for(error),
                    "invalid cua-inject-v2 frame");
                send_frame(client.fd, response);
                close_client(client);
                clients.erase(clients.begin() + static_cast<std::ptrdiff_t>(index - 1));
                continue;
            }

            client.last_activity = std::chrono::steady_clock::now();
            const auto response = client.session.handle(std::get<Frame>(decoded), m_info);
            if (!send_frame(client.fd, response)) {
                close_client(client);
                clients.erase(clients.begin() + static_cast<std::ptrdiff_t>(index - 1));
            }
        }

        if (transport_failed)
            break;
    }

    m_running.store(false);
    for (auto& client : clients)
        close_client(client);
    if (m_listen_fd >= 0) {
        close(m_listen_fd);
        m_listen_fd = -1;
    }
    remove_owned_socket();
    if (transport_failed && !m_stop_requested.load() && snapshot().last_error.empty())
        set_error("transport thread exited unexpectedly");
}

void InjectionServer::remove_owned_socket() {
    if (!m_owns_socket)
        return;

    struct stat current{};
    if (lstat(m_socket_path.c_str(), &current) == 0) {
        if (S_ISSOCK(current.st_mode) && current.st_dev == m_socket_device &&
            current.st_ino == m_socket_inode) {
            if (unlink(m_socket_path.c_str()) != 0 && errno != ENOENT)
                set_error(std::string{"cannot remove owned socket: "} +
                          std::strerror(errno));
        } else {
            set_error("socket path identity changed; refusing to unlink it");
        }
    } else if (errno != ENOENT) {
        set_error(std::string{"cannot inspect owned socket during cleanup: "} +
                  std::strerror(errno));
    }

    m_owns_socket = false;
    m_socket_device = 0;
    m_socket_inode = 0;
}

void InjectionServer::set_error(std::string error) {
    std::lock_guard lock{m_error_mutex};
    m_last_error = std::move(error);
}

} // namespace cua::hyprland
