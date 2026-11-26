#include "external_pointer_motion.hpp"

#include <cstdlib>
#include <iostream>
#include <string>

using cua::hyprland::ExternalPointerMotion;

namespace {
void check(bool condition, const char* message) {
    if (!condition) {
        std::cerr << "FAIL: " << message << '\n';
        std::exit(1);
    }
}

// The lane rule from input_experiment.cpp, reduced to what decides the reply to
// a typed key: FOREGROUND_TARGET binds a token, a user-input notification
// cancels authority and clears it, and KEY with a cleared token is refused.
struct Lane {
    ExternalPointerMotion& motion;
    // The rule before this fix: every emission was user input, and the
    // cancellation reason was not kept.
    bool legacy = false;
    std::string token, cancelled_by;
    void target() { token = "t"; cancelled_by.clear(); }
    void mouse_move(ExternalPointerMotion::Point position) {
        const bool user = motion.user_moved(position) || legacy;
        if (user && !token.empty()) { token.clear(); if (!legacy) cancelled_by = "foreground_interrupted"; }
    }
    std::string key() {
        if (token.empty()) return cancelled_by.empty() ? "stale_target" : cancelled_by;
        token.clear();
        return "ok";
    }
};

// Type `keys` characters. Between each FOREGROUND_TARGET and its KEY, a busy
// compositor runs `refocus_passes` refocus passes (a bar, notification or
// overlay mapping), all reporting the unmoved cursor at (322, 413).
std::string type(ExternalPointerMotion& motion, unsigned keys, unsigned refocus_passes, bool legacy = false) {
    Lane lane{motion, legacy, {}, {}};
    for (unsigned i = 0; i < keys; ++i) {
        lane.target();
        for (unsigned pass = 0; pass < refocus_passes; ++pass) lane.mouse_move({322.4, 413.9});
        if (const auto reply = lane.key(); reply != "ok") return reply;
    }
    return "ok";
}
} // namespace

int main() {
    {
        ExternalPointerMotion motion;
        check(motion.user_moved({10, 10}), "without a known position, motion is assumed to be the user's");
        check(!motion.user_moved({10, 10}), "a refocus pass at the same position is not user motion");
        check(!motion.user_moved({10.7, 10.2}), "positions compare as Hyprland floors them");
        check(motion.user_moved({11, 10}), "a moved cursor is the user's motion");
        check(!motion.user_moved({11, 10}), "the moved position becomes the new baseline");
    }
    {
        ExternalPointerMotion motion;
        motion.rebase({322, 413});
        check(!motion.user_moved({322, 413}), "the position when listening starts is not user motion");
        motion.rebase({500, 200});
        check(!motion.user_moved({500.5, 200.5}), "a refocus pass after the plugin's own warp is not user motion");
        check(motion.user_moved({501, 200}), "the user moving after the warp still interrupts");
    }
    {
        // The reported failure: a live foot window refused part-way through a
        // 46-character command while Hyprland was busy with session start.
        ExternalPointerMotion motion;
        motion.rebase({322, 413});
        check(type(motion, 46, 0, true) == "ok", "the old rule typed everything on an idle compositor");
        check(type(motion, 46, 1, true) == "stale_target", "the old rule refused a live target on a busy compositor");
        check(type(motion, 46, 0) == "ok", "an idle compositor types the whole string");
        check(type(motion, 46, 3) == "ok", "refocus passes between keys no longer retire a live target");
    }
    {
        // A real user move mid-type still stops typing, and the refusal names
        // the interruption, not a stale window.
        ExternalPointerMotion motion;
        motion.rebase({322, 413});
        Lane lane{motion, false, {}, {}};
        lane.target();
        check(lane.key() == "ok", "first key is delivered");
        lane.target();
        lane.mouse_move({340, 420});
        check(lane.key() == "foreground_interrupted", "user motion interrupts with its own reason");
    }
    return 0;
}
