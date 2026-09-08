#!/usr/bin/env python3
"""Build the deterministic dotLottie source archive for ``cua.default``.

The checked-in ``cua.default.lottie`` is the canonical authoring artifact used
to produce the embedded ``cua.default.cua-theme``.  This script deliberately
uses only Python's standard library so contributors can reproduce the source
archive without a LottieFiles account or a browser editor.

The runtime applies two bounded effects after decoding the compiled theme:

* the approved blue tint key is replaced with the stable session color;
* the shared four-second float transform is applied to the composed theme.

Those effects are runtime concerns because they must stay synchronized across
all action layers. Delivery and target context is rendered by the host badge.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any, Iterable, Sequence
from zipfile import ZIP_DEFLATED, ZipFile, ZipInfo

CANVAS = 128
FPS = 30
BLUE = [94 / 255, 192 / 255, 232 / 255, 1]
WHITE = [1, 1, 1, 1]
TRANSPARENT = [0, 0, 0, 0]

Point = tuple[float, float]
PathPoint = tuple[Point, Point, Point]


def static(value: Any) -> dict[str, Any]:
    return {"a": 0, "k": value}


def _value(value: Any) -> list[float]:
    if isinstance(value, (int, float)):
        return [value]
    return list(value)


def animated(
    values: Sequence[tuple[int, Any]],
    *,
    linear: bool = False,
) -> dict[str, Any]:
    frames: list[dict[str, Any]] = []
    for index, (frame, value) in enumerate(values):
        item: dict[str, Any] = {"t": frame, "s": _value(value)}
        if index + 1 < len(values):
            item["e"] = _value(values[index + 1][1])
            if linear:
                item["i"] = {"x": [0.833], "y": [0.833]}
                item["o"] = {"x": [0.167], "y": [0.167]}
            else:
                item["i"] = {"x": [0.25], "y": [1.0]}
                item["o"] = {"x": [0.2], "y": [0.75]}
        frames.append(item)
    return {"a": 1, "k": frames}


def path_shape(
    points: Sequence[PathPoint],
    *,
    closed: bool = False,
    name: str = "Path",
) -> dict[str, Any]:
    return {
        "ty": "sh",
        "nm": name,
        "ks": {
            "a": 0,
            "k": {
                "i": [[point[1][0], point[1][1]] for point in points],
                "o": [[point[2][0], point[2][1]] for point in points],
                "v": [[point[0][0], point[0][1]] for point in points],
                "c": closed,
            },
        },
    }


def points(coordinates: Sequence[Point], *, closed: bool = False, name: str = "Path") -> dict[str, Any]:
    return path_shape(
        [((x, y), (0, 0), (0, 0)) for x, y in coordinates],
        closed=closed,
        name=name,
    )


def ellipse(x: float, y: float, width: float, height: float, *, name: str) -> dict[str, Any]:
    return {
        "ty": "el",
        "nm": name,
        "p": static([x, y]),
        "s": static([width, height]),
        "d": 1,
    }


def rectangle(
    x: float,
    y: float,
    width: float,
    height: float,
    *,
    roundness: float = 0,
    name: str,
) -> dict[str, Any]:
    return {
        "ty": "rc",
        "nm": name,
        "p": static([x, y]),
        "s": static([width, height]),
        "r": static(roundness),
        "d": 1,
    }


def stroke(color: list[float], width: float, opacity: float = 100) -> dict[str, Any]:
    return {
        "ty": "st",
        "c": static(color),
        "o": static(opacity),
        "w": static(width),
        "lc": 2,
        "lj": 2,
        "ml": 4,
    }


def fill(color: list[float], opacity: float = 100) -> dict[str, Any]:
    return {"ty": "fl", "c": static(color), "o": static(opacity), "r": 1}


def shape_transform() -> dict[str, Any]:
    return {
        "ty": "tr",
        "p": static([0, 0]),
        "a": static([0, 0]),
        "s": static([100, 100]),
        "r": static(0),
        "o": static(100),
        "sk": static(0),
        "sa": static(0),
    }


def layer_transform(
    *,
    position: dict[str, Any] | None = None,
    scale: dict[str, Any] | None = None,
    rotation: dict[str, Any] | None = None,
    opacity: dict[str, Any] | None = None,
) -> dict[str, Any]:
    return {
        "o": opacity or static(100),
        "r": rotation or static(0),
        "p": position or static([64, 64]),
        "a": static([64, 64]),
        "s": scale or static([100, 100]),
    }


def shape_layer(
    index: int,
    name: str,
    shapes: Sequence[dict[str, Any]],
    frames: int,
    *,
    position: dict[str, Any] | None = None,
    scale: dict[str, Any] | None = None,
    rotation: dict[str, Any] | None = None,
    opacity: dict[str, Any] | None = None,
) -> dict[str, Any]:
    return {
        "ddd": 0,
        "ind": index,
        "ty": 4,
        "nm": name,
        "sr": 1,
        "ks": layer_transform(
            position=position,
            scale=scale,
            rotation=rotation,
            opacity=opacity,
        ),
        "ao": 0,
        "shapes": [*shapes, shape_transform()],
        "ip": 0,
        "op": frames,
        "st": 0,
        "bm": 0,
    }


CURSOR_PATH = path_shape(
    [
        ((55, 30), (0, 0), (-7, -2)),
        ((43, 41), (-1, -8), (0, 0)),
        ((64, 98), (0, 0), (3, 8)),
        ((77, 99), (-4, 7), (0, 0)),
        ((86, 79), (0, 0), (2, -4)),
        ((95, 70), (-4, 2), (0, 0)),
        ((108, 63), (0, 0), (7, -4)),
        ((107, 50), (7, 3), (0, 0)),
    ],
    closed=True,
    name="Cursor",
)


def cursor_layers(
    frames: int,
    *,
    position: dict[str, Any] | None = None,
    scale: dict[str, Any] | None = None,
) -> list[dict[str, Any]]:
    layers: list[dict[str, Any]] = []
    index = 1
    for width, opacity in [
        (44, 2.0),
        (36, 2.4),
        (29, 3.0),
        (23, 3.8),
        (18, 4.8),
        (14, 6.0),
        (10, 7.5),
        (7, 9.5),
    ]:
        layers.append(
            shape_layer(
                index,
                f"Cursor glow {width}",
                [CURSOR_PATH, stroke(BLUE, width, opacity)],
                frames,
                position=position,
                scale=scale,
            )
        )
        index += 1
    layers.append(
        shape_layer(
            index,
            "Cursor body",
            [CURSOR_PATH, fill(BLUE), stroke(WHITE, 5)],
            frames,
            position=position,
            scale=scale,
        )
    )
    return layers


def cue_layers(
    start_index: int,
    name: str,
    geometry: Sequence[dict[str, Any]],
    frames: int,
    *,
    width: float = 4,
    position: dict[str, Any] | None = None,
    scale: dict[str, Any] | None = None,
    rotation: dict[str, Any] | None = None,
    opacity: dict[str, Any] | None = None,
) -> list[dict[str, Any]]:
    layers: list[dict[str, Any]] = []
    index = start_index
    for expansion, glow_opacity in [(13, 2.4), (9, 3.1), (6, 4.0), (3, 5.2)]:
        layers.append(
            shape_layer(
                index,
                f"{name} glow {expansion}",
                [*geometry, stroke(BLUE, width + expansion, glow_opacity)],
                frames,
                position=position,
                scale=scale,
                rotation=rotation,
                opacity=opacity,
            )
        )
        index += 1
    layers.append(
        shape_layer(
            index,
            f"{name} outline",
            [*geometry, stroke(WHITE, width + 1.5)],
            frames,
            position=position,
            scale=scale,
            rotation=rotation,
            opacity=opacity,
        )
    )
    index += 1
    layers.append(
        shape_layer(
            index,
            f"{name} color",
            [*geometry, stroke(BLUE, max(width - 1, 1.5))],
            frames,
            position=position,
            scale=scale,
            rotation=rotation,
            opacity=opacity,
        )
    )
    return layers


def lottie(name: str, frames: int, layers: Sequence[dict[str, Any]]) -> dict[str, Any]:
    return {
        "v": "5.12.2",
        "fr": FPS,
        "ip": 0,
        "op": frames,
        "w": CANVAS,
        "h": CANVAS,
        "nm": name,
        "ddd": 0,
        "assets": [],
        "layers": list(reversed(layers)),
        "markers": [],
    }


def action_idle() -> dict[str, Any]:
    return lottie("action_idle", 1, cursor_layers(1))


def action_observe() -> dict[str, Any]:
    frames = 48
    geometry = [
        path_shape(
            [
                ((38, 28), (0, 0), (-11, 1)),
                ((20, 49), (0, -11), (0, 0)),
            ],
            name="Observe inner",
        ),
        path_shape(
            [
                ((42, 19), (0, 0), (-19, 0)),
                ((11, 51), (0, -18), (0, 0)),
            ],
            name="Observe outer",
        ),
    ]
    cue = cue_layers(
        20,
        "Observe",
        geometry,
        frames,
        position=static([72, 54]),
        scale=animated([(0, [88, 88]), (24, [100, 100]), (48, [108, 108])]),
        opacity=animated([(0, 0), (7, 100), (34, 100), (44, 0), (48, 0)]),
    )
    return lottie("action_observe", frames, [*cursor_layers(frames), *cue])


def action_click() -> dict[str, Any]:
    frames = 20
    geometry = [
        points([(35, 20), (34, 11)], name="Click upper"),
        points([(27, 25), (19, 19)], name="Click middle"),
        points([(25, 34), (15, 34)], name="Click lower"),
    ]
    cursor_scale = animated(
        [(0, [100, 100]), (7, [93, 93]), (12, [103, 103]), (20, [100, 100])]
    )
    cue = cue_layers(
        20,
        "Click",
        geometry,
        frames,
        position=static([74, 67]),
        scale=animated([(0, [110, 110]), (8, [130, 130]), (20, [150, 150])]),
        opacity=animated([(0, 0), (4, 100), (11, 100), (20, 0)]),
    )
    return lottie(
        "action_click",
        frames,
        [*cursor_layers(frames, scale=cursor_scale), *cue],
    )


def action_drag() -> dict[str, Any]:
    frames = 48
    movement = animated(
        [(0, [64, 64]), (8, [64, 64]), (30, [71, 67]), (40, [71, 67]), (48, [64, 64])]
    )
    geometry = [
        points([(28, 38), (16, 35)], name="Drag upper"),
        points([(26, 48), (12, 45)], name="Drag lower"),
    ]
    cue = cue_layers(
        20,
        "Drag",
        geometry,
        frames,
        position=movement,
        opacity=animated([(0, 20), (15, 100), (36, 100), (48, 20)]),
    )
    return lottie(
        "action_drag",
        frames,
        [*cursor_layers(frames, position=movement), *cue],
    )


def action_scroll() -> dict[str, Any]:
    frames = 48
    geometry = [
        points([(23, 31), (31, 22), (39, 31)], name="Scroll up"),
        points([(23, 49), (31, 58), (39, 49)], name="Scroll down"),
    ]
    cue = cue_layers(
        20,
        "Scroll",
        geometry,
        frames,
        position=animated([(0, [59, 68]), (24, [59, 60]), (48, [59, 68])]),
        opacity=animated([(0, 42), (24, 100), (48, 42)]),
    )
    return lottie("action_scroll", frames, [*cursor_layers(frames), *cue])


def action_text() -> dict[str, Any]:
    frames = 48
    geometry = [
        points([(31, 22), (31, 58)], name="Text stem"),
        points([(24, 22), (38, 22)], name="Text top"),
        points([(24, 58), (38, 58)], name="Text bottom"),
    ]
    cue = cue_layers(
        20,
        "Text",
        geometry,
        frames,
        position=static([60, 64]),
        opacity=animated(
            [(0, 100), (16, 100), (17, 18), (30, 18), (31, 100), (48, 100)],
            linear=True,
        ),
    )
    return lottie("action_text", frames, [*cursor_layers(frames), *cue])


def action_key() -> dict[str, Any]:
    frames = 48
    geometry = [
        rectangle(28, 39, 28, 28, roundness=6, name="Key cap"),
        points([(23, 32), (23, 46)], name="Key stem"),
        points([(24, 39), (33, 32)], name="Key upper"),
        points([(25, 39), (34, 46)], name="Key lower"),
    ]
    cue = cue_layers(
        20,
        "Key",
        geometry,
        frames,
        width=3.5,
        position=animated([(0, [55, 64]), (8, [55, 67]), (17, [55, 63]), (26, [55, 64]), (48, [55, 64])]),
        scale=animated([(0, [100, 100]), (8, [92, 92]), (17, [103, 103]), (26, [100, 100]), (48, [100, 100])]),
        opacity=animated([(0, 0), (4, 100), (30, 100), (42, 0), (48, 0)]),
    )
    return lottie("action_key", frames, [*cursor_layers(frames), *cue])


def action_navigate() -> dict[str, Any]:
    frames = 48
    geometry = [
        points([(15, 29), (25, 40), (15, 51)], name="Navigate first"),
        points([(29, 29), (39, 40), (29, 51)], name="Navigate second"),
    ]
    cue = cue_layers(
        20,
        "Navigate",
        geometry,
        frames,
        position=animated([(0, [54, 64]), (18, [59, 64]), (30, [63, 64]), (48, [63, 64])]),
        opacity=animated([(0, 0), (8, 100), (28, 100), (42, 0), (48, 0)]),
    )
    return lottie("action_navigate", frames, [*cursor_layers(frames), *cue])


def action_app() -> dict[str, Any]:
    frames = 48
    geometry = [
        rectangle(x, y, 10, 10, roundness=2, name=f"App {x} {y}")
        for x, y in [(18, 31), (34, 31), (18, 47), (34, 47)]
    ]
    cue = cue_layers(
        20,
        "App",
        geometry,
        frames,
        width=3.5,
        position=static([59, 64]),
        scale=animated([(0, [20, 20]), (14, [112, 112]), (24, [100, 100]), (48, [100, 100])]),
        opacity=animated([(0, 0), (8, 100), (34, 100), (44, 0), (48, 0)]),
    )
    return lottie("action_app", frames, [*cursor_layers(frames), *cue])


def action_transfer() -> dict[str, Any]:
    frames = 48
    geometry = [
        points([(22, 50), (22, 20), (14, 28), (22, 20), (30, 28)], name="Transfer up"),
        points([(37, 28), (37, 58), (29, 50), (37, 58), (45, 50)], name="Transfer down"),
    ]
    cue = cue_layers(
        20,
        "Transfer",
        geometry,
        frames,
        position=animated([(0, [55, 70]), (24, [55, 58]), (48, [55, 70])]),
        opacity=animated([(0, 38), (24, 100), (48, 38)]),
    )
    return lottie("action_transfer", frames, [*cursor_layers(frames), *cue])


def action_record() -> dict[str, Any]:
    frames = 48
    ring = cue_layers(
        20,
        "Record ring",
        [ellipse(29, 39, 34, 34, name="Record ring")],
        frames,
        position=static([52, 64]),
    )
    dot_scale = animated([(0, [72, 72]), (24, [114, 114]), (48, [72, 72])])
    dot_opacity = animated([(0, 42), (24, 100), (48, 42)])
    dot_geometry = [ellipse(29, 39, 10, 10, name="Record dot")]
    dot = [
        shape_layer(
            30,
            "Record dot outline",
            [*dot_geometry, fill(BLUE), stroke(WHITE, 3)],
            frames,
            position=static([52, 64]),
            scale=dot_scale,
            opacity=dot_opacity,
        )
    ]
    return lottie("action_record", frames, [*cursor_layers(frames), *ring, *dot])


def action_system() -> dict[str, Any]:
    frames = 48
    geometry = [
        ellipse(29, 39, 24, 24, name="System outer"),
        ellipse(29, 39, 8, 8, name="System inner"),
        *[
            points(coordinates, name=f"System tooth {index}")
            for index, coordinates in enumerate(
                [
                    [(29, 20), (29, 25)],
                    [(29, 53), (29, 58)],
                    [(10, 39), (15, 39)],
                    [(43, 39), (48, 39)],
                    [(16, 26), (20, 30)],
                    [(38, 48), (42, 52)],
                    [(16, 52), (20, 48)],
                    [(38, 30), (42, 26)],
                ]
            )
        ],
    ]
    cue = cue_layers(
        20,
        "System",
        geometry,
        frames,
        width=3.5,
        position=static([50, 64]),
        rotation=animated([(0, -18), (22, 50), (34, 0), (48, 0)]),
        opacity=animated([(0, 0), (6, 100), (34, 100), (44, 0), (48, 0)]),
    )
    return lottie("action_system", frames, [*cursor_layers(frames), *cue])


def all_animations() -> dict[str, dict[str, Any]]:
    return {
        "action_idle": action_idle(),
        "action_observe": action_observe(),
        "action_click": action_click(),
        "action_drag": action_drag(),
        "action_scroll": action_scroll(),
        "action_text": action_text(),
        "action_key": action_key(),
        "action_navigate": action_navigate(),
        "action_app": action_app(),
        "action_transfer": action_transfer(),
        "action_record": action_record(),
        "action_system": action_system(),
    }


def semantic_manifest() -> dict[str, Any]:
    actions = {
        name: {"animation": f"action_{name}", "still_frame": 0 if name == "idle" else 8}
        for name in [
            "idle",
            "observe",
            "click",
            "drag",
            "scroll",
            "text",
            "key",
            "navigate",
            "app",
            "transfer",
            "record",
            "system",
        ]
    }
    actions["click"]["still_frame"] = 8
    return {
        "schema": "cua.cursor-theme/2",
        "id": "cua.default",
        "name": "Cua Default",
        "version": "2.0.0",
        "author": "Cua",
        "license": "MIT",
        "compatibility": {"profile": "cua-driver-actions-v2", "semantics": 2},
        "canvas": {"width": CANVAS, "height": CANVAS, "fps": FPS},
        "hotspot": {"x": 55, "y": 30},
        "actions": actions,
    }


def encoded_json(value: Any) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def write_entry(archive: ZipFile, name: str, data: bytes) -> None:
    info = ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
    info.compress_type = ZIP_DEFLATED
    info.external_attr = 0o100644 << 16
    archive.writestr(info, data, compresslevel=9)


def build(output: Path) -> None:
    animations = all_animations()
    standard_manifest = {
        "version": "2",
        "generator": "Cua Driver default cursor generator",
        "animations": [
            {"id": animation_id, "name": animation_id}
            for animation_id in animations
        ],
    }
    output.parent.mkdir(parents=True, exist_ok=True)
    with ZipFile(output, "w") as archive:
        write_entry(archive, "manifest.json", encoded_json(standard_manifest))
        write_entry(archive, "cua/theme.json", encoded_json(semantic_manifest()))
        for animation_id, animation in animations.items():
            write_entry(archive, f"a/{animation_id}.json", encoded_json(animation))


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--output",
        type=Path,
        default=Path(__file__).with_name("cua.default.lottie"),
    )
    args = parser.parse_args()
    build(args.output.resolve())
    print(args.output.resolve())


if __name__ == "__main__":
    main()
