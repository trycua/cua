#include "cua_hyprland/protocol.hpp"
#include "cua_hyprland/session.hpp"
#include "inject_server.hpp"

#include <array>
#include <algorithm>
#include <cstdlib>
#include <chrono>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <span>
#include <string>
#include <thread>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <unistd.h>
#include <variant>

namespace {

void check(bool condition, const std::string& message) {
    if (condition)
        return;
    std::cerr << "FAIL: " << message << '\n';
    std::exit(1);
}

class TempDirectory {
  public:
    TempDirectory() {
        std::array<char, 64> pattern{};
        const auto seed = std::string{"/tmp/cua-hyprland-transport-XXXXXX"};
        std::copy(seed.begin(), seed.end(), pattern.begin());
        const auto* path = mkdtemp(pattern.data());
        check(path != nullptr, "temporary directory created");
        m_path = path;
    }

    ~TempDirectory() {
        static_cast<void>(rmdir(m_path.c_str()));
    }

    std::filesystem::path path() const { return m_path; }

  private:
    std::filesystem::path m_path;
};

int connect_client(const std::string& socket_path) {
#ifdef __linux__
    constexpr int socket_type = SOCK_SEQPACKET;
#else
    constexpr int socket_type = SOCK_STREAM;
#endif
    const auto fd = socket(AF_UNIX, socket_type, 0);
    check(fd >= 0, "client socket created");

    sockaddr_un address{};
    address.sun_family = AF_UNIX;
    check(socket_path.size() < sizeof(address.sun_path), "test socket path fits");
    std::copy(socket_path.begin(), socket_path.end(), address.sun_path);
    check(connect(fd, reinterpret_cast<sockaddr*>(&address), sizeof(address)) == 0,
          "client connected");
    return fd;
}

void send_request(int fd, const cua::hyprland::Frame& frame) {
    const auto bytes = cua::hyprland::encode_frame(frame);
#ifdef MSG_NOSIGNAL
    constexpr int no_signal = MSG_NOSIGNAL;
#else
    constexpr int no_signal = 0;
#endif
    check(send(fd, bytes.data(), bytes.size(), no_signal) ==
              static_cast<ssize_t>(bytes.size()),
          "request sent");
}

void send_bytes(int fd, std::span<const std::byte> bytes) {
#ifdef MSG_NOSIGNAL
    constexpr int no_signal = MSG_NOSIGNAL;
#else
    constexpr int no_signal = 0;
#endif
    check(send(fd, bytes.data(), bytes.size(), no_signal) ==
              static_cast<ssize_t>(bytes.size()),
          "raw packet sent");
}

cua::hyprland::Frame receive_response(int fd) {
    std::array<std::byte, cua::hyprland::kMaxFrameSize> bytes{};
    const auto received = recv(fd, bytes.data(), bytes.size(), 0);
    check(received > 0, "response received");
    const auto decoded = cua::hyprland::decode_frame(
        std::span<const std::byte>{bytes.data(), static_cast<std::size_t>(received)});
    check(std::holds_alternative<cua::hyprland::Frame>(decoded),
          "response frame decoded");
    return std::get<cua::hyprland::Frame>(decoded);
}

} // namespace

int main() {
    using namespace cua::hyprland;

    TempDirectory temporary;
    const auto socket_path = (temporary.path() / "cua-inject-v2.sock").string();
    InjectionServer server{
        socket_path,
        ServerInfo{
            .compositor_epoch = 77,
            .supported_capabilities = capability(Capability::discovery),
            .enabled_capabilities = capability(Capability::discovery),
        }};
    const auto started = server.start();
    check(started, "server started: " + server.snapshot().last_error);

    struct stat metadata{};
    check(lstat(socket_path.c_str(), &metadata) == 0 && S_ISSOCK(metadata.st_mode),
          "server created a Unix socket");
    check((metadata.st_mode & 0777) == 0600, "socket mode is 0600");

    const auto bad_magic_client = connect_client(socket_path);
    auto bad_magic_bytes = encode_frame(make_frame(
        MessageType::hello, 8, encode_hello(Hello{})));
    bad_magic_bytes[0] = std::byte{'X'};
    send_bytes(bad_magic_client, bad_magic_bytes);
    const auto bad_magic_response = receive_response(bad_magic_client);
    const auto bad_magic_error = decode_error(bad_magic_response.payload);
    check(bad_magic_response.header.request_id == 0 &&
              std::holds_alternative<ProtocolError>(bad_magic_error) &&
              std::get<ProtocolError>(bad_magic_error).code ==
                  ErrorCode::malformed_frame,
          "wire-level bad magic returns an uncorrelated malformed-frame error");
    close(bad_magic_client);

    const auto future_client = connect_client(socket_path);
    auto future_bytes = encode_frame(make_frame(
        MessageType::hello, 9, encode_hello(Hello{})));
    future_bytes[4] = std::byte{0};
    future_bytes[5] = std::byte{3};
    send_bytes(future_client, future_bytes);
    const auto future_response = receive_response(future_client);
    const auto future_error = decode_error(future_response.payload);
    check(future_response.header.request_id == 9 &&
              std::holds_alternative<ProtocolError>(future_error) &&
              std::get<ProtocolError>(future_error).code ==
                  ErrorCode::unsupported_version,
          "wire-level future major returns a correlated version error");
    close(future_client);

#ifdef __linux__
    const auto oversized_client = connect_client(socket_path);
    const std::vector<std::byte> oversized_packet(kMaxFrameSize + 1,
                                                   std::byte{0x5a});
    send_bytes(oversized_client, oversized_packet);
    close(oversized_client);

    const auto zero_length_client = connect_client(socket_path);
    send_bytes(zero_length_client, {});
    close(zero_length_client);
#endif

    const auto client = connect_client(socket_path);
    send_request(
        client,
        make_frame(
            MessageType::hello, 10,
            encode_hello(Hello{
                .requested_capabilities = capability(Capability::discovery),
                .required_capabilities = capability(Capability::discovery),
            })));
    const auto welcome_frame = receive_response(client);
    check(welcome_frame.header.type == MessageType::welcome,
          "same-UID client negotiated a session");
    const auto welcome = decode_welcome(welcome_frame.payload);
    check(std::holds_alternative<Welcome>(welcome) &&
              std::get<Welcome>(welcome).compositor_epoch == 77,
          "server returned the current compositor epoch");

    send_request(client, make_frame(MessageType::pointer_button, 11));
    const auto refusal_frame = receive_response(client);
    const auto refusal = decode_error(refusal_frame.payload);
    check(std::holds_alternative<ProtocolError>(refusal) &&
              std::get<ProtocolError>(refusal).code ==
                  ErrorCode::background_unavailable,
          "mutation returned typed background_unavailable");
    close(client);

#ifdef __linux__
    for (int attempt = 0;
         attempt < 100 && server.snapshot().malformed_frames < 3;
         ++attempt)
        std::this_thread::sleep_for(std::chrono::milliseconds(5));
    check(server.snapshot().malformed_frames == 3,
          "wire-level malformed, unsupported, and oversized packets counted");
#else
    check(server.snapshot().malformed_frames == 2,
          "wire-level malformed and unsupported packets counted");
#endif

    const auto long_lived_client = connect_client(socket_path);
    send_request(
        long_lived_client,
        make_frame(
            MessageType::hello, 13,
            encode_hello(Hello{
                .requested_capabilities = capability(Capability::discovery),
                .required_capabilities = capability(Capability::discovery),
            })));
    check(receive_response(long_lived_client).header.type == MessageType::welcome,
          "long-lived client negotiated before transport restart");
    server.stop();
    check(!std::filesystem::exists(socket_path), "owned socket removed on stop");
    std::array<std::byte, 1> closed_probe{};
    check(recv(long_lived_client, closed_probe.data(), closed_probe.size(), 0) == 0,
          "long-lived client observed transport shutdown");
    close(long_lived_client);

    InjectionServer restarted_server{
        socket_path,
        ServerInfo{
            .compositor_epoch = 78,
            .supported_capabilities = capability(Capability::discovery),
            .enabled_capabilities = capability(Capability::discovery),
        }};
    check(restarted_server.start(),
          "restarted server started: " + restarted_server.snapshot().last_error);
    const auto reconnected_client = connect_client(socket_path);
    send_request(
        reconnected_client,
        make_frame(
            MessageType::hello, 14,
            encode_hello(Hello{
                .requested_capabilities = capability(Capability::discovery),
                .required_capabilities = capability(Capability::discovery),
            })));
    const auto reconnected_welcome = receive_response(reconnected_client);
    const auto reconnected_welcome_payload =
        decode_welcome(reconnected_welcome.payload);
    check(std::holds_alternative<Welcome>(reconnected_welcome_payload) &&
              std::get<Welcome>(reconnected_welcome_payload).compositor_epoch == 78,
          "reconnected client negotiated the replacement transport epoch");
    close(reconnected_client);
    restarted_server.stop();

    const auto busy_socket_path =
        (temporary.path() / "cua-inject-v2-busy.sock").string();
    InjectionServer busy_server{
        busy_socket_path,
        ServerInfo{
            .compositor_epoch = 79,
            .supported_capabilities = capability(Capability::discovery),
            .enabled_capabilities = capability(Capability::discovery),
        }};
    check(busy_server.start(),
          "busy-test server started: " + busy_server.snapshot().last_error);

    std::array<int, 8> held_clients{};
    for (auto& held_client : held_clients)
        held_client = connect_client(busy_socket_path);
    for (int attempt = 0;
         attempt < 100 && busy_server.snapshot().accepted_connections < 8;
         ++attempt)
        std::this_thread::sleep_for(std::chrono::milliseconds(5));
    check(busy_server.snapshot().accepted_connections == 8,
          "server accepted eight concurrent held clients");

    const auto excess_client = connect_client(busy_socket_path);
    const auto busy_frame = receive_response(excess_client);
    const auto busy = decode_error(busy_frame.payload);
    check(busy_frame.header.type == MessageType::error &&
              std::holds_alternative<ProtocolError>(busy) &&
              std::get<ProtocolError>(busy).code == ErrorCode::server_busy,
          "excess client received typed server_busy refusal");
    close(excess_client);
    for (const auto held_client : held_clients)
        close(held_client);

    busy_server.stop();
    check(!std::filesystem::exists(busy_socket_path),
          "busy-test socket removed on stop");
    check(busy_server.snapshot().rejected_busy == 1,
          "busy connection rejection was recorded");

    const auto timeout_socket_path =
        (temporary.path() / "cua-inject-v2-timeout.sock").string();
    InjectionServer timeout_server{
        timeout_socket_path,
        ServerInfo{
            .compositor_epoch = 80,
            .supported_capabilities = capability(Capability::discovery),
            .enabled_capabilities = capability(Capability::discovery),
        },
        ServerOptions{.handshake_timeout = std::chrono::milliseconds{30}}};
    check(timeout_server.start(),
          "timeout-test server started: " + timeout_server.snapshot().last_error);
    std::array<int, 8> silent_clients{};
    for (auto& silent_client : silent_clients)
        silent_client = connect_client(timeout_socket_path);
    for (int attempt = 0;
         attempt < 100 && timeout_server.snapshot().timed_out_handshakes < 8;
         ++attempt)
        std::this_thread::sleep_for(std::chrono::milliseconds(5));
    check(timeout_server.snapshot().timed_out_handshakes == 8,
          "silent pre-handshake clients were reaped");
    const auto admitted_after_timeout = connect_client(timeout_socket_path);
    send_request(
        admitted_after_timeout,
        make_frame(
            MessageType::hello, 12,
            encode_hello(Hello{
                .requested_capabilities = capability(Capability::discovery),
                .required_capabilities = capability(Capability::discovery),
            })));
    check(receive_response(admitted_after_timeout).header.type ==
              MessageType::welcome,
          "new client was admitted after handshake timeout cleanup");
    close(admitted_after_timeout);
    for (const auto silent_client : silent_clients)
        close(silent_client);
    timeout_server.stop();

    const auto idle_socket_path =
        (temporary.path() / "cua-inject-v2-idle.sock").string();
    InjectionServer idle_server{
        idle_socket_path,
        ServerInfo{
            .compositor_epoch = 82,
            .supported_capabilities = capability(Capability::discovery),
            .enabled_capabilities = capability(Capability::discovery),
        },
        ServerOptions{
            .handshake_timeout = std::chrono::milliseconds{1000},
            .idle_timeout = std::chrono::milliseconds{30},
        }};
    check(idle_server.start(),
          "idle-test server started: " + idle_server.snapshot().last_error);
    const auto idle_client = connect_client(idle_socket_path);
    send_request(
        idle_client,
        make_frame(
            MessageType::hello, 15,
            encode_hello(Hello{
                .requested_capabilities = capability(Capability::discovery),
                .required_capabilities = capability(Capability::discovery),
            })));
    check(receive_response(idle_client).header.type == MessageType::welcome,
          "idle-test client negotiated a session");
    for (int attempt = 0;
         attempt < 100 && idle_server.snapshot().timed_out_idle < 1;
         ++attempt)
        std::this_thread::sleep_for(std::chrono::milliseconds(5));
    check(idle_server.snapshot().timed_out_idle == 1,
          "idle handshaken client was reaped");
    std::array<std::byte, 1> idle_probe{};
    check(recv(idle_client, idle_probe.data(), idle_probe.size(), 0) == 0,
          "idle client observed timeout shutdown");
    close(idle_client);
    idle_server.stop();

    const auto replaced_socket_path =
        (temporary.path() / "cua-inject-v2-replaced.sock").string();
    InjectionServer replaced_server{
        replaced_socket_path, ServerInfo{.compositor_epoch = 81}};
    check(replaced_server.start(),
          "replacement-test server started: " +
              replaced_server.snapshot().last_error);
    check(std::filesystem::remove(replaced_socket_path),
          "test removed the original socket directory entry");
    {
        std::ofstream replacement{replaced_socket_path};
        replacement << "replacement must survive server shutdown";
    }
    replaced_server.stop();
    check(std::filesystem::is_regular_file(replaced_socket_path),
          "server did not remove a replacement path");
    check(replaced_server.snapshot().last_error ==
              "socket path identity changed; refusing to unlink it",
          "replacement refusal is observable in status");
    check(std::filesystem::remove(replaced_socket_path),
          "test removed the preserved replacement path");

    const auto occupied_path = (temporary.path() / "occupied").string();
    {
        std::ofstream occupied{occupied_path};
        occupied << "not a socket";
    }
    InjectionServer collision{occupied_path, ServerInfo{.compositor_epoch = 88}};
    check(!collision.start(), "pre-existing path refused");
    collision.stop();
    check(std::filesystem::is_regular_file(occupied_path),
          "failed start did not remove an unowned path");
    check(std::filesystem::remove(occupied_path),
          "test removed the preserved occupied path");

    std::cout << "cua-inject-v2 transport tests passed\n";
    return 0;
}
