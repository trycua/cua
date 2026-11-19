#pragma once

#include <format>
#include <string>
#include <string_view>

namespace cua::hyprland {
// The fixed, content-free predicates behind `session_unavailable`. Reporting
// which one failed changes no admission decision: every predicate is still
// required, in this order.
struct SessionAvailability {
    bool retired = false;
    bool suspended = false;
    bool compositor_session_active = true;
    bool compositor_dpms_on = true;
    bool shutting_down = false;
    bool session_locked = false;

    // First failed predicate, or empty when input may be admitted.
    std::string_view unavailable() const {
        if (retired) return "plugin_retired";
        if (suspended) return "plugin_suspended";
        if (!compositor_session_active) return "compositor_session_inactive";
        if (!compositor_dpms_on) return "compositor_dpms_off";
        if (shutting_down) return "compositor_shutting_down";
        if (session_locked) return "session_locked";
        return {};
    }

    bool available() const { return unavailable().empty(); }

    std::string json() const {
        return std::format(
            R"({{"available":{},"unavailable":"{}","retired":{},"suspended":{},"compositor_session_active":{},"compositor_dpms_on":{},"shutting_down":{},"session_locked":{}}})",
            available(), unavailable(), retired, suspended, compositor_session_active, compositor_dpms_on, shutting_down,
            session_locked);
    }
};

// Add the failed predicate to an existing fixed refusal object. Only the
// internal identifiers above ever enter the JSON.
inline std::string with_unavailable(std::string refusal, const SessionAvailability& availability) {
    const auto reason = availability.unavailable();
    if (reason.empty() || refusal.size() < 2 || refusal.back() != '}') return refusal;
    refusal.insert(refusal.size() - 1, std::format(R"(,"unavailable":"{}")", reason));
    return refusal;
}
} // namespace cua::hyprland
