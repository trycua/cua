#include "cua_hyprland/protocol.hpp"

#include <algorithm>
#include <limits>

namespace cua::hyprland {
namespace {

void append_u16(std::vector<std::byte>& out, std::uint16_t value) {
    out.push_back(static_cast<std::byte>((value >> 8) & 0xff));
    out.push_back(static_cast<std::byte>(value & 0xff));
}

void append_u32(std::vector<std::byte>& out, std::uint32_t value) {
    for (int shift = 24; shift >= 0; shift -= 8)
        out.push_back(static_cast<std::byte>((value >> shift) & 0xff));
}

void append_u64(std::vector<std::byte>& out, std::uint64_t value) {
    for (int shift = 56; shift >= 0; shift -= 8)
        out.push_back(static_cast<std::byte>((value >> shift) & 0xff));
}

std::uint16_t read_u16(std::span<const std::byte> bytes, std::size_t offset) {
    return (std::to_integer<std::uint16_t>(bytes[offset]) << 8) |
           std::to_integer<std::uint16_t>(bytes[offset + 1]);
}

std::uint32_t read_u32(std::span<const std::byte> bytes, std::size_t offset) {
    std::uint32_t value = 0;
    for (std::size_t index = 0; index < 4; ++index)
        value = (value << 8) | std::to_integer<std::uint32_t>(bytes[offset + index]);
    return value;
}

std::uint64_t read_u64(std::span<const std::byte> bytes, std::size_t offset) {
    std::uint64_t value = 0;
    for (std::size_t index = 0; index < 8; ++index)
        value = (value << 8) | std::to_integer<std::uint64_t>(bytes[offset + index]);
    return value;
}

} // namespace

std::vector<std::byte> encode_frame(const Frame& frame) {
    if (frame.payload.size() > kMaxPayloadSize)
        return {};

    std::vector<std::byte> out;
    out.reserve(kHeaderSize + frame.payload.size());
    out.insert(out.end(), kMagic.begin(), kMagic.end());
    append_u16(out, frame.header.major);
    append_u16(out, frame.header.minor);
    append_u16(out, static_cast<std::uint16_t>(frame.header.type));
    append_u16(out, frame.header.flags);
    append_u64(out, frame.header.request_id);
    append_u32(out, static_cast<std::uint32_t>(frame.payload.size()));
    out.insert(out.end(), frame.payload.begin(), frame.payload.end());
    return out;
}

std::variant<Frame, DecodeError> decode_frame(std::span<const std::byte> bytes) {
    if (bytes.size() < kHeaderSize)
        return DecodeError::frame_too_small;
    if (!std::equal(kMagic.begin(), kMagic.end(), bytes.begin()))
        return DecodeError::bad_magic;

    Header header{
        .major = read_u16(bytes, 4),
        .minor = read_u16(bytes, 6),
        .type = static_cast<MessageType>(read_u16(bytes, 8)),
        .flags = read_u16(bytes, 10),
        .request_id = read_u64(bytes, 12),
        .payload_size = read_u32(bytes, 20),
    };

    if (header.major != kProtocolMajor || header.minor > kProtocolMinor)
        return DecodeError::unsupported_version;
    if (header.flags != 0)
        return DecodeError::nonzero_flags;
    if (header.payload_size > kMaxPayloadSize)
        return DecodeError::payload_too_large;
    if (bytes.size() != kHeaderSize + header.payload_size)
        return DecodeError::size_mismatch;

    return Frame{
        .header = header,
        .payload = std::vector<std::byte>(bytes.begin() + kHeaderSize, bytes.end()),
    };
}

std::vector<std::byte> encode_hello(const Hello& hello) {
    std::vector<std::byte> out;
    out.reserve(16);
    append_u64(out, hello.requested_capabilities);
    append_u64(out, hello.required_capabilities);
    return out;
}

std::variant<Hello, DecodeError> decode_hello(std::span<const std::byte> payload) {
    if (payload.size() != 16)
        return DecodeError::malformed_payload;
    return Hello{
        .requested_capabilities = read_u64(payload, 0),
        .required_capabilities = read_u64(payload, 8),
    };
}

std::vector<std::byte> encode_welcome(const Welcome& welcome) {
    std::vector<std::byte> out;
    out.reserve(32);
    append_u64(out, welcome.compositor_epoch);
    append_u64(out, welcome.supported_capabilities);
    append_u64(out, welcome.enabled_capabilities);
    append_u32(out, welcome.max_frame_size);
    append_u32(out, 0);
    return out;
}

std::variant<Welcome, DecodeError> decode_welcome(std::span<const std::byte> payload) {
    if (payload.size() != 32 || read_u32(payload, 28) != 0)
        return DecodeError::malformed_payload;
    return Welcome{
        .compositor_epoch = read_u64(payload, 0),
        .supported_capabilities = read_u64(payload, 8),
        .enabled_capabilities = read_u64(payload, 16),
        .max_frame_size = read_u32(payload, 24),
    };
}

std::vector<std::byte> encode_error(const ProtocolError& error) {
    constexpr std::size_t kErrorPrefixSize = 4;
    const auto detail_size = std::min(error.detail.size(), kMaxPayloadSize - kErrorPrefixSize);
    std::vector<std::byte> out;
    out.reserve(kErrorPrefixSize + detail_size);
    append_u32(out, static_cast<std::uint32_t>(error.code));
    for (std::size_t index = 0; index < detail_size; ++index)
        out.push_back(static_cast<std::byte>(error.detail[index]));
    return out;
}

std::variant<ProtocolError, DecodeError> decode_error(std::span<const std::byte> payload) {
    if (payload.size() < 4)
        return DecodeError::malformed_payload;
    std::string detail;
    detail.reserve(payload.size() - 4);
    for (const auto byte : payload.subspan(4))
        detail.push_back(static_cast<char>(byte));
    return ProtocolError{
        .code = static_cast<ErrorCode>(read_u32(payload, 0)),
        .detail = std::move(detail),
    };
}

Frame make_frame(MessageType type, std::uint64_t request_id,
                 std::vector<std::byte> payload) {
    return Frame{
        .header = Header{
            .type = type,
            .request_id = request_id,
            .payload_size = static_cast<std::uint32_t>(payload.size()),
        },
        .payload = std::move(payload),
    };
}

Frame make_error(std::uint64_t request_id, ErrorCode code, std::string detail) {
    return make_frame(MessageType::error, request_id,
                      encode_error(ProtocolError{code, std::move(detail)}));
}

} // namespace cua::hyprland
