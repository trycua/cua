#include "user_seat_policy.hpp"

#include <cstdio>
#include <cstdlib>

namespace {

void check(bool condition, const char* what) {
    if (!condition) {
        std::fprintf(stderr, "FAIL: %s\n", what);
        std::exit(1);
    }
}

} // namespace

int main() {
    using cua::hyprland::user_seat_request_allowed;
    // Opaque client identities: only their addresses are compared.
    char storage[3]{};
    const auto* app = reinterpret_cast<const wl_client*>(&storage[0]);
    const auto* user_app = reinterpret_cast<const wl_client*>(&storage[1]);
    const auto* panel = reinterpret_cast<const wl_client*>(&storage[2]);

    check(user_seat_request_allowed(app, app, nullptr), "keyboard-focused client opens a menu from a key");
    check(user_seat_request_allowed(app, user_app, app), "client under the user's pointer starts a drag");
    check(user_seat_request_allowed(panel, user_app, panel), "a panel the user clicked opens its menu");
    check(!user_seat_request_allowed(app, user_app, user_app), "background app: no grab or drag");
    check(!user_seat_request_allowed(app, user_app, nullptr), "background app, pointer over nothing: no grab");
    check(!user_seat_request_allowed(app, nullptr, nullptr), "no user focus at all: no grab");
    check(!user_seat_request_allowed(nullptr, nullptr, nullptr), "request without a surface: no grab");
    std::puts("user seat policy: ok");
    return 0;
}
