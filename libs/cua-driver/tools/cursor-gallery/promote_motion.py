#!/usr/bin/env python3
"""Promote a tuned movement override into the shared movement spec.

Reads the fixed-path override file written by the cursor-gallery dev
server, merges its clamped values over the current repository spec, and
rewrites `motion.default.json` in the canonical serialization (schema
first, fixed key order, two-space indent, trailing newline) so spec
diffs are value-only. `--validate-only` checks without writing.

The Rust parity tests in
`rust/crates/cursor-overlay/src/motion.rs` remain the authority: they
fail if the promoted spec no longer reproduces `MotionConfig::default`.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

OVERRIDE_SCHEMA = "cua.cursor-gallery-motion-override/1"
SPEC_SCHEMA = "cua.cursor-motion/1"

# Every key the embedded Rust spec parse requires. Promotion must never
# remove or rename one; a spec missing any of these fails the Rust
# `deny_unknown_fields` build, so validate before writing.
REQUIRED_SHARED_KEYS = (
    "start_handle",
    "end_handle",
    "arc_size",
    "arc_flow",
    "spring",
    "glide_duration_ms",
    "dwell_after_click_ms",
    "idle_hide_ms",
    "press_duration_ms",
    "peak_speed",
    "min_start_speed",
    "min_end_speed",
    "turn_radius",
    "click_offset",
)
REQUIRED_SETTLE = {
    "macos": ("mode", "k", "c", "overshoot"),
    "windows_linux": ("mode", "k_per_spring", "c_per_spring", "overshoot"),
}

# Override fields -> spec key under `shared`. Only shared-section
# tunables are promotable; platform spring-settle divergence stays a
# deliberate repository decision, not a playground knob.
PROMOTABLE = {
    "peak_speed": "peak_speed",
    "min_start_speed": "min_start_speed",
    "min_end_speed": "min_end_speed",
    "turn_radius": "turn_radius",
    "spring": "spring",
    "glide_duration_ms": "glide_duration_ms",
}


def fail(message: str) -> None:
    print(f"promote-motion: {message}", file=sys.stderr)
    raise SystemExit(1)


def load_override(path: Path) -> dict[str, float]:
    try:
        payload = json.loads(path.read_text())
    except FileNotFoundError:
        fail(f"override file not found: {path}")
    except json.JSONDecodeError as error:
        fail(f"override file is not valid JSON: {error}")
    if not isinstance(payload, dict):
        fail("override file must be a JSON object")
    if payload.get("schema") != OVERRIDE_SCHEMA:
        fail(f"override schema must be {OVERRIDE_SCHEMA}")
    unknown = set(payload) - {"schema"} - set(PROMOTABLE)
    if unknown:
        fail(f"override carries non-promotable fields: {sorted(unknown)}")
    override: dict[str, float] = {}
    for key, value in payload.items():
        if key == "schema":
            continue
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            fail(f"override field {key} must be a number")
        number = float(value)
        if number != number or number in (float("inf"), float("-inf")):
            fail(f"override field {key} must be finite")
        override[key] = number
    if not override:
        fail("override has no movement values to promote")
    return override


def load_spec(path: Path) -> dict:
    try:
        spec = json.loads(path.read_text())
    except FileNotFoundError:
        fail(f"spec file not found: {path}")
    except json.JSONDecodeError as error:
        fail(f"spec file is not valid JSON: {error}")
    if not isinstance(spec, dict) or spec.get("schema") != SPEC_SCHEMA:
        fail(f"spec schema must be {SPEC_SCHEMA}")
    return spec


def canonicalize(spec: dict) -> str:
    """Serialize the spec in the one canonical repository shape.

    Key order comes from the loaded file (json.loads preserves it), so a
    promotion rewrites values only; keys are neither added, removed, nor
    reordered. The complete required structure is validated first so the
    Rust `deny_unknown_fields` parse stays exhaustive.
    """
    shared: dict = spec.get("shared")  # type: ignore[assignment]
    if not isinstance(shared, dict):
        fail("spec is missing the `shared` section")
    missing = [key for key in REQUIRED_SHARED_KEYS if key not in shared]
    if missing:
        fail(f"spec `shared` is missing required keys: {missing}")
    settle = spec.get("spring_settle")
    if not isinstance(settle, dict):
        fail("spec is missing the `spring_settle` section")
    for platform, keys in REQUIRED_SETTLE.items():
        block = settle.get(platform)
        if not isinstance(block, dict):
            fail(f"spec `spring_settle.{platform}` is missing")
        absent = [key for key in keys if key not in block]
        if absent:
            fail(f"spec `spring_settle.{platform}` is missing keys: {absent}")
    return json.dumps(spec, indent=2) + "\n"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--override", type=Path, required=True)
    parser.add_argument("--spec", type=Path, required=True)
    parser.add_argument(
        "--validate-only",
        action="store_true",
        help="check the override and spec without writing",
    )
    args = parser.parse_args()

    override = load_override(args.override)
    spec = load_spec(args.spec)

    before = canonicalize(spec)
    promoted = []
    for field, key in PROMOTABLE.items():
        if field not in override:
            continue
        current = spec["shared"].get(key)
        if current == override[field]:
            continue
        spec["shared"][key] = override[field]
        promoted.append(f"shared.{key}: {current} -> {override[field]}")

    after = canonicalize(spec)

    if args.validate_only:
        print("override and spec are promotable:")
        for line in promoted or ["(values already match the spec)"]:
            print(f"  {line}")
        return

    if promoted:
        args.spec.write_text(after)
        print(f"promoted {len(promoted)} value(s) into {args.spec}:")
        for line in promoted:
            print(f"  {line}")
        if before != after:
            print("review the diff, run the motion parity tests, then commit.")
    else:
        print("override values already match the spec; nothing written.")


if __name__ == "__main__":
    main()
