"""Live, multi-step agentic interface to the 13 real `cua-bench-basic` GUI
environments -- the raw RL surface, NOT the closed-option-set `CuaTask`
framing.

Use this module when you want a policy to actually *act* in the environment
over multiple steps and be scored by the environment's own real reward
function. Use `cua_bench_s1.datagen.cua_bench_basic` instead when you want
one-shot bounded-decision `CuaTask`s built from the same envs.

Everything here is a thin passthrough to a real
`cua_bench.environment.Environment`: `reset()`, `step(action)`, `evaluate()`
and `close()` call straight through to the live environment. The wrapper adds
exactly three things:

1. a hard `max_steps` cap (default `MAX_STEPS = 20`, this project's
   requirement) enforced as a truncation, so a rollout always terminates;
2. episode bookkeeping (step count, action history, terminal/truncated
   flags) and a single real success/failure signal per episode, taken from
   the env's own `@cb.evaluate_task` reward;
3. a provider override so rollouts can run on cua-bench's own `simulated`
   (Playwright) provider instead of the bundled envs' declared `native`
   (Docker/QEMU) provider.

### Honest note on the `simulated` provider

The bundled envs declare `provider: "native"`. `provider="simulated"` (the
default here, because it needs no Docker/QEMU/GPU) runs the real pages in
Playwright. Two real caveats:

1. That provider renders its window-content iframe ~150px tall whatever
   height the env asked for, clipping most of each page. `reset()` therefore
   applies `fit_window_layout` by default (`fit_layout=False` to opt out),
   which resizes the provider's own container iframe -- not the task page --
   to the size the env requested. Leave it on; with it off, most of each
   widget UI is neither visible nor clickable.
2. Even then, only 7 of the 13 envs' own reference solutions can earn reward
   under `simulated`: `click-button`, `click-icon`, `color-picker`,
   `right-click-menu`, `spreadsheet-cell`, `toggle-switch` and
   `typing-input` score 1.0 on every real parameterization, while
   `date-picker`, `drag-drop`, `fill-form`, `select-dropdown` and
   `video-player` score 0.0 on every one, and `drag-slider` on 4 of 5 --
   that provider cannot actuate native `<select>` popups, HTML5
   drag-and-drop, native date inputs or `<video>` playback. **Reward is
   effectively unreachable on those six, so do not train on them under
   `simulated`**: pass `provider="native"` (or `provider=None` to keep
   whatever the env declares) instead. `oracle_reward_check()` measures this
   for any env/parameterization you care about rather than making you trust
   this paragraph.

## Minimal usage

```python
import asyncio
from cua_bench_s1.agentic import CuaBenchBasicEnv
from cua_bench.types import ClickAction, DoneAction

async def main():
    async with CuaBenchBasicEnv(
        "click-button",
        dataset_dir="libs/cua-bench/datasets/cua-bench-basic",
        task_index=0,              # which real tasks_config parameterization
        provider="simulated",      # or "native" / None
        max_steps=20,
    ) as env:
        obs = await env.reset()            # obs.screenshot: PNG bytes
        print(obs.instruction, obs.metadata)

        while not obs.done:
            action = my_policy(obs)        # any cua_bench action, or a string
            obs = await env.step(action)   # e.g. ClickAction(x=72, y=145)

        print(obs.reward, obs.success)     # real env reward, real success flag

asyncio.run(main())
```

An element-grounded (rather than pixels-only) policy can ask the live page
for its real interactive elements and their real screen-space boxes:

```python
for el in await env.elements():
    print(el["role"], el["label"], el["frame"])   # e.g. Button 'Red color' [26,124,165,240]
```

A whole episode driven by a callable policy, in one call:

```python
from cua_bench_s1.agentic import rollout

result = asyncio.run(rollout(
    "typing-input",
    policy=my_async_policy,             # async (Observation) -> action
    dataset_dir="libs/cua-bench/datasets/cua-bench-basic",
    task_index=0,
))
print(result.success, result.reward, result.n_steps, result.actions)
```

`step()` accepts either a real `cua_bench.types.*Action` instance or a string
in cua-bench's own action-string formats (`"ClickAction(x=72, y=145)"` or
`"click(72, 145)"`), parsed by cua-bench's own
`cua_bench.actions.parse_action_string` -- so a text-generating policy can be
plugged in without the caller writing a parser.

Enumerate what is available with `list_task_variants(dataset_dir)`.
"""
from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Awaitable, Callable

#: Hard per-episode step cap for agentic rollouts on this family.
MAX_STEPS = 20

#: The 13 bundled `cua-bench-basic` env directory names.
ENV_NAMES = (
    "click-button",
    "click-icon",
    "color-picker",
    "date-picker",
    "drag-drop",
    "drag-slider",
    "fill-form",
    "right-click-menu",
    "select-dropdown",
    "spreadsheet-cell",
    "toggle-switch",
    "typing-input",
    "video-player",
)

#: Envs whose own reference solution does not earn reward under the
#: `simulated` (Playwright) provider. See the module docstring.
SIMULATED_UNSUPPORTED = (
    "date-picker",
    "drag-drop",
    "drag-slider",
    "fill-form",
    "select-dropdown",
    "video-player",
)


@dataclass
class StepResult:
    """One real observation from the live environment.

    `screenshot` is the real PNG bytes of the desktop after the action.
    `reward` is the environment's own real reward, and is only populated once
    the episode is over (cua-bench rewards are end-of-episode, not per-step);
    it is `None` mid-episode rather than a fabricated 0.0 shaping signal.
    """

    screenshot: bytes
    instruction: str
    metadata: dict
    step_count: int
    done: bool = False
    truncated: bool = False
    reward: float | None = None
    success: bool | None = None
    info: dict = field(default_factory=dict)


#: Alias: `reset()` and `step()` return the same observation type.
Observation = StepResult


@dataclass
class EpisodeResult:
    env_name: str
    task_index: int
    success: bool
    reward: float | None
    n_steps: int
    truncated: bool
    actions: list[str]
    error: str | None = None


def _default_dataset_dir() -> Path:
    """Best-effort location of the bundled dataset inside this monorepo."""
    here = Path(__file__).resolve()
    for parent in here.parents:
        candidate = parent / "libs" / "cua-bench" / "datasets" / "cua-bench-basic"
        if candidate.is_dir():
            return candidate
    raise FileNotFoundError(
        "could not locate libs/cua-bench/datasets/cua-bench-basic -- pass dataset_dir explicitly"
    )


def list_task_variants(dataset_dir: str | Path | None = None, *, split: str = "train") -> dict[str, int]:
    """`{env_name: n_parameterizations}` for every bundled env -- the real
    count of real `tasks_config` parameterizations, read from the envs
    themselves (no session or provider is started)."""
    from cua_bench.core import make

    root = Path(dataset_dir) if dataset_dir is not None else _default_dataset_dir()
    out: dict[str, int] = {}
    for name in ENV_NAMES:
        env_dir = root / name
        if not (env_dir / "main.py").exists():
            continue
        env = make(str(env_dir), split=split)
        out[name] = len(env.tasks_config_fn()) if env.tasks_config_fn else 0
    return out


def parse_action(action: Any) -> Any:
    """Passes a real cua-bench action through untouched; parses a string with
    cua-bench's own parser. Raises on anything else rather than guessing."""
    if isinstance(action, str):
        from cua_bench.actions import parse_action_string

        return parse_action_string(action)
    return action


class CuaBenchBasicEnv:
    """A capped, episode-tracking wrapper around one real `cua-bench-basic`
    environment. Not thread-safe; one instance drives one episode at a time.
    Always `close()` it (or use `async with`) -- it owns a real browser or
    sandbox session."""

    def __init__(
        self,
        env_name: str,
        *,
        dataset_dir: str | Path | None = None,
        task_index: int = 0,
        provider: str | None = "simulated",
        split: str = "train",
        max_steps: int = MAX_STEPS,
        headless: bool = True,
        fit_layout: bool = True,
    ) -> None:
        root = Path(dataset_dir) if dataset_dir is not None else _default_dataset_dir()
        self.env_dir = root / env_name if (root / env_name).is_dir() else root
        if not (self.env_dir / "main.py").exists():
            raise FileNotFoundError(f"no cua-bench env with a main.py at {self.env_dir}")
        self.env_name = env_name
        self.task_index = task_index
        self.provider = provider
        self.split = split
        self.max_steps = max_steps
        self.headless = headless
        self.fit_layout = fit_layout

        self._env: Any = None
        self._task: Any = None
        self.step_count = 0
        self.done = False
        self.truncated = False
        self.actions: list[str] = []

    # -- lifecycle -------------------------------------------------------
    async def reset(self, task_index: int | None = None) -> StepResult:
        """Starts a fresh real episode and returns the real initial state."""
        from cua_bench.core import make

        await self.close()
        if task_index is not None:
            self.task_index = task_index

        env = make(str(self.env_dir), split=self.split)
        env.headless = self.headless
        tasks = env.tasks_config_fn()
        if self.provider is not None:
            for t in tasks:
                if getattr(t, "computer", None):
                    t.computer = {**t.computer, "provider": self.provider}
        env.tasks = tasks
        env.current_task = tasks[self.task_index]
        # cua-bench raises MaxStepsExceeded past this budget; we truncate
        # before ever reaching it, and keep a slack of 1 so an over-eager
        # caller gets our clean truncation rather than that exception.
        env.max_steps = self.max_steps + 1

        screenshot, task_cfg = await env.reset(task_id=self.task_index)
        if self.fit_layout:
            # cua-bench's `simulated` provider renders its window-content
            # iframe ~150px tall regardless of the size the env asked for,
            # clipping most of every one of these widget pages. See
            # `cua_bench_s1.datagen.cua_bench_basic.fit_window_layout`.
            from ..datagen.cua_bench_basic import fit_window_layout

            pid = _first_window_pid(env.session)
            if pid is not None:
                await fit_window_layout(env.session, pid)
                screenshot = await env.session.screenshot()
        self._env = env
        self._task = task_cfg
        self.step_count = 0
        self.done = False
        self.truncated = False
        self.actions = []
        return StepResult(
            screenshot=screenshot,
            instruction=task_cfg.description,
            metadata=dict(task_cfg.metadata or {}),
            step_count=0,
            info={"env_name": self.env_name, "task_index": self.task_index,
                  "provider": self.provider, "max_steps": self.max_steps},
        )

    async def step(self, action: Any) -> StepResult:
        """Executes one real action. The episode ends on a real `DoneAction`
        or when `max_steps` is reached (truncation); either way the returned
        observation carries the environment's own real end-of-episode reward
        and a real success flag."""
        if self._env is None:
            raise RuntimeError("call reset() before step()")
        if self.done:
            raise RuntimeError("episode is over; call reset() to start a new one")

        act = parse_action(action)
        is_done_action = type(act).__name__ == "DoneAction"

        screenshot = await self._env.step(act)
        self.step_count += 1
        self.actions.append(repr(act))

        self.truncated = (not is_done_action) and self.step_count >= self.max_steps
        self.done = is_done_action or self.truncated

        reward: float | None = None
        success: bool | None = None
        if self.done:
            reward = await self.evaluate()
            success = reward is not None and reward >= 0.5

        return StepResult(
            screenshot=screenshot,
            instruction=self._task.description,
            metadata=dict(self._task.metadata or {}),
            step_count=self.step_count,
            done=self.done,
            truncated=self.truncated,
            reward=reward,
            success=success,
            info={"action": repr(act), "env_name": self.env_name},
        )

    async def elements(self) -> list[dict]:
        """The real interactive elements of the live page right now, with real
        screen-space boxes (`{"id","role","label","frame","tag","dom_id"}`) --
        the same live-DOM query the `cua_bench_basic` datagen uses. Optional:
        a pixels-only policy never needs it, but a text-mode or
        element-grounded policy would otherwise have to re-derive coordinates
        from the screenshot. Returns `[]` if the provider exposes no such
        query (e.g. `native`)."""
        if self._env is None:
            raise RuntimeError("call reset() before elements()")
        from dataclasses import asdict

        from ..datagen.cua_bench_basic import (
            INTERACTIVE_ELEMENTS_JS,
            dom_elements_from_query,
        )

        session = self._env.session
        pid = _first_window_pid(session)
        if pid is None:
            return []
        try:
            raw = await session.execute_javascript(pid, INTERACTIVE_ELEMENTS_JS)
            win = await session.get_element_rect(pid, "body", space="window")
            scr = await session.get_element_rect(pid, "body", space="screen")
        except Exception:
            return []
        off = (int(scr["x"]) - int(win["x"]), int(scr["y"]) - int(win["y"])) if win and scr else (0, 0)
        return [
            asdict(e) for e in dom_elements_from_query(raw or [], offset_x=off[0], offset_y=off[1])
        ]

    async def evaluate(self) -> float | None:
        """The environment's own real reward for the current episode state,
        normalized to a float (cua-bench evaluators return `[1.0]`/`[0.0]`,
        a bare number, or a dict). `None` if the env reports nothing
        numeric."""
        if self._env is None:
            raise RuntimeError("call reset() before evaluate()")
        return _scalar_reward(await self._env.evaluate())

    async def solve(self) -> float | None:
        """Runs the env's own bundled reference solution and returns its real
        reward -- the oracle upper bound for this parameterization, useful as
        an RL sanity check that the env is solvable under this provider."""
        if self._env is None:
            raise RuntimeError("call reset() before solve()")
        await self._env.solve()
        self.done = True
        return await self.evaluate()

    async def close(self) -> None:
        if self._env is not None:
            try:
                await self._env.close()
            finally:
                self._env = None
                self._task = None

    async def __aenter__(self) -> "CuaBenchBasicEnv":
        return self

    async def __aexit__(self, exc_type, exc, tb) -> None:
        await self.close()


def _first_window_pid(session: Any) -> str | None:
    """The provider's own pid for the single task window these envs launch."""
    mapping = getattr(session, "_pid_to_index", None)
    if isinstance(mapping, dict) and mapping:
        return str(next(iter(mapping)))
    return None


def _scalar_reward(result: Any) -> float | None:
    if isinstance(result, (list, tuple)):
        vals = [v for v in result if isinstance(v, (int, float))]
        return float(sum(vals) / len(vals)) if vals else None
    if isinstance(result, bool):
        return 1.0 if result else 0.0
    if isinstance(result, (int, float)):
        return float(result)
    if isinstance(result, dict):
        for k in ("reward", "score", "success"):
            if k in result:
                return _scalar_reward(result[k])
    return None


async def rollout(
    env_name: str,
    *,
    policy: Callable[[StepResult], Awaitable[Any]],
    dataset_dir: str | Path | None = None,
    task_index: int = 0,
    provider: str | None = "simulated",
    split: str = "train",
    max_steps: int = MAX_STEPS,
) -> EpisodeResult:
    """Drives one full real episode with an async `policy(observation) ->
    action` and returns the real per-episode success/failure signal.

    A policy exception is caught and reported in `EpisodeResult.error` with
    `success=False`, rather than crashing a training loop mid-batch -- but it
    is never silently swallowed.
    """
    env = CuaBenchBasicEnv(
        env_name,
        dataset_dir=dataset_dir,
        task_index=task_index,
        provider=provider,
        split=split,
        max_steps=max_steps,
    )
    error: str | None = None
    obs: StepResult | None = None
    try:
        obs = await env.reset()
        while not obs.done:
            obs = await env.step(await policy(obs))
    except Exception as e:  # noqa: BLE001 - reported, never hidden
        error = f"{type(e).__name__}: {e}"
    finally:
        reward = obs.reward if obs is not None else None
        if error is None and obs is not None and obs.reward is None:
            reward = await env.evaluate()
        await env.close()

    return EpisodeResult(
        env_name=env_name,
        task_index=task_index,
        success=bool(reward is not None and reward >= 0.5),
        reward=reward,
        n_steps=env.step_count,
        truncated=env.truncated,
        actions=list(env.actions),
        error=error,
    )


async def oracle_reward_check(
    env_name: str,
    *,
    dataset_dir: str | Path | None = None,
    task_index: int = 0,
    provider: str | None = "simulated",
    split: str = "train",
) -> float | None:
    """Real measurement of whether this env's own reference solution actually
    earns reward under `provider` -- i.e. whether reward is reachable at all
    for an RL run configured this way. Returns the real reward."""
    env = CuaBenchBasicEnv(
        env_name, dataset_dir=dataset_dir, task_index=task_index,
        provider=provider, split=split,
    )
    try:
        await env.reset()
        return await env.solve()
    finally:
        await env.close()
