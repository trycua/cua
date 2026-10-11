#!/usr/bin/env python3
"""Evaluate PROBE-CANVAS: BenchLab canvas mode.

Usage: evaluate.py --seed N --state PATH --events PATH --result OUT.json
"""

from __future__ import annotations

import math
import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402

MODE = "canvas"
TOLERANCE = 8.0  # px the tile may stick out of the zone outline
REPLAY_TOLERANCE = 1.0  # px between event-replayed and state tile centers
MIN_MOVE = 20.0  # px a tile must travel from its start position
MIN_SEQUENCES = 3


def _centers(value: Any) -> dict[str, tuple[float, float]] | None:
    if not isinstance(value, list):
        return None
    out: dict[str, tuple[float, float]] = {}
    for item in value:
        if not isinstance(item, dict) or not isinstance(item.get("name"), str):
            return None
        if not (C.is_num(item.get("cx")) and C.is_num(item.get("cy"))):
            return None
        out[item["name"]] = (float(item["cx"]), float(item["cy"]))
    return out


def _inside(center: tuple[float, float], zone: dict[str, int]) -> bool:
    half = C.TILE_SIZE / 2
    cx, cy = center
    return (
        cx - half >= zone["x"] - TOLERANCE
        and cx + half <= zone["x"] + zone["w"] + TOLERANCE
        and cy - half >= zone["y"] - TOLERANCE
        and cy + half <= zone["y"] + zone["h"] + TOLERANCE
    )


def drag_sequences(events: list[dict[str, Any]]) -> list[str]:
    """Tile names of completed drags: mouseDown on a tile, >=1 mouseDragged, mouseUp."""
    sequences: list[str] = []
    open_tile: str | None = None
    dragged = 0
    for event in C.events_of(events, "mouse"):
        d = C.details(event)
        phase, tile = d.get("phase"), d.get("tile")
        if phase == "mouseDown":
            open_tile, dragged = (tile if isinstance(tile, str) else None), 0
        elif phase == "mouseDragged":
            if open_tile is not None and tile == open_tile:
                dragged += 1
        elif phase == "mouseUp":
            if open_tile is not None and tile == open_tile and dragged >= 1:
                sequences.append(open_tile)
            open_tile, dragged = None, 0
    return sequences


def evaluate(
    seed: int, state: dict[str, Any], events: list[dict[str, Any]], checks: C.Checks
) -> None:
    exp = C.derive_canvas(seed)
    zones = {z["name"]: z for z in exp["zones"]}
    starts = {t["name"]: (t["x"] + t["size"] / 2, t["y"] + t["size"] / 2) for t in exp["tiles"]}
    final = _centers(state.get("tiles"))

    state_zones = state.get("zones")
    zones_ok = isinstance(state_zones, list) and len(state_zones) == 3
    if zones_ok:
        by_name = {z.get("name"): z for z in state_zones if isinstance(z, dict)}
        for name, z in zones.items():
            sz = by_name.get(name)
            zones_ok = (
                zones_ok
                and sz is not None
                and all(
                    C.is_num(sz.get(k)) and abs(sz[k] - z[k]) < 0.5 for k in ("x", "y", "w", "h")
                )
            )
    checks.add(
        "zones_consistent",
        zones_ok,
        0.05,
        "state zones match the seed layout"
        if zones_ok
        else "state zones differ from the seed layout",
    )

    for name in C.COLORS:
        if final is None or name not in final:
            checks.add(f"{name}_in_zone", False, 0.1, "state has no tile centers")
            continue
        ok = _inside(final[name], zones[name])
        checks.add(f"{name}_in_zone", ok, 0.1, f"center {final[name]!r}, zone {zones[name]!r}")

    done_events = C.events_of(events, "done_click")
    done_ok = state.get("done") is True and bool(done_events)
    checks.add(
        "done_pressed",
        done_ok,
        0.1,
        f"done={state.get('done')!r}, {len(done_events)} done_click event(s)",
    )

    snap = _centers(state.get("done_snapshot"))
    event_snap = _centers(C.details(done_events[-1]).get("tiles")) if done_events else None
    placed = (
        snap is not None
        and event_snap == snap
        and all(name in snap and _inside(snap[name], zones[name]) for name in C.COLORS)
    )
    checks.add(
        "placed_when_done",
        placed,
        0.1,
        "tiles were inside their zones when Done was pressed"
        if placed
        else "tiles were not all inside their zones at the Done press",
    )

    seqs = drag_sequences(events)
    seq_ok = len(seqs) >= MIN_SEQUENCES and state.get("drag_sequences") == len(seqs)
    checks.add(
        "drag_sequences",
        seq_ok,
        0.15,
        f"{len(seqs)} drag sequence(s) in events, state drag_sequences={state.get('drag_sequences')!r}, need >= {MIN_SEQUENCES}",
    )

    replay = dict(starts)
    for event in C.events_of(events, "mouse"):
        d = C.details(event)
        if isinstance(d.get("tile"), str) and C.is_num(d.get("cx")) and C.is_num(d.get("cy")):
            replay[d["tile"]] = (float(d["cx"]), float(d["cy"]))
    via_ok = (
        final is not None
        and all(
            name in final and math.dist(final[name], replay[name]) <= REPLAY_TOLERANCE
            for name in C.COLORS
        )
        and all(name in seqs for name in C.COLORS)
    )
    checks.add(
        "positions_via_events",
        via_ok,
        0.15,
        "final tile centers are reproduced by logged drags"
        if via_ok
        else f"state {final!r} vs event replay {replay!r}, dragged tiles {sorted(set(seqs))}",
    )

    moved = final is not None and all(
        name in final and math.dist(final[name], starts[name]) >= MIN_MOVE for name in C.COLORS
    )
    checks.add(
        "tiles_moved",
        moved,
        0.1,
        "every tile moved away from its start position"
        if moved
        else "a tile did not move from its start position",
    )


if __name__ == "__main__":
    C.main_guard(MODE, evaluate)
