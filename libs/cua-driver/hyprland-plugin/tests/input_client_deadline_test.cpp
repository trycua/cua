#include "input_client_deadline.hpp"

#include <array>
#include <cstdlib>
#include <iostream>

using cua::hyprland::InputClientDeadline;
using namespace std::chrono_literals;

namespace {
void check(bool condition, const char* message) {
    if (!condition) {
        std::cerr << "FAIL: " << message << '\n';
        std::exit(1);
    }
}
}

int main() {
    const auto start = InputClientDeadline::Clock::time_point{} + 1h;
    InputClientDeadline deadline(start);
    check(!deadline.expired(false, start + 4999ms), "HELLO allowed before acceptance deadline");
    for (auto elapsed = 1s; elapsed < 5s; elapsed += 1s) {
        deadline.touch(start + elapsed);
        check(!deadline.expired(false, start + elapsed), "pre-HELLO traffic is initially tolerated");
    }
    check(deadline.expired(false, start + 5s), "traffic cannot extend the hard HELLO deadline");
    check(deadline.expired(false, start + 6s), "late HELLO must be refused before handling it");
    deadline.touch(start + 6s);
    check(deadline.expired(false, start + 6s), "even late activity cannot revive an uninitialized client");

    InputClientDeadline initialized(start);
    initialized.touch(start + 4s); // successful HELLO
    check(!initialized.expired(true, start + 63999ms), "HELLO begins rolling idle time");
    check(initialized.expired(true, start + 64s), "initialized idle deadline is sixty seconds");
    initialized.touch(start + 63s);
    check(!initialized.expired(true, start + 122999ms), "initialized traffic renews idle time");
    check(initialized.expired(true, start + 123s), "renewed idle deadline remains bounded");

    std::array<InputClientDeadline, 8> clients{
        InputClientDeadline(start), InputClientDeadline(start),
        InputClientDeadline(start), InputClientDeadline(start),
        InputClientDeadline(start), InputClientDeadline(start),
        InputClientDeadline(start), InputClientDeadline(start)};
    for (auto& client : clients) {
        client.touch(start + 4999ms);
        check(client.expired(false, start + 5s), "every pre-HELLO slot becomes reclaimable");
    }
    std::cout << "input client deadline tests passed\n";
}
