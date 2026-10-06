"""Cua Cursor Motion through the generated Python bindings.

Replays the golden trajectories that the Rust crate and the TypeScript port
share (libs/cua-driver/rust/crates/cua-cursor-motion/fixtures/golden.json)
through `plan_cursor_move` / `plan_cursor_spec` in the staged native library.
"""

from __future__ import annotations

import json
import math
import os
import platform
import unittest
from pathlib import Path


def _library_name() -> str:
    if os.name == "nt":
        return "cua_driver_sdk.dll"
    if platform.system() == "Darwin":
        return "libcua_driver_sdk.dylib"
    return "libcua_driver_sdk.so"


ROOT = Path(__file__).parents[1]
LIBRARY = ROOT / "src" / "cua_driver" / _library_name()
GOLDEN = ROOT.parent / "rust/crates/cua-cursor-motion/fixtures/golden.json"
# The fixture rounds to 1e-9; the bindings run the same Rust code.
TOL = 1e-8
FRAME_FRACTIONS = (0.2, 0.5, 0.8, 1.0)

if os.environ.get("CUA_DRIVER_REQUIRE_UNIFFI") == "1" and not LIBRARY.exists():
    raise RuntimeError(f"required staged UniFFI library is missing: {LIBRARY}")


@unittest.skipUnless(LIBRARY.exists(), "host-native UniFFI library is not staged")
class CursorMotionGoldenTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        from cua_driver import _native as n
        from cua_driver import _native_contract as c

        cls.n = n
        cls.c = c
        cls.golden = json.loads(GOLDEN.read_text())

    # Fixture JSON -> generated records -------------------------------------

    def params(self, p: dict):
        n, c = self.n, self.c
        return n.CursorMotionParams(
            style=getattr(c.CursorMotionStyle, p["style"].upper()),
            timing=getattr(c.CursorMotionTiming, p["timing"].upper()),
            effects=c.CursorMotionEffects(**{k: p["effects"][k] for k in self.effect_names}),
            start_handle=p["start_handle"],
            end_handle=p["end_handle"],
            arc_size=p["arc_size"],
            arc_flow=p["arc_flow"],
            spring=p["spring"],
            glide_duration_ms=p["glide_duration_ms"],
            peak_speed=p["peak_speed"],
            min_start_speed=p["min_start_speed"],
            min_end_speed=p["min_end_speed"],
            turn_radius=p["turn_radius"],
        )

    effect_names = ("trail", "glow", "magnet", "ripple", "squish")

    def request(self, r: dict):
        n = self.n
        target = r["target"]
        return n.CursorMoveRequest(
            from_point=n.CursorMotionPoint(x=r["from"][0], y=r["from"][1]),
            to_point=n.CursorMotionPoint(x=r["to"][0], y=r["to"][1]),
            from_heading=r["from_heading"],
            end_heading=r["end_heading"],
            target=None
            if target is None
            else n.CursorMotionRect(x=target[0], y=target[1], width=target[2], height=target[3]),
            seed=r["seed"],
            reduced_motion=r["reduced_motion"],
        )

    @staticmethod
    def variant(enum_type, value: dict):
        fields = {k: v for k, v in value.items() if k != "type"}
        return getattr(enum_type, value["type"].upper())(**fields)

    def spec(self, s: dict):
        n, c = self.n, self.c
        return n.CursorMotionSpec(
            path=self.variant(n.CursorPathShape, s["path"]),
            ease=self.variant(n.CursorEase, s["ease"]),
            settle=self.variant(n.CursorSettle, s["settle"]),
            duration=self.variant(n.CursorMotionDuration, s["duration"]),
            heading=getattr(n.CursorHeading, s["heading"].upper()),
            effects=c.CursorMotionEffectsOutput(**s["effects"]),
            trail=n.CursorTrailSpec(**s["trail"]),
        )

    # Comparison --------------------------------------------------------------

    def near(self, name: str, got: float, want: float) -> None:
        self.assertLessEqual(abs(got - want), TOL, f"{name}: {got} vs {want}")

    def check_frame(self, name: str, frame, click, want: dict) -> None:
        def present(value, expected, fields):
            self.assertEqual(value is None, expected is None, f"{name} presence")
            if value is not None:
                for i, field in enumerate(fields):
                    self.near(f"{name}.{field}", field(value), expected[i])

        present(frame.glow, want["glow"], [lambda g: g.x, lambda g: g.y, lambda g: g.r, lambda g: g.alpha])
        present(
            frame.magnet,
            want["magnet"],
            [lambda m: m.rect.x, lambda m: m.rect.y, lambda m: m.rect.width, lambda m: m.rect.height, lambda m: m.glow],
        )
        present(
            click.ripple,
            want["ripple"],
            [lambda r: r.x, lambda r: r.y, lambda r: r.r, lambda r: r.width, lambda r: r.alpha],
        )
        self.near(f"{name}.squish", click.squish, want["squish"])
        self.assertEqual(len(frame.trail), len(want["trail"]), f"{name} trail length")
        for seg, w in zip(frame.trail, want["trail"]):
            for got, expected in zip((seg.ax, seg.ay, seg.bx, seg.by, seg.width, seg.alpha), w):
                self.near(f"{name}.trail", got, expected)

    def check(self, name: str, traj, out: dict) -> None:
        n, c = self.n, self.c
        samples = traj.samples()
        self.assertEqual(len(samples), out["samples"], f"{name}: sample count")
        self.near(f"{name}.duration", traj.duration(), out["duration"])
        self.near(f"{name}.arrival", traj.arrival_t(), out["arrival_t"])
        snap = traj.snap_t()
        self.assertEqual(snap is None, out["snap_t"] is None, f"{name}: snap")
        if snap is not None:
            self.near(f"{name}.snap", snap, out["snap_t"])
        t = traj.target()
        for got, want in zip((t.x, t.y, t.width, t.height), out["target"]):
            self.near(f"{name}.target", got, want)
        self.assertEqual(traj.target_known(), out["target_known"])
        fx = traj.effects()
        self.assertEqual({k: getattr(fx, k) for k in self.effect_names}, out["effects"])
        d = traj.duration()
        grid = self.golden["grid"]
        for i, want in enumerate(out["grid"]):
            s = traj.sample_at(d * i / grid)
            for field, got, expected in zip("txyh", (s.t, s.x, s.y, s.heading), want):
                self.near(f"{name}.grid[{i}].{field}", got, expected)
        if "frames" in out:
            self.near(f"{name}.linger", traj.linger(), out["linger"])
            clicks = c.CursorMotionEffectsOutput(trail=False, glow=False, magnet=False, ripple=True, squish=True)
            for k, f in zip(FRAME_FRACTIONS, out["frames"]):
                at = d * k
                s = traj.sample_at(at)
                frame = traj.effect_frame(at, True)
                click = n.cursor_click_effects(clicks, 0.03 + 0.4 * k, s.x, s.y)
                self.check_frame(f"{name}@{k}", frame, click, f["frame"])

    # Tests -------------------------------------------------------------------

    def test_built_in_styles_match_the_golden_trajectories(self) -> None:
        cases = self.golden["cases"]
        self.assertGreater(len(cases), 100)
        for case in cases:
            traj = self.n.plan_cursor_move(self.params(case["params"]), self.request(case["request"]))
            self.check(case["name"], traj, case["out"])

    def test_custom_specs_match_the_golden_trajectories(self) -> None:
        for case in self.golden["spec_cases"]:
            traj = self.n.plan_cursor_spec(self.spec(case["spec"]), self.request(case["request"]))
            self.check(case["name"], traj, case["out"])

    def test_the_arc_styles_are_specs(self) -> None:
        n, c = self.n, self.c
        params = n.default_cursor_motion_params()
        request = self.request(self.golden["cases"][0]["request"])
        spec = n.cursor_motion_spec_for_style(c.CursorMotionStyle.COMET_SWOOP, params)
        self.assertIsNotNone(spec)
        params.style = c.CursorMotionStyle.COMET_SWOOP
        a = n.plan_cursor_move(params, request).samples()
        b = n.plan_cursor_spec(spec, request).samples()
        self.assertEqual(a, b)
        self.assertIsNone(n.cursor_motion_spec_for_style(c.CursorMotionStyle.MAGNETIC, params))
        self.assertTrue(math.isclose(params.arc_size, 0.25))


if __name__ == "__main__":
    unittest.main()
