#include "external_input.hpp"

#include <cstdlib>
#include <iostream>

int main() {
    using cua::hyprland::ExternalInputCount;
    using cua::hyprland::ExternalInputKind;
    int failures = 0;
    auto expect = [&](bool condition, const char* what) {
        if (!condition) {
            std::cerr << "FAIL: " << what << '\n';
            ++failures;
        }
    };

    ExternalInputCount count;
    expect(count.value() == 0, "starts at zero");
    // Compositor warps and refocus arrive only as unified motion.
    count.note(ExternalInputKind::unified_motion);
    expect(count.value() == 0, "unified motion is not counted");
    // Everything a person does with a device is counted, one per event,
    // including motion that returns to where it started and a pointer
    // device that appears mid-transaction.
    for (const auto kind : {ExternalInputKind::key, ExternalInputKind::pointer_motion, ExternalInputKind::button,
             ExternalInputKind::axis, ExternalInputKind::gesture, ExternalInputKind::touch, ExternalInputKind::tablet,
             ExternalInputKind::switch_toggle, ExternalInputKind::device_added})
        count.note(kind);
    expect(count.value() == 9, "device input of every kind is counted");
    count.note(ExternalInputKind::pointer_motion);
    count.note(ExternalInputKind::pointer_motion);
    expect(count.value() == 11, "motion back to an accepted position is still counted");

    if (failures) return EXIT_FAILURE;
    std::cout << "external input: ok\n";
    return EXIT_SUCCESS;
}
