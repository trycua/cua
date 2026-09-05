#include "primary_trace.hpp"
#include <src/Compositor.hpp>
#include <src/managers/SeatManager.hpp>
#include <src/pointer/PointerManager.hpp>
#include <src/plugins/PluginAPI.hpp>
#include <src/protocols/core/Compositor.hpp>
#include <wayland-server-core.h>

#include <array>
#include <chrono>
#include <cstring>
#include <format>
#include <stdexcept>

namespace cua::hyprland {
struct PrimaryTrace::Impl {
    using Clock = std::chrono::steady_clock;
    struct Event {
        std::uint64_t ns = 0;
        const char* kind = "";
        double x = 0, y = 0;
        unsigned actor = 0, state = 0;
    };
    static inline Impl* live = nullptr;
    std::array<Event, 32768> events{};
    std::size_t count = 0;
    bool active = false, overflow = false, timed_out = false;
    Clock::time_point started{};
    void* plugin;
    CFunctionHook* motion = nullptr;
    wl_protocol_logger* logger = nullptr;
    CHyprSignalListener pointer_focus, keyboard_focus;
    std::function<unsigned(wl_resource*)> actor;

    explicit Impl(void* handle, std::function<unsigned(wl_resource*)> classify)
        : plugin(handle), actor(std::move(classify)) {}

    void start() {
        if (live) throw std::runtime_error("primary trace already installed");
        const auto matches = HyprlandAPI::findFunctionsByName(plugin, "_ZN7Pointer15CPointerManager13onCursorMovedEv");
        if (matches.size() != 1) throw std::runtime_error("exact primary cursor trace hook unavailable");
        motion = HyprlandAPI::createFunctionHook(plugin, matches.front().address, reinterpret_cast<void*>(moved));
        if (!motion || !motion->hook()) throw std::runtime_error("primary cursor trace hook refused");
        live = this;
        logger = wl_display_add_protocol_logger(g_pCompositor->m_wlDisplay, protocol, this);
        if (!logger) throw std::runtime_error("primary protocol trace unavailable");
        pointer_focus = g_pSeatManager->m_events.pointerFocusChange.listen([this] { record("pointer_focus", 0); });
        keyboard_focus = g_pSeatManager->m_events.keyboardFocusChange.listen([this] { record("keyboard_focus", 0); });
    }
    ~Impl() {
        active = false;
        pointer_focus.reset(); keyboard_focus.reset();
        if (logger) wl_protocol_logger_destroy(logger);
        if (motion) HyprlandAPI::removeFunctionHook(plugin, motion);
        if (live == this) live = nullptr;
    }
    void record(const char* kind, unsigned owner, unsigned state = 0) noexcept {
        if (!active) return;
        const auto now = Clock::now();
        if (now - started > std::chrono::seconds(60)) { timed_out = true; active = false; return; }
        if (count == events.size()) { overflow = true; active = false; return; }
        const auto position = Pointer::mgr()->position();
        events[count++] = Event{static_cast<std::uint64_t>(std::chrono::duration_cast<std::chrono::nanoseconds>(now.time_since_epoch()).count()),
            kind, position.x, position.y, owner, state};
    }
    static void moved(void* pointer) {
        auto* self = live;
        // This hook records the compositor-owned position on *every* mutation,
        // including two warps within one event-loop turn. It never changes it.
        self->record("cursor", 0);
        reinterpret_cast<void (*)(void*)>(self->motion->m_original)(pointer);
    }
    static void protocol(void* data, wl_protocol_logger_type direction, const wl_protocol_logger_message* message) {
        auto& self = *static_cast<Impl*>(data);
        if (!self.active || direction != WL_PROTOCOL_LOGGER_EVENT) return;
        const auto* type = wl_resource_get_class(message->resource);
        const auto* name = message->message->name;
        const auto owner = self.actor(message->resource);
        // Only event categories and button/key state are retained, never typed
        // keycodes, text, window names, or arbitrary protocol arguments.
        if (!std::strcmp(type, "wl_pointer")) {
            if (!std::strcmp(name, "motion")) self.record("pointer_motion", owner);
            else if (!std::strcmp(name, "enter")) self.record("pointer_enter", owner);
            else if (!std::strcmp(name, "leave")) self.record("pointer_leave", owner);
            else if (!std::strcmp(name, "button")) self.record("pointer_button", owner, message->arguments[3].u);
            else if (!std::strcmp(name, "axis")) self.record("pointer_axis", owner);
        } else if (!std::strcmp(type, "wl_keyboard")) {
            if (!std::strcmp(name, "enter")) self.record("keyboard_enter", owner);
            else if (!std::strcmp(name, "leave")) self.record("keyboard_leave", owner);
            else if (!std::strcmp(name, "key")) self.record("keyboard_key", owner, message->arguments[3].u);
        }
    }
    std::string request(const std::string& command, unsigned after) {
        if (command == "TRACE_START") {
            count = 0; overflow = false; timed_out = false; started = Clock::now(); active = true;
            record("start", 0);
        } else if (command == "TRACE_STOP") {
            record("stop", 0); active = false;
        } else if (command != "TRACE_READ") {
            return R"({"ok":false,"code":"invalid_request"})";
        }
        std::string rows;
        for (std::size_t i = after; i < count && i < static_cast<std::size_t>(after) + 8; ++i) {
            const auto& event = events[i];
            if (!rows.empty()) rows += ',';
            rows += std::format(R"([{}, {}, "{}", {}, {}, {}, {}])", i + 1, event.ns, event.kind, event.x, event.y, event.actor, event.state);
        }
        return std::format(R"({{"ok":true,"active":{},"overflow":{},"timed_out":{},"hook":true,"count":{},"events":[{}]}})",
            active, overflow, timed_out, count, rows);
    }
};
PrimaryTrace::PrimaryTrace(void* plugin, std::function<unsigned(wl_resource*)> actor)
    : impl_(std::make_unique<Impl>(plugin, std::move(actor))) { impl_->start(); }
PrimaryTrace::~PrimaryTrace() = default;
std::string PrimaryTrace::request(const std::string& command, unsigned after) { return impl_->request(command, after); }
void PrimaryTrace::mark(const char* kind, unsigned actor) { impl_->record(kind, actor); }
}
