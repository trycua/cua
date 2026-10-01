"""``cb interact``: open a task's sandbox, run its setup and hand it to you."""

import asyncio
import os
import shutil
import time
import webbrowser
from pathlib import Path
from typing import Any, Optional

# ANSI colors
RESET = "\033[0m"
BOLD = "\033[1m"
CYAN = "\033[36m"
GREEN = "\033[92m"
YELLOW = "\033[33m"
RED = "\033[91m"
GREY = "\033[90m"


def execute(args):
    """Execute the interact command."""
    return asyncio.run(_execute_async(args))


#: How long ``cb interact`` waits for the display page to answer.
DISPLAY_PROBE_ATTEMPTS = 8
DISPLAY_PROBE_INTERVAL_S = 2.0


def _wants_browser(args) -> bool:
    """Open the display unless --no-browser, --no-wait or CUA_BENCH_NO_BROWSER=1."""
    if getattr(args, "no_browser", False) or getattr(args, "no_wait", False):
        return False
    return os.environ.get("CUA_BENCH_NO_BROWSER", "").strip().lower() not in ("1", "true", "yes")


async def _probe_display(url: str) -> Optional[str]:
    """``None`` once the display page answers, else the last problem seen.

    Bounded: at most ``DISPLAY_PROBE_ATTEMPTS`` requests.
    """
    import httpx

    problem: Optional[str] = None
    async with httpx.AsyncClient(timeout=5.0, follow_redirects=True) as client:
        for attempt in range(DISPLAY_PROBE_ATTEMPTS):
            try:
                response = await client.get(url)
                if response.status_code < 400:
                    return None
                problem = f"HTTP {response.status_code}"
            except httpx.HTTPError as error:
                problem = f"{type(error).__name__}: {error}" if str(error) else type(error).__name__
            if attempt + 1 < DISPLAY_PROBE_ATTEMPTS:
                await asyncio.sleep(DISPLAY_PROBE_INTERVAL_S)
    return problem


async def _show_display(sandbox: Any, *, open_browser: bool) -> Optional[str]:
    """Print the sandbox's ``Display:`` URL (the cua-spacesd viewer link, or
    a legacy image's web display or VNC address) and open it; report why not."""
    try:
        display = await sandbox.get_display_url()
    except NotImplementedError as error:
        print(f"{YELLOW}Display: not available ({error}){RESET}")
        return None
    except Exception as error:  # noqa: BLE001 - reported, the task still runs
        print(f"{RED}Display: could not get the display URL: {type(error).__name__}: {error}{RESET}")
        return None
    if not display:
        print(f"{YELLOW}Display: not available (the sandbox reports no display){RESET}")
        return None
    print(f"{CYAN}Display: {BOLD}{display}{RESET}")
    web = display.startswith(("http://", "https://"))
    if web:
        problem = await _probe_display(display)
        if problem:
            print(
                f"{YELLOW}  The display is not answering ({problem}); the image may not "
                f"run cua-spacesd (its viewer is served on port 3211).{RESET}"
            )
    if open_browser and web:
        try:
            opened = webbrowser.open(display)
        except Exception:  # noqa: BLE001 - no browser is not an error
            opened = False
        if not opened:
            print(f"{GREY}  No browser could be opened; open the URL above.{RESET}")
    return display


async def _screenshot_or_none(session):
    try:
        return await asyncio.wait_for(session.screenshot(), 20)
    except Exception:  # noqa: BLE001 - not every image has a screen
        return None


async def _execute_async(args):
    """Execute the interact command asynchronously."""
    from .registry import resolve_dataset

    default_image = None
    # Handle --dataset flag: resolve from registry
    if getattr(args, "dataset", None):
        dataset_dir, default_image = resolve_dataset(args.dataset)
        env_path = dataset_dir / args.env_path if dataset_dir else None
        if env_path is None or not env_path.exists():
            print(
                f"{RED}Error: Task '{args.env_path}' not found in dataset '{args.dataset}'{RESET}"
            )
            return 1
    elif getattr(args, "dataset_path", None):
        # Handle --dataset-path flag: resolve task from local dataset directory
        dataset_path = Path(args.dataset_path)
        if not dataset_path.exists():
            print(f"{RED}Error: Dataset path not found: {dataset_path}{RESET}")
            return 1

        # Resolve the env path from the dataset directory
        env_path = dataset_path / args.env_path
        if not env_path.exists():
            print(
                f"{RED}Error: Environment '{args.env_path}' not found in dataset path '{args.dataset_path}' at {env_path}{RESET}"
            )
            return 1
    else:
        env_path = Path(args.env_path)

        if not env_path.exists():
            print(f"{RED}Error: Environment not found: {env_path}{RESET}")
            return 1

    return await _execute_native_interactive(args, env_path, default_image=default_image)


async def _execute_native_interactive(
    args, env_path: Path, default_image: Optional[str] = None
) -> int:
    """Open the task's sandbox (local or cloud), run setup and hand it to the user."""
    from cua_bench import make
    from cua_bench.computers.remote import RemoteDesktopSession
    from cua_bench.sandboxes import CloudAuthError, cloud_auth_source, explain_error, open_sandbox
    from cua_bench.targets import TargetError, resolve_env_spec

    from .run import target_from_args

    task_index = getattr(args, "variant_id", 0) or 0
    spec = None
    try:
        target = target_from_args(args)
        env = make(str(env_path))
        tasks = env.tasks_config_fn() if env.tasks_config_fn else []
        if task_index >= len(tasks):
            print(f"{RED}Error: variant {task_index} out of range ({len(tasks)}){RESET}")
            return 1
        task_cfg = tasks[task_index]
        from cua_bench.sandboxes import cached_index_runtime

        spec = resolve_env_spec(
            getattr(task_cfg, "computer", None),
            target,
            variant_resolver=cached_index_runtime(),
            default_image=default_image,
        )
        from cua_bench.targets import check_requirements

        check_requirements([spec], target)
        if target.cloud:
            cloud_auth_source()
    except (TargetError, CloudAuthError) as error:
        print(f"{RED}{error}{RESET}")
        return 1

    print(
        f"{CYAN}Starting {spec.os_type} {spec.kind} on {target.on} "
        f"({spec.backend(target.on)}): {spec.image_label}{RESET}"
    )

    def on_progress(event) -> None:
        if event.stage in ("cold_start", "ready", "claim"):
            print(f"{GREY}  {event.message}{RESET}")

    trace_out = getattr(args, "trace_out", None)
    # A --view trace without --trace-out goes to a bounded directory (the
    # newest few are kept), and is removed again when the run fails.
    managed_trace = None
    if getattr(args, "view", False) and not trace_out:
        from cua_bench import retention

        managed_trace = retention.new_interact_trace_dir()
        trace_out = str(managed_trace)
    if trace_out:
        try:
            print(f"{GREY}Tracing started. trajectory_id={env.tracing.start()}{RESET}")
        except Exception:  # noqa: BLE001
            trace_out = None
    if getattr(args, "max_steps", None) is not None:
        env.max_steps = int(args.max_steps)

    try:
        async with open_sandbox(spec, target, on_progress=on_progress) as sandbox:
            session = RemoteDesktopSession.attach(
                sandbox, os_type=spec.os_type, width=spec.width, height=spec.height
            )
            session.env = env
            env.session = session
            env.current_task = task_cfg
            print(f"\n{BOLD}Task: {task_cfg.description}{RESET}")
            from cua_bench.runner.desktop import wait_for_desktop

            await wait_for_desktop(session)
            t0 = time.perf_counter()
            if env.setup_task_fn:
                await env.setup_task_fn(task_cfg, session)
            print(f"{GREEN}✓ Setup complete in {time.perf_counter() - t0:.2f}s{RESET}")
            if trace_out:
                shot = await _screenshot_or_none(session)
                env.tracing.record("reset", {"task": repr(task_cfg)}, [shot] if shot else [])
            if getattr(args, "oracle", False) and env.solve_task_fn:
                await env.solve_task_fn(task_cfg, session)
                print(f"{GREEN}✓ Oracle solution ran{RESET}")

            await _show_display(sandbox, open_browser=_wants_browser(args))

            if getattr(args, "screenshot", None):
                Path(args.screenshot).write_bytes(await session.screenshot())
                print(f"{GREEN}✓ Screenshot saved to {args.screenshot}{RESET}")

            if not getattr(args, "no_wait", False):
                print(f"\n{GREY}Sandbox is open. Press Enter to evaluate and release it...{RESET}")
                await asyncio.get_running_loop().run_in_executor(None, input)

            if env.evaluate_task_fn:
                result = await env.evaluate_task_fn(task_cfg, session)
                print(f"{YELLOW}✓ Evaluation result: {BOLD}{result}{RESET}")
                if trace_out:
                    env.tracing.record("evaluate", {"result": result})
        print(f"{GREEN}✓ Sandbox released{RESET}")
        if trace_out:
            env.tracing.save_to_disk(str(trace_out))
            print(f"{GREEN}✓ Trace saved to: {trace_out}{RESET}")
        return 0
    except Exception as error:  # noqa: BLE001
        print(f"{RED}Error: {explain_error(error, spec, target)}{RESET}")
        if managed_trace is not None:
            shutil.rmtree(managed_trace, ignore_errors=True)
        return 1
