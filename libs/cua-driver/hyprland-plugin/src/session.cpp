#include "cua_hyprland/session.hpp"

namespace cua::hyprland {

Frame Session::handle(const Frame& request, const ServerInfo& server) {
    if (!m_handshaken) {
        if (request.header.type != MessageType::hello) {
            return make_error(request.header.request_id, ErrorCode::handshake_required,
                              "HELLO must be the first request");
        }

        const auto decoded = decode_hello(request.payload);
        if (!std::holds_alternative<Hello>(decoded)) {
            return make_error(request.header.request_id, ErrorCode::malformed_frame,
                              "invalid HELLO payload");
        }

        const auto hello = std::get<Hello>(decoded);
        const auto unavailable = hello.required_capabilities & ~server.enabled_capabilities;
        if (unavailable != 0) {
            return make_error(request.header.request_id,
                              ErrorCode::capability_unavailable,
                              "required capabilities are unavailable");
        }

        m_negotiated_capabilities =
            server.enabled_capabilities &
            (hello.requested_capabilities | hello.required_capabilities);
        m_handshaken = true;
        return make_frame(
            MessageType::welcome, request.header.request_id,
            encode_welcome(Welcome{
                .compositor_epoch = server.compositor_epoch,
                .supported_capabilities = server.supported_capabilities,
                .enabled_capabilities = m_negotiated_capabilities,
            }));
    }

    switch (request.header.type) {
        case MessageType::ping:
            if (!request.payload.empty())
                return make_error(request.header.request_id, ErrorCode::malformed_frame,
                                  "PING payload must be empty");
            return make_frame(MessageType::pong, request.header.request_id);
        case MessageType::status_request:
            if (!request.payload.empty())
                return make_error(request.header.request_id, ErrorCode::malformed_frame,
                                  "STATUS payload must be empty");
            return make_frame(
                MessageType::status_response, request.header.request_id,
                encode_welcome(Welcome{
                    .compositor_epoch = server.compositor_epoch,
                    .supported_capabilities = server.supported_capabilities,
                    .enabled_capabilities = m_negotiated_capabilities &
                                            server.enabled_capabilities,
                }));
        case MessageType::hello:
            return make_error(request.header.request_id, ErrorCode::malformed_frame,
                              "HELLO may only be sent once");
        case MessageType::pointer_motion:
        case MessageType::pointer_button:
        case MessageType::pointer_axis:
        case MessageType::pointer_drag:
        case MessageType::keyboard_key:
        case MessageType::keyboard_text:
            return make_error(request.header.request_id,
                              ErrorCode::background_unavailable,
                              "background mutation is not enabled in this build");
        default:
            return make_error(request.header.request_id, ErrorCode::malformed_frame,
                              "message type is invalid for a client request");
    }
}

bool Session::handshaken() const {
    return m_handshaken;
}

} // namespace cua::hyprland
