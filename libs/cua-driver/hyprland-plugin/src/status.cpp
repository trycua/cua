#include "cua_hyprland/status.hpp"

#include "cua_hyprland/protocol.hpp"

#include <format>
#include <string>

namespace cua::hyprland {
namespace {

bool continuation(unsigned char value) {
    return value >= 0x80 && value <= 0xbf;
}

std::size_t valid_sequence_size(std::string_view value, std::size_t offset) {
    const auto first = static_cast<unsigned char>(value[offset]);
    const auto remaining = value.size() - offset;
    if (first < 0x80)
        return 1;
    if (first >= 0xc2 && first <= 0xdf && remaining >= 2 &&
        continuation(static_cast<unsigned char>(value[offset + 1])))
        return 2;
    if (remaining >= 3 && first >= 0xe0 && first <= 0xef) {
        const auto second = static_cast<unsigned char>(value[offset + 1]);
        const auto third = static_cast<unsigned char>(value[offset + 2]);
        const auto valid_second =
            (first == 0xe0 && second >= 0xa0 && second <= 0xbf) ||
            (first == 0xed && second >= 0x80 && second <= 0x9f) ||
            (((first >= 0xe1 && first <= 0xec) ||
              (first >= 0xee && first <= 0xef)) &&
             continuation(second));
        if (valid_second && continuation(third))
            return 3;
    }
    if (remaining >= 4 && first >= 0xf0 && first <= 0xf4) {
        const auto second = static_cast<unsigned char>(value[offset + 1]);
        const auto third = static_cast<unsigned char>(value[offset + 2]);
        const auto fourth = static_cast<unsigned char>(value[offset + 3]);
        const auto valid_second =
            (first == 0xf0 && second >= 0x90 && second <= 0xbf) ||
            (first == 0xf4 && second >= 0x80 && second <= 0x8f) ||
            (first >= 0xf1 && first <= 0xf3 && continuation(second));
        if (valid_second && continuation(third) && continuation(fourth))
            return 4;
    }
    return 0;
}

void append_byte_escape(std::string& escaped, unsigned char value) {
    constexpr char hex[] = "0123456789abcdef";
    escaped += "\\u00";
    escaped += hex[(value >> 4) & 0x0f];
    escaped += hex[value & 0x0f];
}

std::string escape_json(std::string_view value) {
    std::string escaped;
    escaped.reserve(value.size());
    for (std::size_t index = 0; index < value.size();) {
        const auto character = value[index];
        const auto byte = static_cast<unsigned char>(character);
        switch (character) {
            case '\\':
                escaped += "\\\\";
                ++index;
                continue;
            case '"':
                escaped += "\\\"";
                ++index;
                continue;
            case '\n':
                escaped += "\\n";
                ++index;
                continue;
            case '\r':
                escaped += "\\r";
                ++index;
                continue;
            case '\t':
                escaped += "\\t";
                ++index;
                continue;
            default: break;
        }
        if (byte < 0x20 || byte == 0x7f) {
            append_byte_escape(escaped, byte);
            ++index;
            continue;
        }
        const auto sequence_size = valid_sequence_size(value, index);
        if (sequence_size == 0) {
            append_byte_escape(escaped, byte);
            ++index;
            continue;
        }
        escaped.append(value.substr(index, sequence_size));
        index += sequence_size;
    }
    return escaped;
}

constexpr std::string_view json_bool(bool value) {
    return value ? "true" : "false";
}

} // namespace

bool is_valid_utf8(std::string_view value) {
    for (std::size_t offset = 0; offset < value.size();) {
        const auto size = valid_sequence_size(value, offset);
        if (size == 0)
            return false;
        offset += size;
    }
    return true;
}

std::string render_status_json(const StatusReport& report) {
    return std::format(
        R"({{"name":"cua-hyprland-plugin","plugin_version":"{}",)"
        R"("state":"discovery_only","protocol":{{"major":{},"minor":{},)"
        R"("max_frame_bytes":{}}},"compositor_epoch":{},"abi":{{)"
        R"("compiled_hash":"{}","runtime_hash":"{}","match":{}}},)"
        R"("configured":{},"transport":{{"ready":{},"socket":"{}",)"
        R"("mode":"0600","peer_policy":"same_uid","last_error":"{}"}},)"
        R"("capabilities":{{"supported":["discovery"],"enabled":{},)"
        R"("mutation":"disabled_pending_rfc_and_agent_seat"}},)"
        R"("connections":{{"accepted":{},"rejected_peers":{},)"
        R"("rejected_busy":{},"timed_out_handshakes":{},)"
        R"("timed_out_idle":{},"malformed_frames":{}}}}})",
        escape_json(report.plugin_version), kProtocolMajor, kProtocolMinor,
        kMaxFrameSize, report.compositor_epoch, escape_json(report.compiled_hash),
        escape_json(report.runtime_hash),
        json_bool(report.compiled_hash == report.runtime_hash),
        json_bool(report.configured),
        json_bool(report.transport_ready), escape_json(report.socket_path),
        escape_json(report.last_error),
        report.discovery_enabled ? R"(["discovery"])" : "[]",
        report.accepted_connections, report.rejected_peers, report.rejected_busy,
        report.timed_out_handshakes, report.timed_out_idle,
        report.malformed_frames);
}

std::string render_status_text(const StatusReport& report) {
    return std::format(
        "cua-hyprland-plugin {}\nconfigured: {}\ntransport: {}\nsocket: {}\n"
        "last error: {}\nprotocol: {}.{}\ncompositor epoch: {}\n"
        "ABI compiled: {}\nABI runtime: {}\nABI match: {}\n"
        "connections: accepted={}, "
        "rejected_peers={}, rejected_busy={}, timed_out_handshakes={}, "
        "timed_out_idle={}, malformed_frames={}\n"
        "capabilities: discovery only; background mutation disabled\n",
        report.plugin_version, report.configured ? "yes" : "no",
        report.transport_ready ? "ready" : "stopped",
        report.socket_path.empty() ? "<not resolved>" :
                                     escape_json(report.socket_path),
        report.last_error.empty() ? "<none>" : escape_json(report.last_error),
        kProtocolMajor, kProtocolMinor, report.compositor_epoch,
        escape_json(report.compiled_hash), escape_json(report.runtime_hash),
        report.compiled_hash == report.runtime_hash ? "yes" : "no",
        report.accepted_connections, report.rejected_peers,
        report.rejected_busy, report.timed_out_handshakes,
        report.timed_out_idle, report.malformed_frames);
}

} // namespace cua::hyprland
