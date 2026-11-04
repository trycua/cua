#include "cua_hyprland/protocol.hpp"
#include "cua_hyprland/session.hpp"

#include <cstdlib>
#include <iostream>
#include <limits>
#include <span>
#include <string>
#include <variant>

namespace {

void check(bool condition, const std::string& message) {
    if (condition)
        return;
    std::cerr << "FAIL: " << message << '\n';
    std::exit(1);
}

template <typename T>
const T& expect(const auto& value, const char* message) {
    check(std::holds_alternative<T>(value), message);
    return std::get<T>(value);
}

} // namespace

int main() {
    using namespace cua::hyprland;

    const auto hello = make_frame(
        MessageType::hello, 42,
        encode_hello(Hello{
            .requested_capabilities = capability(Capability::discovery),
            .required_capabilities = capability(Capability::discovery),
        }));
    const auto bytes = encode_frame(hello);
    const auto decoded_result = decode_frame(bytes);
    const auto& decoded = expect<Frame>(decoded_result, "frame round trip");
    check(decoded.header.request_id == 42, "request ID round trip");
    const auto decoded_hello_result = decode_hello(decoded.payload);
    const auto& decoded_hello =
        expect<Hello>(decoded_hello_result, "HELLO round trip");
    check(decoded_hello.required_capabilities == capability(Capability::discovery),
          "HELLO capabilities round trip");

    check(std::get<DecodeError>(decode_frame({})) == DecodeError::frame_too_small,
          "empty frame rejected");

    const auto boundary_frame = make_frame(
        MessageType::error, std::numeric_limits<std::uint64_t>::max(),
        std::vector<std::byte>(kMaxPayloadSize, std::byte{0x5a}));
    const auto boundary_bytes = encode_frame(boundary_frame);
    check(boundary_bytes.size() == kMaxFrameSize,
          "maximum-sized frame encoded");
    const auto decoded_boundary_result = decode_frame(boundary_bytes);
    const auto& decoded_boundary =
        expect<Frame>(decoded_boundary_result, "maximum-sized frame decoded");
    check(decoded_boundary.header.request_id ==
              std::numeric_limits<std::uint64_t>::max() &&
              decoded_boundary.payload.size() == kMaxPayloadSize,
          "maximum-sized frame fields round trip");
    check(encode_frame(make_frame(
              MessageType::error, 1,
              std::vector<std::byte>(kMaxPayloadSize + 1, std::byte{0})))
              .empty(),
          "oversized payload cannot be encoded");

    auto bad_magic = bytes;
    bad_magic[0] = std::byte{'X'};
    check(std::get<DecodeError>(decode_frame(bad_magic)) == DecodeError::bad_magic,
          "bad magic rejected");

    auto future_version = bytes;
    future_version[5] = std::byte{3};
    check(std::get<DecodeError>(decode_frame(future_version)) ==
              DecodeError::unsupported_version,
          "future major version rejected");

    auto oversized_declared_payload = bytes;
    oversized_declared_payload[20] = std::byte{0};
    oversized_declared_payload[21] = std::byte{0};
    oversized_declared_payload[22] = std::byte{0x10};
    oversized_declared_payload[23] = std::byte{0x01};
    check(std::get<DecodeError>(decode_frame(oversized_declared_payload)) ==
              DecodeError::payload_too_large,
          "oversized declared payload rejected before size matching");

    auto nonzero_flags = bytes;
    nonzero_flags[11] = std::byte{1};
    check(std::get<DecodeError>(decode_frame(nonzero_flags)) ==
              DecodeError::nonzero_flags,
          "nonzero flags rejected");

    auto truncated = bytes;
    truncated.pop_back();
    check(std::get<DecodeError>(decode_frame(truncated)) == DecodeError::size_mismatch,
          "truncated frame rejected");

    const ServerInfo server{
        .compositor_epoch = 99,
        .supported_capabilities = capability(Capability::discovery),
        .enabled_capabilities = capability(Capability::discovery),
    };

    Session before_hello;
    const auto handshake_error = before_hello.handle(
        make_frame(MessageType::ping, 1), server);
    check(handshake_error.header.type == MessageType::error,
          "pre-handshake request rejected");
    const auto decoded_handshake_error_result = decode_error(handshake_error.payload);
    const auto& decoded_handshake_error =
        expect<ProtocolError>(decoded_handshake_error_result,
                              "handshake error decodes");
    check(decoded_handshake_error.code == ErrorCode::handshake_required,
          "handshake error is typed");

    Session unavailable;
    const auto unavailable_response = unavailable.handle(
        make_frame(
            MessageType::hello, 2,
            encode_hello(Hello{
                .requested_capabilities = capability(Capability::pointer_button),
                .required_capabilities = capability(Capability::pointer_button),
            })),
        server);
    const auto unavailable_error_result = decode_error(unavailable_response.payload);
    const auto& unavailable_error =
        expect<ProtocolError>(unavailable_error_result,
                              "capability error decodes");
    check(unavailable_error.code == ErrorCode::capability_unavailable,
          "required mutation capability refused");
    check(!unavailable.handshaken(), "failed negotiation does not establish session");

    Session session;
    const auto welcome_response = session.handle(hello, server);
    check(welcome_response.header.type == MessageType::welcome,
          "HELLO receives WELCOME");
    const auto welcome_result = decode_welcome(welcome_response.payload);
    const auto& welcome = expect<Welcome>(welcome_result, "WELCOME decodes");
    check(welcome.compositor_epoch == 99, "WELCOME carries compositor epoch");
    check(welcome.enabled_capabilities == capability(Capability::discovery),
          "WELCOME carries negotiated capabilities");
    check(session.handshaken(), "session records successful handshake");

    const auto status_response =
        session.handle(make_frame(MessageType::status_request, 45), server);
    const auto status_result = decode_welcome(status_response.payload);
    const auto& status =
        expect<Welcome>(status_result, "STATUS_RESPONSE payload decodes");
    check(status_response.header.type == MessageType::status_response &&
              status.enabled_capabilities == capability(Capability::discovery),
          "STATUS_RESPONSE reports only session-negotiated capabilities");

    const ServerInfo revoked_server{
        .compositor_epoch = 99,
        .supported_capabilities = capability(Capability::discovery),
        .enabled_capabilities = 0,
    };
    const auto revoked_status_response = session.handle(
        make_frame(MessageType::status_request, 46), revoked_server);
    const auto revoked_status_result =
        decode_welcome(revoked_status_response.payload);
    const auto& revoked_status =
        expect<Welcome>(revoked_status_result, "revoked STATUS_RESPONSE decodes");
    check(revoked_status.enabled_capabilities == 0,
          "STATUS_RESPONSE reflects capability revocation without granting new bits");

    Session no_capabilities;
    const auto no_capabilities_welcome = no_capabilities.handle(
        make_frame(MessageType::hello, 47, encode_hello(Hello{})), server);
    const auto no_capabilities_result =
        decode_welcome(no_capabilities_welcome.payload);
    check(expect<Welcome>(no_capabilities_result,
                          "zero-capability WELCOME decodes")
                  .enabled_capabilities == 0,
          "session cannot gain a capability it did not request");
    const auto no_capabilities_status = no_capabilities.handle(
        make_frame(MessageType::status_request, 48), server);
    check(expect<Welcome>(decode_welcome(no_capabilities_status.payload),
                          "zero-capability STATUS_RESPONSE decodes")
                  .enabled_capabilities == 0,
          "status does not elevate a zero-capability session");

    const auto pong = session.handle(make_frame(MessageType::ping, 43), server);
    check(pong.header.type == MessageType::pong && pong.header.request_id == 43,
          "PING receives correlated PONG");

    const auto mutation = session.handle(
        make_frame(MessageType::pointer_button, 44), server);
    const auto mutation_error_result = decode_error(mutation.payload);
    const auto& mutation_error =
        expect<ProtocolError>(mutation_error_result, "mutation error decodes");
    check(mutation_error.code == ErrorCode::background_unavailable,
          "mutation remains fail-closed");

    const auto invalid_direction = session.handle(
        make_frame(MessageType::welcome, 49), server);
    const auto invalid_direction_error_result =
        decode_error(invalid_direction.payload);
    check(expect<ProtocolError>(invalid_direction_error_result,
                                "invalid-direction error decodes")
                  .code == ErrorCode::malformed_frame,
          "server-only message types are reported as malformed client requests");

    const auto long_error = encode_error(
        ProtocolError{ErrorCode::server_busy, std::string(kMaxPayloadSize * 2, 'x')});
    check(long_error.size() == kMaxPayloadSize, "error detail is bounded");

    std::cout << "cua-inject-v2 protocol tests passed\n";
    return 0;
}
