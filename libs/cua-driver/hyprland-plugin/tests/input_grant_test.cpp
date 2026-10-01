#include "input_grant.hpp"

#include <cstdlib>
#include <iostream>
#include <limits>

using cua::hyprland::InputGrant;
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
    const auto now = InputGrant::Clock::time_point{} + 1h;
    InputGrant first, second;
    check(!first.permits(1, now), "fresh connection has no input authority");
    for (const auto cap : {1u, 2u, 4u, 8u, 16u}) {
        check(first.arm(cap, now), "each bounded operation can be admitted");
        check(first.permits(cap, now + 4999ms), "admission lasts less than five seconds");
        check(!first.permits(cap, now + 5s), "deadline is exclusive");
        check(!first.consume(cap, now + 5s), "expired authority cannot dispatch");
        check(first.arm(cap, now), "fresh TARGET can reacquire");
        check(first.consume(cap, now + 1s), "first dispatch consumes the grant");
        check(!first.consume(cap, now + 1s), "a second dispatch cannot reuse authority");
    }
    for (const auto cap : {0ull, 3ull, 5ull, 7ull, 15ull, 17ull,
                           std::numeric_limits<unsigned long long>::max()}) {
        check(first.arm(1, now), "seed valid authority");
        check(!first.arm(cap, now), "zero, combined and unknown capability masks refuse");
        check(!first.permits(1, now), "invalid fresh admission retires old authority");
    }
    check(first.arm(1, now) && second.arm(8, now), "two independent lanes admit different operations");
    check(!first.consume(2, now), "one capability never grants another operation");
    check(!first.permits(1, now), "wrong dispatch cannot retain the previous capability");
    check(second.permits(8, now), "one lane's refusal preserves the other lane");
    first.reset();
    check(second.consume(8, now + 2s), "other lane remains usable after cancellation");
    check(first.arm(2, now), "new admission after cancellation succeeds");
    first.reset();
    check(!first.consume(2, now), "revocation invalidates a pending operation");
    std::cout << "bounded input grant tests passed\n";
}
