#include "session_availability.hpp"

#include <cstdlib>
#include <iostream>
#include <string>

namespace {
int failures = 0;

void expect(bool condition, const char* what) {
    if (!condition) {
        std::cerr << "FAIL: " << what << '\n';
        ++failures;
    }
}
} // namespace

int main() {
    using cua::hyprland::SessionAvailability;
    using cua::hyprland::with_unavailable;

    const SessionAvailability ready{};
    expect(ready.available(), "all predicates hold");
    expect(ready.unavailable().empty(), "no failed predicate");

    // Each predicate alone refuses and is named.
    SessionAvailability a{};
    a.retired = true;
    expect(!a.available() && a.unavailable() == "plugin_retired", "retired");
    a = {};
    a.suspended = true;
    expect(!a.available() && a.unavailable() == "plugin_suspended", "suspended");
    a = {};
    a.compositor_session_active = false;
    expect(!a.available() && a.unavailable() == "compositor_session_inactive", "session inactive");
    a = {};
    a.compositor_dpms_on = false;
    expect(!a.available() && a.unavailable() == "compositor_dpms_off", "dpms off");
    a = {};
    a.shutting_down = true;
    expect(!a.available() && a.unavailable() == "compositor_shutting_down", "shutting down");
    a = {};
    a.session_locked = true;
    expect(!a.available() && a.unavailable() == "session_locked", "session locked");

    // Several failures: the first in the fixed order is named, and all are
    // reported as booleans.
    a = {};
    a.compositor_dpms_on = false;
    a.session_locked = true;
    expect(a.unavailable() == "compositor_dpms_off", "fixed order");
    const auto status = a.json();
    expect(status.find(R"("available":false)") != std::string::npos, "status available");
    expect(status.find(R"("compositor_dpms_on":false)") != std::string::npos, "status dpms");
    expect(status.find(R"("session_locked":true)") != std::string::npos, "status lock");
    expect(ready.json().find(R"("unavailable":"")") != std::string::npos, "status empty reason");

    // Refusals keep their code and detail and only gain the reason.
    const std::string refusal = R"({"ok":false,"code":"session_unavailable","detail":"session_unavailable"})";
    a = {};
    a.session_locked = true;
    expect(with_unavailable(refusal, a) ==
               R"({"ok":false,"code":"session_unavailable","detail":"session_unavailable","unavailable":"session_locked"})",
        "refusal names the predicate");
    expect(with_unavailable(refusal, ready) == refusal, "available leaves the refusal unchanged");

    if (failures) return EXIT_FAILURE;
    std::cout << "session availability: ok\n";
    return EXIT_SUCCESS;
}
