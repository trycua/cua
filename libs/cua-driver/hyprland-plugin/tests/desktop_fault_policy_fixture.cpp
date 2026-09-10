// Used by desktop_fault_policy_test.py, which inserts actual production bodies.
// Transport effects are counted here; native protocol/app behavior is separate.
#include "drag_geometry.hpp"
#include "input_grant.hpp"
#include "passive_pointer_target.hpp"

#include <algorithm>
#include <cstdlib>
#include <iostream>
#include <memory>
#include <optional>
#include <string>
#include <vector>

using namespace cua::hyprland;
using Clock = InputGrant::Clock;
struct Window {};
struct Surface { int client() const { return 1; } };
using Target = PassivePointerTarget<std::weak_ptr<Window>, std::weak_ptr<Surface>>;
struct Client {
    bool dead = false;
    void* source = nullptr;
    int fd = -1;
    std::string token = "old-target";
    std::weak_ptr<Window> window;
    std::weak_ptr<Surface> surface;
    uint64_t approved_deadline = 42, revision = 1;
};
struct Pointer {
    bool dead = false, resource_live = true;
    Pointer* wl = this;
    std::shared_ptr<Surface> focus;
    bool resource() const { return resource_live; }
};
struct Drag { Client* client; DragGeometry geometry{1}; };
struct Trace { void mark(const char*, unsigned) {} };
enum class ForegroundFailureReason { none };
struct ForegroundFailure { ForegroundFailureReason reason; };
void wl_event_source_remove(void*) {}
int close(int) { return 0; }
std::string refusal(std::string_view reason) { return std::string(reason); }
std::string refusal(std::string_view reason, ForegroundFailure) { return refusal(reason); }
void check(bool value, const char* message) {
    if (!value) { std::cerr << message << '\n'; std::exit(1); }
}

struct Lane {
    static constexpr bool kProduction = true;
    bool suspended = false, session = true, layout = true, target_live = true;
    bool primary_busy = false, peer_busy = false, refresh_ok = true;
    bool foreground_started = false, keyboard_focus = true;
    unsigned lane = 0, held_button = 272, releases = 0, leaves = 0;
    uint64_t capabilities = 8, desktop_generation = 1;
    Trace* trace = nullptr;
    void* listen_source = nullptr;
    int listener = -1;
    std::shared_ptr<Window> window = std::make_shared<Window>();
    std::shared_ptr<Surface> surface = std::make_shared<Surface>();
    Target::Geometry geometry{20, 30, 500, 400, 20, 30};
    Target pointer_target;
    std::vector<std::unique_ptr<Pointer>> pointers;
    std::vector<std::unique_ptr<Client>> clients;
    Client* lease = nullptr;
    Client* reservation = nullptr;
    std::optional<Drag> drag;
    InputGrant grant;
    Clock::time_point expires;
    std::vector<unsigned> held_keys{42};
    std::vector<std::string> responses;

    Lane() {
        clients.push_back(std::make_unique<Client>());
        lease = reservation = clients.front().get();
        lease->window = window; lease->surface = surface;
        pointer_target.capture(window, surface, geometry);
        pointers.push_back(std::make_unique<Pointer>());
        pointers.front()->focus = surface;
        grant.arm(8, Clock::now()); expires = grant.deadline();
        drag.emplace(lease);
    }
    bool available() const { return !suspended && session; }
    bool layout_qualified() const { return layout; }
    bool refresh(Client&) const { return refresh_ok; }
    template<typename T> bool primary_conflict(const T&) const { return primary_busy; }
    template<typename T> bool agent_conflict(const T&) const { return peer_busy; }
    std::optional<Target::Geometry> target_geometry(
        const std::shared_ptr<Window>& w, const std::shared_ptr<Surface>& s) const {
        if (!target_live || !w || !s) return {};
        return geometry;
    }
    void send(Client&, const std::string& response) { responses.push_back(response); }
    void finish_foreground() { foreground_started = false; }
    void release_pointer_button() { if (held_button) ++releases; held_button = 0; }
    void leave_pointer() {
        release_pointer_button(); ++leaves; pointer_target.reset();
        for (auto& pointer : pointers) pointer->focus.reset();
    }
    void leave_keyboard() { held_keys.clear(); keyboard_focus = false; }
    void cleanup_socket() {}
    // PRODUCTION_METHODS
    void check_inert(bool retained = true) const {
        check(!lease && !drag && !held_button && held_keys.empty() && !keyboard_focus &&
              !capabilities && grant.deadline() == Clock::time_point{} &&
              expires == Clock::time_point{}, "fault retained input or action authority");
        check(releases == 1, "held button was not released exactly once");
        check(pointer_target.entered() == retained && leaves == (retained ? 0u : 1u),
              "unexpected pointer leave/retention");
    }
};

int main() {
    { Lane l; l.suspend(); l.guard_targets(); l.check_inert();
      check(!l.reservation && l.clients.empty() && !l.available(), "disable retained admission"); }
    { Lane l; l.suspend("plugin_shutdown"); l.guard_targets(); l.check_inert(false); }
    { Lane l; l.desktop_transition(); l.session = false; l.layout = false;
      l.guard_targets(); l.check_inert();
      check(!l.reservation && l.clients.front()->dead && l.desktop_generation == 2,
            "transition retained connection or generation");
      l.session = true; l.layout = true; l.guard_targets(); l.check_inert(); }
    { Lane l; l.desktop_generation = UINT64_MAX; l.desktop_transition();
      l.guard_targets(); l.check_inert(); check(l.suspended, "generation overflow admitted input"); }
    for (bool session_fault : {true, false}) {
        for (int path = 0; path < 3; ++path) {
            Lane l;
            if (session_fault) l.session = false; else l.layout = false;
            if (path == 0) l.guard_targets();
            if (path == 1) l.target_refusal(*l.lease);
            if (path == 2) l.action_refusal(*l.lease);
            l.guard_targets(); l.check_inert();
            if (path == 1)
                check(l.clients.front()->token.empty() && !l.clients.front()->approved_deadline,
                      "refused TARGET retained old binding authority");
            l.session = true; l.layout = true; l.guard_targets(); l.check_inert();
        }
    }
    // Every passive safety guard still applies during either desktop fault.
    for (bool session_fault : {true, false}) {
        for (int bad = 0; bad < 8; ++bad) {
            Lane l; l.desktop_transition();
            if (session_fault) l.session = false; else l.layout = false;
            switch (bad) {
                case 0: l.target_live = false; break;
                case 1: ++l.geometry[0]; break;
                case 2: l.pointers.front()->dead = true; break;
                case 3: l.pointers.front()->resource_live = false; break;
                case 4: l.pointers.front()->focus.reset(); break;
                case 5: l.primary_busy = true; break;
                case 6: l.peer_busy = true; break;
                case 7: l.window.reset(); break;
            }
            l.guard_targets(); l.check_inert(false);
        }
    }
    // Active safety failures and explicit human takeover continue to leave.
    for (int bad = 0; bad < 6; ++bad) {
        Lane l;
        switch (bad) {
            case 0: l.refresh_ok = false; break;
            case 1: l.primary_busy = true; break;
            case 2: l.peer_busy = true; break;
            case 3: ++l.lease->revision; break;
            case 4: l.expires = Clock::time_point{}; break;
            case 5: l.cancel_authority("primary_target_busy", false); break;
        }
        l.guard_targets(); l.check_inert(false);
    }
    std::cout << "production desktop fault policy tests passed\n";
}
