// Independent-seat design adapted from Dillon DuPont's Hyprland prototype.
// All resource dispatch below belongs to this seat, never CSeatManager's focus.
#include "input_experiment.hpp"
#include "drag_geometry.hpp"
#include "primary_trace.hpp"
#include "seat_lifetime.hpp"
#include "owned_socket_path.hpp"

#include <src/Compositor.hpp>
#include <src/devices/IKeyboard.hpp>
#include <src/event/EventBus.hpp>
#include <src/managers/SeatManager.hpp>
#include <src/output/Monitor.hpp>
#include <src/protocols/core/Compositor.hpp>
#include <src/state/MonitorState.hpp>
#include <wayland.hpp>

#include <openssl/evp.h>
#include <openssl/rand.h>
#include <xkbcommon/xkbcommon.h>

#include <algorithm>
#include <array>
#include <charconv>
#include <cerrno>
#include <chrono>
#include <cmath>
#include <cstring>
#include <filesystem>
#include <fcntl.h>
#include <format>
#include <memory>
#include <optional>
#include <stdexcept>
#include <string_view>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <unistd.h>

namespace cua::hyprland {
namespace {
using Clock = std::chrono::steady_clock;
constexpr std::size_t kMaxPacket = 2048;
constexpr std::size_t kMaxClients = 8;
constexpr std::size_t kMaxResources = 512;

std::uint64_t unix_ms() {
    return std::chrono::duration_cast<std::chrono::milliseconds>(
               std::chrono::system_clock::now().time_since_epoch()).count();
}
std::uint32_t event_ms() {
    return static_cast<std::uint32_t>(std::chrono::duration_cast<std::chrono::milliseconds>(
               Clock::now().time_since_epoch()).count());
}
std::string nonce() {
    std::array<unsigned char, 16> bytes{};
    if (RAND_bytes(bytes.data(), bytes.size()) != 1)
        throw std::runtime_error("input experiment entropy unavailable");
    std::string result;
    for (auto b : bytes)
        result += std::format("{:02x}", b);
    return result;
}
std::vector<unsigned char> unhex(std::string_view value) {
    if (value.size() % 2)
        throw std::runtime_error("invalid hex");
    std::vector<unsigned char> bytes;
    for (std::size_t i = 0; i < value.size(); i += 2) {
        unsigned int n = 0;
        const auto [p, error] = std::from_chars(value.data() + i, value.data() + i + 2, n, 16);
        if (error != std::errc{} || p != value.data() + i + 2)
            throw std::runtime_error("invalid hex");
        bytes.push_back(static_cast<unsigned char>(n));
    }
    return bytes;
}
std::uint64_t number(std::string_view value, int base = 10) {
    std::uint64_t n = 0;
    const auto [p, error] = std::from_chars(value.data(), value.data() + value.size(), n, base);
    if (value.empty() || error != std::errc{} || p != value.data() + value.size())
        throw std::runtime_error("invalid integer");
    return n;
}
double real(std::string_view value) {
    double n = 0;
    const auto [p, error] = std::from_chars(value.data(), value.data() + value.size(), n);
    if (value.empty() || error != std::errc{} || p != value.data() + value.size() || !std::isfinite(n))
        throw std::runtime_error("invalid coordinate");
    return n;
}
std::vector<std::string> fields(std::string_view packet) {
    if (packet.empty() || packet.front() == ' ' || packet.back() == ' ')
        throw std::runtime_error("invalid packet");
    std::vector<std::string> result;
    std::size_t start = 0;
    for (std::size_t i = 0; i <= packet.size(); ++i) {
        if (i < packet.size() && (packet[i] < 32 || packet[i] > 126))
            throw std::runtime_error("non-ASCII packet");
        if (i != packet.size() && packet[i] != ' ')
            continue;
        if (i == start || result.size() >= 16)
            throw std::runtime_error("invalid fields");
        result.emplace_back(packet.substr(start, i - start));
        start = i + 1;
    }
    return result;
}
std::string refusal(std::string_view code) {
    // Only internal fixed identifiers enter JSON. No caller-provided strings.
    return std::format(R"({{"ok":false,"code":"{}","detail":"{}"}})", code, code);
}
constexpr auto kDelivered = R"({"ok":true,"effect":"unverifiable","route":"synthetic_events"})";
} // namespace

struct InputExperiment::Impl {
    struct Seat { SP<CWlSeat> wl; bool dead = false; };
    struct Pointer { SP<CWlPointer> wl; bool dead = false; WP<CWLSurfaceResource> focus; };
    struct Keyboard { SP<CWlKeyboard> wl; bool dead = false; WP<CWLSurfaceResource> focus; };
    struct Touch { SP<CWlTouch> wl; bool dead = false; };
    struct Client {
        Impl* owner = nullptr;
        int fd = -1;
        wl_event_source* source = nullptr;
        bool dead = false, hello = false;
        std::string challenge, token;
        WP<Desktop::View::CWindow> window;
        WP<CWLSurfaceResource> surface;
        std::array<double, 6> geometry{};
        std::uint64_t revision = 1, sequence = 0, approved_deadline = 0;
        Clock::time_point activity = Clock::now();
        CHyprSignalListener unmap, destroy;
        ~Client() {
            if (source) wl_event_source_remove(source);
            if (fd >= 0) close(fd);
        }
    };
    struct Drag {
        Client* client;
        double x1, y1, x2, y2;
        Clock::time_point start;
        unsigned duration;
        DragGeometry geometry;
    };
    int listener = -1;
    wl_event_source* listen_source = nullptr;
    wl_event_source* timer = nullptr;
    wl_global* global = nullptr;
    OwnedSocketPath socket_path;
    std::string_view socket_cleanup = "not_bound";
    std::string path, epoch = nonce(), keymap_text;
    std::vector<unsigned char> public_key;
    std::vector<std::unique_ptr<Client>> clients;
    std::vector<std::unique_ptr<Seat>> seats;
    std::vector<std::unique_ptr<Pointer>> pointers;
    std::vector<std::unique_ptr<Keyboard>> keyboards;
    std::vector<std::unique_ptr<Touch>> touches;
    Client* lease = nullptr;
    Client* reservation = nullptr;
    std::uint64_t desktop_generation = 1;
    std::uint64_t capabilities = 0, dispatches = 0;
    Clock::time_point expires{};
    std::optional<Drag> drag;
    std::uint32_t held_button = 0;
    std::vector<std::uint32_t> held_keys;
    xkb_context* xkb_context_ = nullptr;
    xkb_keymap* keymap = nullptr;
    xkb_state* keyboard_state = nullptr;
    int keymap_fd = -1;
    bool retired = false, suspended = true;
    unsigned lane;
    std::array<Impl*, 2> peers{};
    PrimaryTrace* trace = nullptr;

    explicit Impl(const std::string& directory, unsigned index) : lane(index) {
        public_key = unhex(CUA_HYPRLAND_TEST_OPERATOR_KEY);
        if (public_key.size() != 32)
            throw std::runtime_error("invalid test operator public key");
        path = directory + (lane == 0 ? "/cua-input-test.sock" : "/cua-input-test-2.sock");
        if (path.size() >= sizeof(sockaddr_un::sun_path))
            throw std::runtime_error("input socket path too long");
        // No private key or input-enabled default exists in this component.
    }

    void sync_keymap() {
        const auto keyboard = g_pSeatManager->m_keyboard.lock();
        if (!keyboard || keyboard->m_xkbKeymapV1FD.get() < 0 || keyboard->m_xkbKeymapV1String.empty() ||
            (keyboard_state && keymap_text == keyboard->m_xkbKeymapV1String)) return;
        // Prepare a complete replacement before retiring the old independent
        // state. Keep our own fd: primary keyboard replacement must not leave
        // later seat bindings referring to a closed compositor fd.
        auto* context = xkb_context_new(XKB_CONTEXT_NO_FLAGS);
        auto* map = context ? xkb_keymap_new_from_string(context, keyboard->m_xkbKeymapV1String.c_str(),
            XKB_KEYMAP_FORMAT_TEXT_V1, XKB_KEYMAP_COMPILE_NO_FLAGS) : nullptr;
        auto* state = map ? xkb_state_new(map) : nullptr;
        const auto fd = fcntl(keyboard->m_xkbKeymapV1FD.get(), F_DUPFD_CLOEXEC, 0);
        if (!state || fd < 0) {
            if (fd >= 0) close(fd);
            if (state) xkb_state_unref(state);
            if (map) xkb_keymap_unref(map);
            if (context) xkb_context_unref(context);
            throw std::runtime_error("independent XKB state unavailable");
        }
        desktop_transition();
        if (keyboard_state) xkb_state_unref(keyboard_state);
        if (keymap) xkb_keymap_unref(keymap);
        if (xkb_context_) xkb_context_unref(xkb_context_);
        if (keymap_fd >= 0) close(keymap_fd);
        keyboard_state = state; keymap = map; xkb_context_ = context; keymap_fd = fd;
        keymap_text = keyboard->m_xkbKeymapV1String;
        for (auto& k : keyboards)
            if (!k->dead && k->wl->resource())
                k->wl->sendKeymap(WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1, keymap_fd, keymap_text.size() + 1);
        for (auto& seat : seats)
            if (!seat->dead && seat->wl->resource())
                seat->wl->sendCapabilities(static_cast<wl_seat_capability>(WL_SEAT_CAPABILITY_POINTER | WL_SEAT_CAPABILITY_KEYBOARD));
    }
    void start() {
        sync_keymap();
        timer = wl_event_loop_add_timer(g_pCompositor->m_wlEventLoop, tick, this);
        if (!timer) throw std::runtime_error("input timer registration failed");
        global = wl_global_create(g_pCompositor->m_wlDisplay, &wl_seat_interface, 9, this, bind_seat);
        if (!global) throw std::runtime_error("synthetic seat unavailable");
        wl_event_source_timer_update(timer, 16);
    }
    void resume() {
        if (retired || desktop_generation == UINT64_MAX)
            throw std::runtime_error("restart desktop before replacing input seats");
        if (!suspended) return;
        sync_keymap();
        // Every new admission period has fresh identity and no inherited grant.
        epoch = nonce();
        listener = socket(AF_UNIX, SOCK_SEQPACKET | SOCK_NONBLOCK | SOCK_CLOEXEC, 0);
        if (listener < 0) throw std::runtime_error("input socket unavailable");
        sockaddr_un address{};
        address.sun_family = AF_UNIX;
        std::memcpy(address.sun_path, path.c_str(), path.size() + 1);
        // Never unlink a pre-existing socket owned by another plugin instance.
        if (bind(listener, reinterpret_cast<sockaddr*>(&address), sizeof(address)) != 0)
            throw std::runtime_error("input socket bind refused");
        if (!socket_path.capture(path))
            throw std::runtime_error("input socket ownership unavailable");
        socket_cleanup = "bound";
        if (chmod(path.c_str(), 0600) != 0 || listen(listener, 8) != 0)
            throw std::runtime_error("input socket setup failed");
        listen_source = wl_event_loop_add_fd(g_pCompositor->m_wlEventLoop, listener, WL_EVENT_READABLE, accept_ready, this);
        if (!listen_source) throw std::runtime_error("input event loop registration failed");
        suspended = false;
    }
    void cleanup_socket() {
        using Result = OwnedSocketPath::CleanupResult;
        switch (socket_path.cleanup()) {
            case Result::NotCaptured: break;
            case Result::Removed: socket_cleanup = "removed"; break;
            case Result::AlreadyAbsent: socket_cleanup = "absent"; break;
            case Result::Replaced: socket_cleanup = "replacement_preserved"; break;
            case Result::Failed: socket_cleanup = "failed"; break;
        }
    }
    void suspend(std::string_view reason = "plugin_disabled") {
        suspended = true;
        revoke(reason);
        reservation = nullptr;
        if (listen_source) wl_event_source_remove(listen_source);
        listen_source = nullptr;
        clients.clear();
        if (listener >= 0) close(listener);
        listener = -1;
        cleanup_socket();
        // Keep the globals, capabilities, and client resources stable. Removing
        // and recreating them makes existing apps lose their agent input path.
    }
    void retire() {
        // wl_global removal does not revoke existing protocol objects. Destroying
        // those objects immediately disconnects clients that still send release
        // or cursor requests (observed with foot). Retain inert resources and
        // callbacks until compositor exit; the experimental module is NODELETE.
        suspend("plugin_shutdown");
        retired = true;
        for (auto& seat : seats)
            if (!seat->dead && seat->wl->resource())
                seat->wl->sendCapabilities(static_cast<wl_seat_capability>(0));
        if (global) wl_global_remove(global);
    }
    ~Impl() {
        revoke("plugin_shutdown");
        if (global) wl_global_destroy(global);
        if (listen_source) wl_event_source_remove(listen_source);
        if (timer) wl_event_source_remove(timer);
        clients.clear();
        // Destroy all plugin-owned resources before unloading code callbacks.
        pointers.clear(); keyboards.clear(); touches.clear(); seats.clear();
        if (listener >= 0) close(listener);
        cleanup_socket();
        if (keyboard_state) xkb_state_unref(keyboard_state);
        if (keymap) xkb_keymap_unref(keymap);
        if (xkb_context_) xkb_context_unref(xkb_context_);
        if (keymap_fd >= 0) close(keymap_fd);
    }
    std::uint32_t serial() const { return wl_display_next_serial(g_pCompositor->m_wlDisplay); }
    bool available() const {
        return !retired && !suspended && g_pCompositor->m_sessionActive && g_pCompositor->m_dpmsStateOn &&
            !g_pCompositor->m_isShuttingDown && !g_pSessionLockManager->isSessionLocked();
    }
    static void bind_seat(wl_client* client, void* data, std::uint32_t version, std::uint32_t id) {
        auto& self = *static_cast<Impl*>(data);
        if (self.seats.size() >= kMaxResources) { wl_client_post_no_memory(client); return; }
        auto seat = std::make_unique<Seat>();
        auto* entry = seat.get();
        seat->wl = makeShared<CWlSeat>(client, std::min(version, 9u), id);
        if (!seat->wl->resource()) { wl_client_post_no_memory(client); return; }
        // Stock resource lookup casts CWlSeat::data to CWLSeatResource. Null is
        // intentional: an agent serial must not authorize primary-seat WM grabs.
        seat->wl->setData(nullptr);
        seat->wl->setRelease([entry](CWlSeat*) { entry->dead = true; });
        seat->wl->setOnDestroy([entry](CWlSeat*) { entry->dead = true; });
        seat->wl->setGetPointer([&self](CWlSeat* r, std::uint32_t child) { self.add_pointer(r, child); });
        seat->wl->setGetKeyboard([&self](CWlSeat* r, std::uint32_t child) { self.add_keyboard(r, child); });
        seat->wl->setGetTouch([&self](CWlSeat* r, std::uint32_t child) { self.add_touch(r, child); });
        if (version >= 2) seat->wl->sendName(self.lane == 0 ? "Cua-Test-Agent" : "Cua-Test-Agent-2");
        seat->wl->sendCapabilities(static_cast<wl_seat_capability>(self.retired ? 0 :
            WL_SEAT_CAPABILITY_POINTER | (self.keyboard_state ? WL_SEAT_CAPABILITY_KEYBOARD : 0)));
        self.seats.push_back(std::move(seat));
    }
    void add_pointer(CWlSeat* seat, std::uint32_t id) {
        if (pointers.size() >= kMaxResources) { seat->noMemory(); return; }
        auto p = std::make_unique<Pointer>(); auto* entry = p.get();
        p->wl = makeShared<CWlPointer>(seat->client(), seat->version(), id);
        if (!p->wl->resource()) { seat->noMemory(); return; }
        p->wl->setData(nullptr);
        p->wl->setRelease([entry](CWlPointer*) { entry->dead = true; });
        p->wl->setOnDestroy([entry](CWlPointer*) { entry->dead = true; });
        p->wl->setSetCursor([](CWlPointer*, std::uint32_t, wl_resource*, std::int32_t, std::int32_t) {});
        pointers.push_back(std::move(p));
    }
    void add_keyboard(CWlSeat* seat, std::uint32_t id) {
        if (!keyboard_state || keyboards.size() >= kMaxResources) { seat->noMemory(); return; }
        auto k = std::make_unique<Keyboard>(); auto* entry = k.get();
        k->wl = makeShared<CWlKeyboard>(seat->client(), seat->version(), id);
        if (!k->wl->resource()) { seat->noMemory(); return; }
        k->wl->setData(nullptr);
        k->wl->setRelease([entry](CWlKeyboard*) { entry->dead = true; });
        k->wl->setOnDestroy([entry](CWlKeyboard*) { entry->dead = true; });
        k->wl->sendKeymap(WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1, keymap_fd, keymap_text.size() + 1);
        if (seat->version() >= 4) k->wl->sendRepeatInfo(0, 0);
        keyboards.push_back(std::move(k));
    }
    void add_touch(CWlSeat* seat, std::uint32_t id) {
        // Never advertised. A valid inert resource is safer than a dangling id.
        if (touches.size() >= kMaxResources) { seat->noMemory(); return; }
        auto t = std::make_unique<Touch>(); auto* entry = t.get();
        t->wl = makeShared<CWlTouch>(seat->client(), seat->version(), id);
        if (!t->wl->resource()) { seat->noMemory(); return; }
        t->wl->setData(nullptr);
        t->wl->setRelease([entry](CWlTouch*) { entry->dead = true; });
        t->wl->setOnDestroy([entry](CWlTouch*) { entry->dead = true; });
        touches.push_back(std::move(t));
    }
    bool refresh(Client& c) {
        const auto window = c.window.lock(); const auto surface = c.surface.lock();
        if (!window || !surface || !window->m_isMapped || window->isHidden() ||
            window->m_isX11 || window->resource() != surface || !surface->m_mapped || !surface->good()) return false;
        // Match Driver's captured client surface, not the decorated window box
        // (which includes compositor borders and shifts clicks by their width).
        const auto box = window->surfaceLogicalBox(); const auto surface_box = box;
        if (!box || !surface_box || box->w <= 0 || box->h <= 0 || box->w > 32767 || box->h > 32767) return false;
        std::array<double, 6> geometry{box->x, box->y, box->w, box->h, surface_box->x, surface_box->y};
        if (geometry != c.geometry) { c.geometry = geometry; ++c.revision; }
        return true;
    }
    bool primary_conflict(Client& c) const {
        const auto surface = c.surface.lock(); if (!surface) return true;
        const auto pointer = g_pSeatManager->m_state.pointerFocus.lock();
        const auto keyboard = g_pSeatManager->m_state.keyboardFocus.lock();
        // Conservative per-client refusal avoids toolkit-global cross-window state.
        return (pointer && pointer->client() == surface->client()) ||
            (keyboard && keyboard->client() == surface->client());
    }
    bool agent_conflict(Client& c) const {
        const auto surface = c.surface.lock();
        if (!surface) return true;
        for (const auto* peer : peers) {
            if (!peer || peer == this || !peer->lease) continue;
            const auto other = peer->lease->surface.lock();
            if (other && other->client() == surface->client()) return true;
        }
        return false;
    }
    void invalidate(Client& c) {
        if (lease == &c) revoke("stale_target");
        c.token.clear(); c.window.reset(); c.surface.reset();
        // Replay high-water belongs to the old target binding. A genuinely
        // new token may receive a shorter grant after Stop or target change.
        c.approved_deadline = 0;
    }
    void leave() {
        for (auto& p : pointers) {
            const auto surface = p->focus.lock();
            if (!p->dead && p->wl->resource() && surface && surface->good()) {
                if (held_button) p->wl->sendButton(serial(), event_ms(), held_button, WL_POINTER_BUTTON_STATE_RELEASED);
                p->wl->sendLeave(serial(), surface->getResource().get());
                if (p->wl->version() >= 5) p->wl->sendFrame();
            }
            p->focus.reset();
        }
        held_button = 0;
        for (auto& k : keyboards) {
            const auto surface = k->focus.lock();
            if (!k->dead && k->wl->resource() && surface && surface->good()) {
                for (auto key : held_keys) k->wl->sendKey(serial(), event_ms(), key, WL_KEYBOARD_KEY_STATE_RELEASED);
                k->wl->sendModifiers(serial(), 0, 0, 0, 0);
                k->wl->sendLeave(serial(), surface->getResource().get());
            }
            k->focus.reset();
        }
        held_keys.clear();
        if (keyboard_state) xkb_state_unref(keyboard_state);
        keyboard_state = keymap ? xkb_state_new(keymap) : nullptr;
    }
    void revoke(std::string_view reason) {
        if (lease && trace) trace->mark("agent_cancel", lane + 1);
        if (drag && drag->client && !drag->client->dead) send(*drag->client, refusal(reason));
        drag.reset(); leave(); lease = nullptr; capabilities = 0;
    }
    void cancel_authority(std::string_view reason) {
        revoke(reason);
        // Stop is also a boundary for grants that have not been redeemed yet,
        // including a signed renewal with a later deadline. Invalidating every
        // target makes those signatures unusable without closing operator or
        // trace channels. Action connections keep their lane reservation, but
        // must select a fresh target token and obtain new operator approval.
        for (auto& c : clients) invalidate(*c);
    }
    void desktop_transition() {
        // Signals can fire before Hyprland updates its aggregate state. Revoke
        // unconditionally; an off/on pair between timer ticks must not revive
        // authority. Kill pending connections as well as the active lease so
        // a pre-transition signed grant cannot be approved after unlock.
        revoke("desktop_changed");
        for (auto& c : clients) {
            // Observers and operator-control connections own no action target.
            // Keep them alive so Stop/status/evidence survives a transition.
            if (reservation != c.get() && c->token.empty()) continue;
            c->dead = true;
            if (c->source) wl_event_source_remove(c->source);
            c->source = nullptr;
            if (c->fd >= 0) close(c->fd);
            c->fd = -1;
        }
        reservation = nullptr;
        if (desktop_generation == UINT64_MAX)
            suspended = true;
        else
            ++desktop_generation;
    }
    void send(Client& c, const std::string& packet) {
        if (c.dead) return;
        const auto n = ::send(c.fd, packet.data(), packet.size(), MSG_DONTWAIT | MSG_NOSIGNAL);
        if (n < 0 || static_cast<std::size_t>(n) != packet.size()) c.dead = true;
    }
    static int accept_ready(int, std::uint32_t, void* data) {
        auto& self = *static_cast<Impl*>(data);
        for (unsigned i = 0; i < 8; ++i) {
            const auto fd = accept4(self.listener, nullptr, nullptr, SOCK_NONBLOCK | SOCK_CLOEXEC);
            if (fd < 0) break;
            ucred credentials{}; socklen_t size = sizeof(credentials);
            if (getsockopt(fd, SOL_SOCKET, SO_PEERCRED, &credentials, &size) != 0 ||
                size != sizeof(credentials) || credentials.uid != getuid() || self.clients.size() >= kMaxClients) { close(fd); continue; }
            bool owned = false;
            try {
                auto c = std::make_unique<Client>(); c->owner = &self; c->fd = fd; owned = true; c->challenge = nonce();
                c->source = wl_event_loop_add_fd(g_pCompositor->m_wlEventLoop, fd, WL_EVENT_READABLE, client_ready, c.get());
                if (!c->source) continue;
                self.clients.push_back(std::move(c));
            } catch (...) { if (!owned) close(fd); }
        }
        return 0;
    }
    static int client_ready(int fd, std::uint32_t mask, void* data) {
        auto& c = *static_cast<Client*>(data); auto& self = *c.owner;
        if (mask & (WL_EVENT_HANGUP | WL_EVENT_ERROR)) c.dead = true;
        for (unsigned i = 0; i < 8 && !c.dead; ++i) {
            std::array<char, kMaxPacket> buffer{};
            const auto n = recv(fd, buffer.data(), buffer.size(), MSG_DONTWAIT | MSG_TRUNC);
            if (n < 0) { if (errno != EAGAIN && errno != EWOULDBLOCK && errno != EINTR) c.dead = true; break; }
            if (!n) { c.dead = true; break; }
            c.activity = Clock::now();
            if (static_cast<std::size_t>(n) > buffer.size()) { self.send(c, refusal("invalid_request")); continue; }
            try { self.request(c, fields(std::string_view(buffer.data(), n))); }
            catch (...) { self.send(c, refusal("invalid_request")); }
        }
        if (c.dead) {
            if (self.lease == &c) self.revoke("disconnected");
            if (self.reservation == &c) self.reservation = nullptr;
        }
        return 0;
    }
    bool valid_signature(const std::string& message, const std::string& signature) const {
        if (signature.size() != 128) return false;
        const auto bytes = unhex(signature);
        auto* key = EVP_PKEY_new_raw_public_key(EVP_PKEY_ED25519, nullptr, public_key.data(), public_key.size());
        auto* ctx = EVP_MD_CTX_new();
        const bool ok = key && ctx && EVP_DigestVerifyInit(ctx, nullptr, nullptr, nullptr, key) == 1 &&
            EVP_DigestVerify(ctx, bytes.data(), bytes.size(), reinterpret_cast<const unsigned char*>(message.data()), message.size()) == 1;
        EVP_MD_CTX_free(ctx); EVP_PKEY_free(key); return ok;
    }
    void approve(Client& c, const std::vector<std::string>& f) {
        if (f.size() != 6) { send(c, refusal("invalid_grant")); return; }
        const auto deadline = number(f[3]); const auto caps = number(f[4]); const auto now = unix_ms();
        if (deadline <= now || deadline - now > 60000 || caps == 0 || caps > 15) { send(c, refusal("invalid_grant")); return; }
        const auto message = std::format("CUA_TEST_LEASE_1\n{}\n{}\n{}\n{}\n{}\n", epoch, f[1], f[2], deadline, caps);
        if (!valid_signature(message, f[5])) { send(c, refusal("invalid_grant")); return; }
        const auto found = std::ranges::find_if(clients, [&](auto& peer) {
            return !peer->dead && peer->hello && peer->challenge == f[1] && peer->token == f[2] && !peer->token.empty();
        });
        if (found == clients.end() || !refresh(**found)) { send(c, refusal("stale_target")); return; }
        if (reservation != found->get()) { send(c, refusal("lane_not_claimed")); return; }
        if (lease) { send(c, refusal("lease_busy")); return; }
        if (agent_conflict(**found)) { send(c, refusal("agent_target_busy")); return; }
        if (deadline <= (*found)->approved_deadline) { send(c, refusal("invalid_grant")); return; }
        if (!available()) { send(c, refusal("session_unavailable")); return; }
        // Bind steady-clock lifetime once; a clock change cannot extend a lease.
        lease = found->get(); capabilities = caps; expires = Clock::now() + std::chrono::milliseconds(deadline - now);
        lease->approved_deadline = deadline;
        // Approval begins the bounded active period. Time spent waiting for
        // the external operator must not consume the input connection's idle
        // budget while its newly approved lease is still valid.
        lease->activity = Clock::now();
        if (trace) trace->mark("agent_approved", lane + 1);
        send(c, R"({"ok":true})");
    }
    bool point(Client& c, double x, double y) const {
        return std::isfinite(x) && std::isfinite(y) && x >= 0 && y >= 0 && x < c.geometry[2] && y < c.geometry[3];
    }
    bool pointer_enter(Client& c, double x, double y) {
        const auto root = c.surface.lock(); if (!root) return false;
        // Initial experiment refuses subsurface targets instead of misrouting.
        const Vector2D local{x + c.geometry[0] - c.geometry[4], y + c.geometry[1] - c.geometry[5]};
        const auto hit = root->at(local, true);
        if (hit.first != root) return false;
        unsigned count = 0;
        for (auto& p : pointers) {
            if (p->dead || !p->wl->resource() || p->wl->client() != root->client()) continue;
            if (p->focus != root) {
                if (const auto old = p->focus.lock(); old && old->good()) p->wl->sendLeave(serial(), old->getResource().get());
                p->focus = root;
                p->wl->sendEnter(serial(), root->getResource().get(), wl_fixed_from_double(local.x), wl_fixed_from_double(local.y));
            }
            p->wl->sendMotion(event_ms(), wl_fixed_from_double(local.x), wl_fixed_from_double(local.y));
            if (p->wl->version() >= 5) p->wl->sendFrame();
            ++count;
        }
        return count > 0;
    }
    void button(std::uint32_t value, bool pressed) {
        for (auto& p : pointers) {
            if (p->dead || !p->wl->resource() || !p->focus) continue;
            p->wl->sendButton(serial(), event_ms(), value, pressed ? WL_POINTER_BUTTON_STATE_PRESSED : WL_POINTER_BUTTON_STATE_RELEASED);
            if (p->wl->version() >= 5) p->wl->sendFrame();
        }
        held_button = pressed ? value : 0;
    }
    bool keyboard_enter(Client& c) {
        const auto root = c.surface.lock(); const auto physical = g_pSeatManager->m_keyboard.lock();
        if (!root || !physical || physical->m_xkbKeymapV1String != keymap_text || !keyboard_state) return false;
        unsigned count = 0;
        for (auto& k : keyboards) {
            if (k->dead || !k->wl->resource() || k->wl->client() != root->client()) continue;
            if (k->focus != root) {
                if (const auto old = k->focus.lock(); old && old->good()) k->wl->sendLeave(serial(), old->getResource().get());
                k->focus = root;
                wl_array keys{};
                k->wl->sendEnter(serial(), root->getResource().get(), &keys);
                k->wl->sendModifiers(serial(), 0, 0, 0, 0);
            }
            ++count;
        }
        return count > 0;
    }
    void key(std::uint32_t code, bool pressed) {
        xkb_state_update_key(keyboard_state, code + 8, pressed ? XKB_KEY_DOWN : XKB_KEY_UP);
        if (pressed) held_keys.push_back(code); else std::erase(held_keys, code);
        for (auto& k : keyboards) {
            if (k->dead || !k->wl->resource() || !k->focus) continue;
            k->wl->sendKey(serial(), event_ms(), code, pressed ? WL_KEYBOARD_KEY_STATE_PRESSED : WL_KEYBOARD_KEY_STATE_RELEASED);
            k->wl->sendModifiers(serial(), xkb_state_serialize_mods(keyboard_state, XKB_STATE_MODS_DEPRESSED),
                xkb_state_serialize_mods(keyboard_state, XKB_STATE_MODS_LATCHED),
                xkb_state_serialize_mods(keyboard_state, XKB_STATE_MODS_LOCKED),
                xkb_state_serialize_layout(keyboard_state, XKB_STATE_LAYOUT_EFFECTIVE));
        }
    }
    void request(Client& c, const std::vector<std::string>& f) {
        sync_keymap();
        if (c.dead) return;
        const auto& command = f[0];
        if (command == "HELLO") {
            if (f.size() != 1 || c.hello) { send(c, refusal("invalid_request")); return; }
            c.hello = true;
            send(c, std::format(R"({{"ok":true,"protocol":0,"epoch":"{}","challenge":"{}"}})", epoch, c.challenge)); return;
        }
        if (!c.hello) { send(c, refusal("invalid_request")); return; }
        if (command == "CLAIM" && f.size() == 1) {
            if (reservation && reservation != &c) { send(c, refusal("lane_busy")); return; }
            reservation = &c;
            send(c, std::format(R"({{"ok":true,"lane":{}}})", lane)); return;
        }
        if (command == "STOP" && f.size() == 1) {
            for (auto* peer : peers) if (peer) peer->cancel_authority("stopped");
            send(c, R"({"ok":true})"); return;
        }
        if (command == "CANCEL" && f.size() == 1) { cancel_authority("cancelled"); send(c, R"({"ok":true})"); return; }
        if (trace && ((command == "TRACE_START" || command == "TRACE_STOP") && f.size() == 1)) {
            send(c, trace->request(command)); return;
        }
        if (trace && command == "TRACE_READ" && f.size() == 2) {
            const auto after = number(f[1]);
            if (after > 32768) { send(c, refusal("invalid_request")); return; }
            send(c, trace->request(command, after)); return;
        }
        if (command == "APPROVE") { approve(c, f); return; }
        if (reservation != &c) { send(c, refusal("lane_not_claimed")); return; }
        if (command == "TARGET") {
            if (f.size() != 3 || drag) { send(c, refusal("invalid_request")); return; }
            const auto pid = number(f[1]); const auto address = number(f[2], 16);
            PHLWINDOW window;
            for (const auto& w : Desktop::windowState()->windows())
                if (reinterpret_cast<std::uintptr_t>(w.get()) == address && static_cast<std::uint64_t>(w->getPID()) == pid) window = w;
            if (!window || window->m_isX11 || !window->m_isMapped || window->isHidden() || !window->resource()) {
                send(c, refusal("stale_target")); return;
            }
            if (c.window != window || c.surface != window->resource() || c.token.empty()) {
                invalidate(c); c.unmap.reset(); c.destroy.reset();
                c.window = window; c.surface = window->resource(); c.token = nonce(); c.revision = 1;
                c.unmap = window->m_events.unmap.listen([this, &c] { invalidate(c); });
                c.destroy = window->m_events.destroy.listen([this, &c] { invalidate(c); });
            }
            if (!refresh(c)) { invalidate(c); send(c, refusal("stale_target")); return; }
            send(c, std::format(R"({{"ok":true,"target":"{}","revision":{},"width":{},"height":{}}})", c.token, c.revision, c.geometry[2], c.geometry[3])); return;
        }
        const std::uint64_t cap = command == "CLICK" ? 1 : command == "KEY" ? 2 : command == "SCROLL" ? 4 : command == "DRAG" ? 8 : 0;
        const std::size_t count = command == "CLICK" ? 8 : command == "KEY" ? 6 : command == "SCROLL" ? 8 : 9;
        if (!cap) { send(c, refusal("unsupported")); return; }
        if (f.size() != count) { send(c, refusal("invalid_request")); return; }
        const auto sequence = number(f[1]);
        if (sequence <= c.sequence) { send(c, refusal("replay")); return; }
        c.sequence = sequence;
        if (c.token.empty() || f[2] != c.token || !refresh(c)) { send(c, refusal("stale_target")); return; }
        if (number(f[3]) != c.revision) { send(c, refusal("stale_geometry")); return; }
        if (!available()) { revoke("session_unavailable"); send(c, refusal("session_unavailable")); return; }
        if (lease && Clock::now() >= expires) revoke("lease_expired");
        if (lease != &c || !(capabilities & cap)) {
            send(c, std::format(R"({{"ok":false,"code":"pending_operator_approval","detail":"external test operator approval required","epoch":"{}","challenge":"{}","target":"{}"}})", epoch, c.challenge, c.token)); return;
        }
        if (drag) { send(c, refusal("lease_busy")); return; }
        if (primary_conflict(c)) { send(c, refusal("primary_target_busy")); return; }
        if (agent_conflict(c)) { send(c, refusal("agent_target_busy")); return; }
        if (command == "KEY") {
            const auto code = number(f[4]); const auto mods = number(f[5]);
            if (code == 0 || code > 247 || mods > 15 || code == 58 || code == 69 || code == 70) { send(c, refusal("unsupported")); return; }
            if (!keyboard_enter(c)) { send(c, refusal("client_not_bound")); return; }
            const std::array<std::uint32_t, 4> keys{42, 29, 56, 125};
            for (unsigned i = 0; i < 4; ++i) if ((mods & (1u << i)) && keys[i] != code) key(keys[i], true);
            key(code, true); key(code, false);
            for (int i = 3; i >= 0; --i) if ((mods & (1u << i)) && keys[i] != code) key(keys[i], false);
        } else {
            const auto x = real(f[4]), y = real(f[5]);
            if (!point(c, x, y)) { send(c, refusal("invalid_request")); return; }
            if (command == "CLICK") {
                const auto btn = number(f[6]), clicks = number(f[7]);
                if (btn < 272 || btn > 274 || clicks < 1 || clicks > 2) { send(c, refusal("invalid_request")); return; }
                if (!pointer_enter(c, x, y)) { send(c, refusal("client_not_bound")); return; }
                for (unsigned i = 0; i < clicks; ++i) { button(btn, true); button(btn, false); }
            } else if (command == "SCROLL") {
                const auto axis = number(f[6]); const auto value = real(f[7]);
                if (axis > 1 || value == 0 || std::abs(value) > 1000) { send(c, refusal("invalid_request")); return; }
                if (!pointer_enter(c, x, y)) { send(c, refusal("client_not_bound")); return; }
                for (auto& p : pointers) {
                    if (p->dead || !p->wl->resource() || !p->focus) continue;
                    if (p->wl->version() >= 5) p->wl->sendAxisSource(WL_POINTER_AXIS_SOURCE_WHEEL);
                    p->wl->sendAxis(event_ms(), static_cast<wl_pointer_axis>(axis), wl_fixed_from_double(value));
                    if (p->wl->version() >= 5) p->wl->sendFrame();
                }
            } else {
                const auto x2 = real(f[6]), y2 = real(f[7]); const auto duration = number(f[8]);
                if (!point(c, x2, y2) || duration < 50 || duration > 2000) { send(c, refusal("invalid_request")); return; }
                if (Clock::now() + std::chrono::milliseconds(duration + 50) >= expires) { send(c, refusal("lease_expired")); return; }
                if (!pointer_enter(c, x, y)) { send(c, refusal("client_not_bound")); return; }
                if (trace) trace->mark("agent_drag_start", lane + 1);
                button(272, true);
                drag.emplace(Drag{&c, x, y, x2, y2, Clock::now(), static_cast<unsigned>(duration), DragGeometry{c.revision}});
                send(c, R"({"ok":true,"phase":"started"})"); return;
            }
        }
        ++dispatches;
        if (trace) trace->mark("agent_action_end", lane + 1);
        send(c, kDelivered);
    }
    static int tick(void* data) {
        auto& self = *static_cast<Impl*>(data);
        try { self.step(); } catch (...) { self.revoke("internal_error"); }
        wl_event_source_timer_update(self.timer, self.retired ? 500 : 16); return 0;
    }
    void step() {
        if (!retired) sync_keymap();
        if (lease) {
            if (Clock::now() >= expires) revoke("lease_expired");
            else if (lease->dead || !available() || !refresh(*lease) || primary_conflict(*lease) || agent_conflict(*lease) ||
                (drag && !drag->geometry.matches(lease->revision))) revoke("cancelled");
        }
        if (drag) {
            const auto d = *drag;
            const auto elapsed = std::chrono::duration<double, std::milli>(Clock::now() - d.start).count();
            const auto progress = std::min(elapsed / d.duration, 1.0);
            if (!pointer_enter(*d.client, d.x1 + (d.x2 - d.x1) * progress, d.y1 + (d.y2 - d.y1) * progress)) revoke("client_not_bound");
            else if (progress >= 1) {
                button(272, false); drag.reset(); ++dispatches;
                if (trace) trace->mark("agent_drag_end", lane + 1);
                send(*d.client, kDelivered);
            }
        }
        for (auto& c : clients) if (Clock::now() - c->activity > std::chrono::seconds(c->hello ? 60 : 5)) c->dead = true;
        if (lease && lease->dead) revoke("disconnected");
        if (reservation && reservation->dead) reservation = nullptr;
        std::erase_if(clients, [](auto& c) { return c->dead; });
        std::erase_if(pointers, [](auto& p) { return p->dead; });
        std::erase_if(keyboards, [](auto& k) { return k->dead; });
        std::erase_if(touches, [](auto& t) { return t->dead; });
        std::erase_if(seats, [](auto& s) { return s->dead; });
    }
};

// Hyprland 0.56.2 signals run synchronously on the compositor thread. Keep
// their ownership explicit and detach before retiring the seats. No polling
// or aggregate-state comparison substitutes for this transition boundary.
struct InputExperiment::DesktopListeners {
    struct MonitorListeners {
        PHLMONITORREF monitor;
        CHyprSignalListener dpms, mode;
    };
    InputExperiment& owner;
    std::vector<MonitorListeners> monitors;
    CHyprSignalListener lock, unlock, active, layout, added, removed, destroyed;

    explicit DesktopListeners(InputExperiment& input) : owner(input) {
        lock = g_pSessionLockManager->m_events.lock.listen([this] { changed(); });
        unlock = g_pSessionLockManager->m_events.unlock.listen([this] { changed(); });
        if (g_pCompositor->m_aqBackend->hasSession())
            active = g_pCompositor->m_aqBackend->session->events.changeActive.listen([this] { changed(); });
        layout = Event::bus()->m_events.monitor.layoutChanged.listen([this] { changed(); });
        added = Event::bus()->m_events.monitor.preAdded.listen([this](PHLMONITOR monitor) {
            changed(); watch(monitor);
        });
        removed = Event::bus()->m_events.monitor.preRemoved.listen([this](PHLMONITOR) { changed(); });
        destroyed = Event::bus()->m_events.monitor.destroyMon.listen([this](PHLMONITOR monitor) {
            changed();
            std::erase_if(monitors, [&](const auto& entry) { return !entry.monitor || entry.monitor == monitor; });
        });
        for (const auto& monitor : State::monitorState()->allMonitors()) watch(monitor);
    }
    void changed() {
        for (auto& lane : owner.lanes_) lane->desktop_transition();
    }
    void watch(PHLMONITOR monitor) {
        if (!monitor || std::ranges::any_of(monitors, [&](const auto& entry) { return entry.monitor == monitor; })) return;
        MonitorListeners listeners;
        listeners.monitor = monitor;
        listeners.dpms = monitor->m_events.dpmsChanged.listen([this] { changed(); });
        listeners.mode = monitor->m_events.modeChanged.listen([this] { changed(); });
        monitors.push_back(std::move(listeners));
    }
};

InputExperiment::InputExperiment(const std::string& directory, void* plugin) {
    SeatLifetime lifetime(directory);
    for (unsigned i = 0; i < lanes_.size(); ++i) lanes_[i] = std::make_unique<Impl>(directory, i);
    for (auto& lane : lanes_) { lane->peers = {lanes_[0].get(), lanes_[1].get()}; lane->start(); }
    trace_ = std::make_unique<PrimaryTrace>(plugin, [this](wl_resource* resource) {
        for (unsigned i = 0; i < lanes_.size(); ++i) {
            for (const auto& p : lanes_[i]->pointers) if (p->wl->resource() == resource) return i + 1;
            for (const auto& k : lanes_[i]->keyboards) if (k->wl->resource() == resource) return i + 1;
        }
        return 0u;
    });
    for (auto& lane : lanes_) lane->trace = trace_.get();
    desktop_listeners_ = std::make_unique<DesktopListeners>(*this);
    lifetime.publish();
}
InputExperiment::~InputExperiment() {
    desktop_listeners_.reset();
    for (auto& lane : lanes_) { lane->retire(); lane->trace = nullptr; lane->peers = {}; }
    trace_.reset();
    // Intentional process-lifetime ownership: callbacks, removed global, and
    // remaining client-owned resources cannot outlive their Impl. The instance
    // marker refuses replacement modules, so this retains at most two lanes.
    for (auto& lane : lanes_) (void)lane.release();
}
void InputExperiment::suspend() {
    for (auto& lane : lanes_) lane->suspend();
}
void InputExperiment::resume() {
    try {
        for (auto& lane : lanes_) lane->resume();
    } catch (...) {
        // Partial transport setup must not leave one admitted lane behind.
        suspend();
        throw;
    }
}
std::string InputExperiment::status_json() const {
    std::string states;
    for (const auto& lane : lanes_) {
        if (!states.empty()) states += ',';
        const bool pointer_focus = std::ranges::any_of(lane->pointers, [](const auto& p) { return !p->dead && bool(p->focus); });
        const bool keyboard_focus = std::ranges::any_of(lane->keyboards, [](const auto& k) { return !k->dead && bool(k->focus); });
        states += std::format(R"({{"lane":{},"epoch":"{}","desktop_generation":{},"reserved":{},"socket_cleanup":"{}","lease_active":{},"seat_resources":{},"pointer_resources":{},"keyboard_resources":{},"dispatches":{},"held_button":{},"held_keys":{},"drag_active":{},"pointer_focus":{},"keyboard_focus":{}}})",
            lane->lane, lane->epoch, lane->desktop_generation, lane->reservation != nullptr, lane->socket_cleanup, lane->lease != nullptr, lane->seats.size(), lane->pointers.size(), lane->keyboards.size(), lane->dispatches,
            lane->held_button, lane->held_keys.size(), lane->drag.has_value(), pointer_focus, keyboard_focus);
    }
    // Aggregate legacy fields remain available to existing test probes.
    return std::format(R"({{"protocol":0,"test_only":true,"seat_lifetime":"compositor","upgrade":"desktop_restart","transport_ready":{},"epoch":"{}","lease_active":{},"seat_resources":{},"pointer_resources":{},"keyboard_resources":{},"dispatches":{},"lanes":[{}]}})",
        !lanes_[0]->suspended && !lanes_[1]->suspended, lanes_[0]->epoch, lanes_[0]->lease != nullptr || lanes_[1]->lease != nullptr,
        lanes_[0]->seats.size() + lanes_[1]->seats.size(), lanes_[0]->pointers.size() + lanes_[1]->pointers.size(),
        lanes_[0]->keyboards.size() + lanes_[1]->keyboards.size(), lanes_[0]->dispatches + lanes_[1]->dispatches, states);
}
} // namespace cua::hyprland
