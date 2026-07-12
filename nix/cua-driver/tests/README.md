# Linux Nix check layout

Nix expressions are grouped by what they prove:

| Path | Role |
| --- | --- |
| `rust-unit.nix` | Source-built Rust workspace checks without a desktop session |

This directory intentionally contains no desktop behavior catalog. Nix builds
the driver, unit checks, session dependencies, and optional compositor package.
The canonical GUI scenarios and assertions live in the typed Rust harnesses.

New user-behavior coverage belongs in the Rust test harness first. A Nix check
may provide the session and package environment, but it must invoke the shared
Rust catalog instead of defining a second set of behavioral assertions.
