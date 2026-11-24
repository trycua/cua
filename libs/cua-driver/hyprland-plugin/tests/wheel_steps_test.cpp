#include "wheel_steps.hpp"

#include <cstdint>
#include <cstdlib>
#include <iostream>
#include <vector>

namespace {
struct Pointer {
    int v;
    std::vector<std::int32_t> value120, discrete;
    int version() const { return v; }
    void sendAxisValue120(int, std::int32_t n) { value120.push_back(n); }
    void sendAxisDiscrete(int, std::int32_t n) { discrete.push_back(n); }
};

void check(bool condition, const char* message) {
    if (!condition) {
        std::cerr << "FAIL: " << message << '\n';
        std::exit(1);
    }
}
}

int main() {
    using cua::hyprland::send_wheel_steps;

    Pointer v8{8, {}, {}};
    send_wheel_steps(v8, 0, 30);
    send_wheel_steps(v8, 1, -50);
    send_wheel_steps(v8, 0, 0.01);
    check(v8.value120 == std::vector<std::int32_t>{360, -600}, "version 8 sends 120 per click, never zero");
    check(v8.discrete.empty(), "version 8 sends no axis_discrete");

    Pointer v5{5, {}, {}};
    send_wheel_steps(v5, 0, 30);
    send_wheel_steps(v5, 0, -10);
    send_wheel_steps(v5, 0, 4);
    check(v5.discrete == std::vector<std::int32_t>{3, -1}, "versions 5 to 7 send whole clicks, never zero");
    check(v5.value120.empty(), "versions 5 to 7 send no axis_value120");

    Pointer v4{4, {}, {}};
    send_wheel_steps(v4, 0, 30);
    check(v4.value120.empty() && v4.discrete.empty(), "versions before 5 get no click count");
}
