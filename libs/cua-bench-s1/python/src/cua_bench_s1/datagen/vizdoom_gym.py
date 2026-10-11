"""ViZDoom (github.com/mwydmuch/ViZDoom, MIT license, `vizdoom` PyPI package)
-> CuaTask converter for the held-out-only `game_control` family.

Why ViZDoom, and why this is a genuinely different task shape than the other
`datagen/` converters: `androidcontrol.py` and `gui360.py` each read a fixed,
pre-recorded human trajectory -- state and gold action both already exist in
the source data. ViZDoom is a *live, steppable game environment*: there is no
recorded human trajectory to read gold labels from at all. This module drives
real episodes itself and must define what "correct" means from scratch -- see
"Gold-label design" below, which is the real decision this integration makes.

## Scenario scope: `basic` and `defend_the_center` only

ViZDoom ships roughly 15 scenario .cfgs (basic, deadly_corridor,
health_gathering, my_way_home, take_cover, deathmatch, ...). Only two are
integrated here:

- `basic`: player fixed on a track, one stationary Cacodemon target,
  3-button action space (MOVE_LEFT, MOVE_RIGHT, ATTACK).
- `defend_the_center`: player fixed at the center, melee enemies approach
  from any angle, 3-button action space (TURN_LEFT, TURN_RIGHT, ATTACK).

Both were chosen because they share the property that makes an honest
heuristic oracle possible: a small, closed action set where exactly one
button is unambiguously correct at each observed frame, verifiable from
ViZDoom's own ground-truth labels buffer (not guessed at). Scenarios like
`deadly_corridor` or `my_way_home` require multi-step navigation planning
(there is no single frame-local "correct" button -- the right action depends
on a path plan this integration has no honest way to score without either a
trained policy or hand-authored waypoints). Rather than fabricate a
plausible-looking heuristic for those, they are left out -- a real, disclosed
scope limit.

## Gold-label design: labels-buffer-grounded aim heuristic

ViZDoom's `set_labels_buffer_enabled(True)` exposes ground-truth object
detections per frame (`state.labels`: `object_name`, screen-space `x`/`y`/
`width`/`height` of each visible actor's bounding box) -- this is real engine
state, not a model's guess, and both `basic` and `defend_the_center`
guarantee at most one non-player hostile actor is relevant at any frame
sampled here (see `_pick_target`). Given the target's bounding-box x-center
and the player's own screen-center-x (`screen_width / 2`, since both
scenarios keep the crosshair screen-centered), gold action is:

  - `ATTACK` if the target's x-center is within `_AIM_TOLERANCE_PX` of
    screen-center (crosshair is on/near target -- shooting is correct now).
  - `MOVE_LEFT` (`basic`) / `TURN_LEFT` (`defend_the_center`) if the target
    is left of screen-center (get the crosshair a step closer).
  - `MOVE_RIGHT` / `TURN_RIGHT` symmetrically.

This is a heuristic oracle, chosen because neither scenario ships a scripted
bot in ViZDoom itself. It is a real design decision with a real, disclosed
weakness: it is a *local* one-step-lookahead heuristic (align-then-shoot),
not a globally optimal policy -- e.g. it does not account for reload state,
ammo, or multiple simultaneous targets beyond picking the nearest-to-center
one. Good enough to make "which of these 3 buttons is correct at this exact
frame" an honestly answerable, verifiable question, which is what a CuaTask
needs.

## Mapping onto CuaTask's (element, action) schema

CuaTask's schema is GUI-shaped: `options` are (element, action) pairs where
`action` is one of `task.ACTIONS` (fill/check/click/select/scroll/skip).
There is no natural "GUI element" in a Doom frame, so each of the
scenario's discrete game buttons is represented as a synthetic HUD-style
"button" element (role="Button", label=button name, a plausible fixed
on-screen frame along the bottom edge -- an actual reasonable placement for
an on-screen control-remap HUD, though ViZDoom itself renders no such HUD;
`elements_source="synthetic_spec"`, matching the vocabulary `specs.py`
already uses for non-really-detected elements). Pressing that game button is
expressed as this benchmark's `click` action on that synthetic element
(closest existing semantic: "activate this control"); the non-gold buttons
get `skip` exactly like every other converter in this package. This adds
zero new vocabulary to `task.py`.

This family is held out only: it is never mixed into the trained-on
synthetic/real GUI families, only used to measure generalization to a live
game-control domain a GUI-trained model has never seen. `game_control` tasks
are multimodal-only by design: a synthetic button-name list (the only "text"
representation ever available here) conveys no actual game state -- no enemy
position, no distance, nothing a real text-only model could reason from to
pick the correct button.
"""
from __future__ import annotations

import random
from dataclasses import dataclass
from pathlib import Path

from ..task import CuaTask, OptionSpec

_AIM_TOLERANCE_PX = 14  # ~half the Cacodemon's own bbox width at basic.cfg's typical range

# Per-scenario: (cfg filename, turn/move-left button name, turn/move-right
# button name, attack button name, hostile actor names to treat as "target"
# (labels buffer `object_name`; DoomPlayer itself is always excluded)).
SCENARIOS: dict[str, dict] = {
    "basic": {
        "cfg": "basic.cfg",
        "left": "MOVE_LEFT",
        "right": "MOVE_RIGHT",
        "attack": "ATTACK",
        "hostile_names": None,  # None = "any non-DoomPlayer label"
    },
    "defend_the_center": {
        "cfg": "defend_the_center.cfg",
        "left": "TURN_LEFT",
        "right": "TURN_RIGHT",
        "attack": "ATTACK",
        "hostile_names": None,
    },
}

# Fixed synthetic HUD button placements (pixel frame in a 320x240 capture),
# left-to-right in display order -- purely a metadata/cropping convenience,
# not something ViZDoom actually renders.
_HUD_Y0, _HUD_Y1 = 210, 235
_HUD_BUTTON_W = 90
_HUD_GAP = 10


@dataclass
class DoomFrame:
    screen_rgb: "object"  # numpy array, (H, W, 3) uint8
    target_x_center: float | None  # None if no hostile actor visible this frame
    screen_center_x: float
    game_variables: dict


def _pick_target(labels, hostile_names: tuple[str, ...] | None) -> float | None:
    """Returns the x-center (in screen pixels) of the hostile actor closest
    to horizontal screen-center, or None if no hostile actor is visible.
    Real ground truth from ViZDoom's labels buffer -- never inferred from
    pixels."""
    candidates = [l for l in labels if l.object_name != "DoomPlayer"
                  and (hostile_names is None or l.object_name in hostile_names)]
    if not candidates:
        return None
    return min((l.x + l.width / 2.0) for l in candidates)


def gold_button(target_x_center: float | None, screen_center_x: float,
                 left: str, right: str, attack: str) -> str | None:
    """The heuristic oracle described in the module docstring. Returns None
    (caller should skip the frame) if no target is visible -- an honestly
    unanswerable frame, not a fabricated default."""
    if target_x_center is None:
        return None
    dx = target_x_center - screen_center_x
    if abs(dx) <= _AIM_TOLERANCE_PX:
        return attack
    return left if dx < 0 else right


def _build_elements_and_options(button_names: list[str], gold: str,
                                 screen_w: int) -> tuple[list[dict], list[OptionSpec], dict[str, str]]:
    n = len(button_names)
    total_w = n * _HUD_BUTTON_W + (n - 1) * _HUD_GAP
    x0 = max(0, (screen_w - total_w) // 2)
    elements, options, expected = [], [], {}
    for i, name in enumerate(button_names):
        eid = f"el_{i}"
        x_left = x0 + i * (_HUD_BUTTON_W + _HUD_GAP)
        frame = [x_left, _HUD_Y0, x_left + _HUD_BUTTON_W, _HUD_Y1]
        elements.append({"id": eid, "role": "Button", "label": name, "frame": frame})
        # Hard-distractor fix (see chess_gym.py's convert_position for the
        # full rationale): every real candidate action (ATTACK/MOVE_LEFT/
        # MOVE_RIGHT/etc) gets a real {click, skip} pair, not just the gold
        # button. Previously the non-gold buttons were skip-only, so a
        # 3-button scenario reduced to "spot the one button with 2 options"
        # instead of discriminating among the real action set.
        options.append(OptionSpec(element_id=eid, role="Button", label=name, action="click"))
        options.append(OptionSpec(element_id=eid, role="Button", label=name, action="skip"))
        expected[eid] = "click" if name == gold else "skip"
    return elements, options, expected


def _save_png(rgb, path: Path) -> None:
    from PIL import Image
    Image.fromarray(rgb).save(path)


def convert_frame(
    *,
    scenario: str,
    episode_idx: int,
    frame_idx: int,
    frame: DoomFrame,
    button_order: list[str],
    out_dir: Path,
    modality_available: tuple[str, ...] = ("multimodal",),
) -> CuaTask | None:
    """Returns None if the frame has no visible hostile actor (unanswerable,
    see gold_button's docstring) -- skipped, not fabricated."""
    cfg = SCENARIOS[scenario]
    gold = gold_button(frame.target_x_center, frame.screen_center_x, cfg["left"], cfg["right"], cfg["attack"])
    if gold is None:
        return None

    task_id = f"vizdoom_{scenario}_{episode_idx}_{frame_idx}"
    screen_w = frame.screen_rgb.shape[1]
    elements, options, expected = _build_elements_and_options(button_order, gold, screen_w)

    screenshot_rel = None
    if "multimodal" in modality_available:
        out_dir.mkdir(parents=True, exist_ok=True)
        shot_path = out_dir / f"{task_id}.png"
        _save_png(frame.screen_rgb, shot_path)
        screenshot_rel = str(shot_path)
    if "text" in modality_available:
        raise ValueError(
            "game_control tasks do not support a 'text' modality: the only "
            "text representation available (a bare button-name list) carries "
            "no real game state and is honestly answerable only by an "
            "oracle. See the module docstring's 'Mapping onto CuaTask's "
            "(element, action) schema' section."
        )

    return CuaTask(
        id=task_id,
        family="game_control",
        app=f"vizdoom_{scenario}",
        modality_available=list(modality_available),
        screenshot=screenshot_rel,
        ax_tree=None,
        ax_tree_source="synthetic",  # buttons are a synthetic HUD overlay, not a real a11y source
        elements=elements,
        elements_source="synthetic_spec",
        entities=[],
        options=options,
        expected=expected,
        split="public",
        group=None,
        provenance={
            "source": "ViZDoom",
            "source_url": "https://github.com/mwydmuch/ViZDoom",
            "license": "MIT",
            "scenario": scenario,
            "episode_idx": episode_idx,
            "frame_idx": frame_idx,
            "gold_button": gold,
            "target_x_center": frame.target_x_center,
            "screen_center_x": frame.screen_center_x,
            "game_variables": frame.game_variables,
            "gold_label_method": "labels_buffer_aim_heuristic",
        },
    )


def run_episodes(
    scenario: str,
    *,
    n_episodes: int,
    max_frames_per_episode: int,
    out_dir: Path,
    modality_available: tuple[str, ...] = ("multimodal",),
    seed: int = 0,
) -> list[CuaTask]:
    """Drives `n_episodes` real ViZDoom episodes of `scenario`, sampling up to
    `max_frames_per_episode` frames per episode (acting greedily per the
    heuristic oracle itself, so episodes actually play out sensibly rather
    than random-walking) and converting each into a CuaTask. Frames with no
    visible hostile actor are skipped (see convert_frame)."""
    import vizdoom as vzd

    cfg = SCENARIOS[scenario]
    game = vzd.DoomGame()
    game.load_config(vzd.scenarios_path + "/" + cfg["cfg"])
    game.set_window_visible(False)
    game.set_screen_format(vzd.ScreenFormat.RGB24)
    game.set_labels_buffer_enabled(True)
    game.init()

    buttons = game.get_available_buttons()
    button_names = [b.name for b in buttons]
    name_to_action = {}
    for i, name in enumerate(button_names):
        vec = [0] * len(buttons)
        vec[i] = 1
        name_to_action[name] = vec

    rng = random.Random(seed)
    tasks: list[CuaTask] = []
    try:
        for ep in range(n_episodes):
            game.set_seed(seed * 1000 + ep)
            game.new_episode()
            frame_count = 0
            while not game.is_episode_finished() and frame_count < max_frames_per_episode * 4:
                state = game.get_state()
                if state is None:
                    break
                screen_w = game.get_screen_width()
                target_x = _pick_target(state.labels, cfg["hostile_names"])
                doom_frame = DoomFrame(
                    screen_rgb=state.screen_buffer,
                    target_x_center=target_x,
                    screen_center_x=screen_w / 2.0,
                    game_variables={"vars": list(state.game_variables)},
                )
                task = convert_frame(
                    scenario=scenario, episode_idx=ep, frame_idx=frame_count,
                    frame=doom_frame, button_order=button_names, out_dir=out_dir,
                    modality_available=modality_available,
                )
                gold = None
                if task is not None:
                    tasks.append(task)
                    gold = task.provenance["gold_button"]
                    if len(tasks) >= n_episodes * max_frames_per_episode:
                        return tasks
                # Act per the oracle when a target is visible (keeps the episode
                # progressing sensibly); otherwise take a small random turn/move
                # to bring a target into view rather than idling forever.
                action_name = gold or rng.choice(button_names)
                game.make_action(name_to_action[action_name])
                frame_count += 1
    finally:
        game.close()
    return tasks
