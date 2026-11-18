#pragma once

#include <array>
#include <cstddef>
#include <cstdint>
#include <span>
#include <string>
#include <variant>
#include <vector>

namespace cua::hyprland {

constexpr std::array<std::byte, 4> kMagic{
    std::byte{'C'}, std::byte{'U'}, std::byte{'A'}, std::byte{'2'}};
constexpr std::uint16_t kProtocolMajor = 2;
constexpr std::uint16_t kProtocolMinor = 0;
constexpr std::size_t kHeaderSize = 24;
constexpr std::size_t kMaxPayloadSize = 4096;
constexpr std::size_t kMaxFrameSize = kHeaderSize + kMaxPayloadSize;

enum class MessageType : std::uint16_t {
    hello = 1,
    welcome = 2,
    ping = 3,
    pong = 4,
    status_request = 5,
    status_response = 6,
    pointer_motion = 0x100,
    pointer_button = 0x101,
    pointer_axis = 0x102,
    pointer_drag = 0x103,
    keyboard_key = 0x110,
    keyboard_text = 0x111,
    error = 0xffff,
};

enum class Capability : std::uint64_t {
    discovery = 1ULL << 0,
    pointer_motion = 1ULL << 1,
    pointer_button = 1ULL << 2,
    pointer_axis = 1ULL << 3,
    pointer_drag = 1ULL << 4,
    keyboard_key = 1ULL << 5,
    keyboard_text = 1ULL << 6,
    observation = 1ULL << 7,
};

constexpr std::uint64_t capability(Capability value) {
    return static_cast<std::uint64_t>(value);
}

enum class ErrorCode : std::uint32_t {
    malformed_frame = 1,
    unsupported_version = 2,
    handshake_required = 3,
    capability_unavailable = 4,
    permission_denied = 5,
    stale_epoch = 6,
    server_busy = 7,
    background_unavailable = 8,
};

struct Header {
    std::uint16_t major = kProtocolMajor;
    std::uint16_t minor = kProtocolMinor;
    MessageType type = MessageType::error;
    std::uint16_t flags = 0;
    std::uint64_t request_id = 0;
    std::uint32_t payload_size = 0;
};

struct Frame {
    Header header;
    std::vector<std::byte> payload;
};

struct Hello {
    std::uint64_t requested_capabilities = 0;
    std::uint64_t required_capabilities = 0;
};

struct Welcome {
    std::uint64_t compositor_epoch = 0;
    std::uint64_t supported_capabilities = 0;
    std::uint64_t enabled_capabilities = 0;
    std::uint32_t max_frame_size = static_cast<std::uint32_t>(kMaxFrameSize);
};

struct ProtocolError {
    ErrorCode code = ErrorCode::malformed_frame;
    std::string detail;
};

enum class DecodeError {
    frame_too_small,
    bad_magic,
    unsupported_version,
    nonzero_flags,
    payload_too_large,
    size_mismatch,
    malformed_payload,
};

std::vector<std::byte> encode_frame(const Frame& frame);
std::variant<Frame, DecodeError> decode_frame(std::span<const std::byte> bytes);

std::vector<std::byte> encode_hello(const Hello& hello);
std::variant<Hello, DecodeError> decode_hello(std::span<const std::byte> payload);
std::vector<std::byte> encode_welcome(const Welcome& welcome);
std::variant<Welcome, DecodeError> decode_welcome(std::span<const std::byte> payload);
std::vector<std::byte> encode_error(const ProtocolError& error);
std::variant<ProtocolError, DecodeError> decode_error(std::span<const std::byte> payload);

Frame make_frame(MessageType type, std::uint64_t request_id,
                 std::vector<std::byte> payload = {});
Frame make_error(std::uint64_t request_id, ErrorCode code, std::string detail);

} // namespace cua::hyprland
