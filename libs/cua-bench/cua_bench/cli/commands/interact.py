"""Interact command - Interactively run a task with browser visible."""

import os
import tempfile
from types import SimpleNamespace
from pathlib import Path
import time
import asyncio

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

def _detect_provider_type(env_path: Path) -> str:
    """Detect provider type from task configuration.

    Args:
        env_path: Path to task environment directory

    Returns:
        Provider type ("simulated", "webtop", "native", "computer", or "unknown")
    """
    import sys
    import importlib.util

    # Try to import and inspect the task
    try:
        # Load main.py module
        main_file = env_path / "main.py"
        if not main_file.exists():
            return "unknown"

        spec = importlib.util.spec_from_file_location("env_module", main_file)
        if spec is None or spec.loader is None:
            return "unknown"

        module = importlib.util.module_from_spec(spec)

        # Add env_path to Python path temporarily for imports
        sys.path.insert(0, str(env_path))
        try:
            spec.loader.exec_module(module)

            # Look for tasks_config function
            for name in dir(module):
                obj = getattr(module, name)
                if callable(obj) and hasattr(obj, "_td_type"):
                    if getattr(obj, "_td_type") == "tasks_config":
                        # Call the function to get tasks
                        tasks = obj()
                        if tasks and len(tasks) > 0:
                            task = tasks[0]
                            if hasattr(task, "computer") and task.computer:
                                provider = task.computer.get("provider", "unknown")
                                return provider

            return "unknown"

        finally:
            sys.path.pop(0)
            # Clean up module
            if 'env_module' in sys.modules:
                del sys.modules['env_module']

    except Exception as e:
        print(f"{YELLOW}Warning: Could not detect provider type: {e}{RESET}")
        return "unknown"


def _get_env_type_from_provider(provider: str) -> str:
    """Map provider type to env_type for task runner.

    Args:
        provider: Provider type (simulated, webtop, native, computer)

    Returns:
        env_type for task runner (linux-docker, windows-qemu, etc.)
    """
    # For native providers, default to linux-docker
    # Users can override via platform detection or args
    if provider in ("native", "computer"):
        return "linux-docker"
    return "linux-docker"


async def _execute_async(args):
    """Execute the interact command asynchronously."""
    from .registry import resolve_task_path

    # Handle --dataset flag: resolve from registry
    if getattr(args, 'dataset', None):
        env_path = resolve_task_path(args.dataset, args.env_path)
        if env_path is None:
            print(f"{RED}Error: Task '{args.env_path}' not found in dataset '{args.dataset}'{RESET}")
            return 1
    elif getattr(args, 'dataset_path', None):
        # Handle --dataset-path flag: resolve task from local dataset directory
        dataset_path = Path(args.dataset_path)
        if not dataset_path.exists():
            print(f"{RED}Error: Dataset path not found: {dataset_path}{RESET}")
            return 1
        
        # Resolve the env path from the dataset directory
        env_path = dataset_path / args.env_path
        if not env_path.exists():
            print(f"{RED}Error: Environment '{args.env_path}' not found in dataset path '{args.dataset_path}' at {env_path}{RESET}")
            return 1
    else:
        env_path = Path(args.env_path)

        if not env_path.exists():
            print(f"{RED}Error: Environment not found: {env_path}{RESET}")
            return 1

    # Detect provider type
    provider_type = _detect_provider_type(env_path)
    print(f"{GREY}Detected provider: {provider_type}{RESET}")

    # Use different execution paths based on provider type
    if provider_type in ("native", "computer"):
        # Native provider - use task runner for 2-container architecture
        return await _execute_native_interactive(args, env_path, provider_type)
    else:
        # Simulated provider - use legacy make() approach
        return await _execute_simulated_interactive(args, env_path)


async def _execute_simulated_interactive(args, env_path: Path) -> int:
    """Execute interactive mode for simulated tasks (webtop/Playwright)."""
    try:
        from cua_bench import make

        print(f"{CYAN}Loading environment: {env_path}{RESET}")
        env = make(str(env_path))

        # Set headless to False for interactive mode
        env.headless = False
        env.print_actions = True
        # Apply max steps if provided
        if getattr(args, 'max_steps', None) is not None:
            env.max_steps = int(args.max_steps)
            print(f"{GREY}Max steps set to {env.max_steps}{RESET}")

        # Determine trace behavior
        tmp_trace_dir: Path | None = None
        want_trace = bool(getattr(args, 'trace_out', None) or getattr(args, 'view', False))
        if want_trace and getattr(args, 'trace_out', None) is None:
            tmp_trace_dir = Path(tempfile.mkdtemp(prefix="cua_trace_"))
            args.trace_out = str(tmp_trace_dir)
        # Start tracing if requested (either --trace-out or --view-trace)
        if want_trace:
            try:
                tid = env.tracing.start()
                print(f"{GREY}Tracing started. trajectory_id={tid}{RESET}")
            except Exception:
                print(f"{RED}Failed to start tracing; continuing without trace.{RESET}")

        print(f"{CYAN}Running task {args.variant_id} (interactive mode - browser will be visible)...{RESET}")
        _t0 = time.perf_counter()
        screenshot, task_cfg = await env.reset(task_id=args.variant_id)
        _elapsed = time.perf_counter() - _t0

        # Print task description
        if hasattr(task_cfg, 'description') and task_cfg.description:
            print(f"\n{BOLD}Task: {task_cfg.description}{RESET}")

        # Bold the setup time only
        print(
            f"{GREEN}✓ Setup complete in {BOLD}{_elapsed:.2f}s{RESET}{GREEN} "
            f"(screenshot: {len(screenshot)} bytes){RESET}"
        )

        if args.oracle:
            print(f"{YELLOW}Running solution...{RESET}")
            screenshot = await env.solve()
            print(f"{GREEN}✓ Solution complete (screenshot: {len(screenshot)} bytes){RESET}")

        # Save screenshot if requested
        if args.screenshot:
            screenshot_path = Path(args.screenshot)
            screenshot_path.write_bytes(screenshot)
            print(f"{GREEN}✓ Screenshot saved to: {screenshot_path}{RESET}")

        # Keep preview open for interaction (unless --no-wait)
        if not getattr(args, 'no_wait', False):
            print(f"\n{GREY}Environment is open. Press Enter to close...{RESET}")
            input()

        # Evaluate if function exists
        if env.evaluate_task_fn:
            print(f"{CYAN}Running evaluation...{RESET}")
            result = await env.evaluate()
            print(f"{YELLOW}✓ Evaluation result: {BOLD}{result}{RESET}")

        # Save trace if requested
        if getattr(args, 'trace_out', None) and getattr(env, 'tracing', None) and env.tracing.trajectory_id:
            try:
                out_path = Path(args.trace_out)
                env.tracing.save_to_disk(str(out_path))
                print(f"{GREEN}✓ Trace saved to: {out_path}{RESET}")
                # If --view-trace, open the viewer
                if getattr(args, 'view', False):
                    try:
                        from . import view_trace as _view_trace
                        print(f"{CYAN}Opening trace viewer...{RESET}")
                        _view_trace.execute(SimpleNamespace(path=str(out_path)))
                    except Exception as _e:
                        print(f"{RED}Failed to open trace viewer: {_e}{RESET}")
            except Exception as _e:
                print(f"{RED}Failed to save trace: {_e}{RESET}")

        await env.close()
        print(f"\n{GREEN}✓ Task completed successfully!{RESET}")
        try:
            if getattr(args, 'trace_out', None):
                out_path = Path(args.trace_out)
                print(f"\n{CYAN}Next steps:{RESET}")
                print(f"  • {BOLD}View trace{RESET}:\n     {YELLOW}cb{RESET} view-trace {out_path}")
                print(f"  • {BOLD}Create replay environment{RESET}:\n     {YELLOW}cb{RESET} create-replay {out_path}")
                print(f"  • {BOLD}Process outputs{RESET}:\n     {YELLOW}cb{RESET} process {out_path.parent} {GREY}--mode aguvis --save-dir ./outputs/processed{RESET}")
        except Exception:
            pass

    except Exception as e:
        print(f"{RED}Error running task: {e}{RESET}")
        import traceback
        traceback.print_exc()
        return 1
    finally:
        try:
            if 'env' in locals():
                await env.close()
        except Exception:
            pass

    return 0


async def _execute_native_interactive(args, env_path: Path, provider_type: str) -> int:
    """Execute interactive mode for native tasks using task runner."""
    from cua_bench.runner.task_runner import TaskRunner
    import time

    try:
        # Determine env_type (linux-docker, windows-qemu, etc.)
        env_type = getattr(args, 'env_type', None) or _get_env_type_from_provider(provider_type)

        print(f"{CYAN}Starting environment with task runner (env_type: {env_type})...{RESET}")

        # Create task runner
        runner = TaskRunner()

        # Get task index
        task_index = getattr(args, 'variant_id', 0)

        # Start environment container interactively
        vnc_url, api_url, cleanup, task_config, env, session = await runner.run_task_interactively(
            env_type=env_type,
            env_path=env_path,
            task_index=task_index,
            memory=getattr(args, 'memory', '8G'),
            cpus=getattr(args, 'cpus', '8'),
        )

        print(f"{GREEN}✓ Environment started{RESET}")

        # Print task description if available
        if task_config:
            description = task_config.get('description')
            if description:
                print(f"\n{BOLD}Task: {description}{RESET}")

            # Print setup info
            setup_time = task_config.get('_setup_time')
            screenshot_size = task_config.get('_screenshot_size')
            if setup_time is not None:
                print(
                    f"{GREEN}✓ Setup complete in {BOLD}{setup_time:.2f}s{RESET}{GREEN} "
                    f"(screenshot: {screenshot_size} bytes){RESET}"
                )

        print(f"{CYAN}VNC URL: {BOLD}{vnc_url}{RESET}")
        print(f"{GREY}API URL: {api_url}{RESET}")

        # Wait for VNC to be available
        print(f"{CYAN}Waiting for VNC to be ready...{RESET}")
        import urllib.request
        import time as sync_time

        vnc_ready = False
        max_attempts = 30
        for attempt in range(max_attempts):
            try:
                # Try to connect to the VNC URL
                with urllib.request.urlopen(vnc_url, timeout=2) as response:
                    if response.status == 200:
                        vnc_ready = True
                        break
            except Exception:
                pass

            await asyncio.sleep(0.5)

        if not vnc_ready:
            print(f"{YELLOW}Warning: VNC may not be ready yet, but opening anyway...{RESET}")

        # Open VNC in browser automatically with autoconnect and show_dot parameters
        import webbrowser
        vnc_url_with_params = f"{vnc_url}?autoconnect=true&show_dot=true"
        webbrowser.open(vnc_url_with_params)
        print(f"{GREEN}✓ Opened VNC in browser{RESET}")

        # Keep environment running until user presses Enter
        if not getattr(args, 'no_wait', False):
            print(f"\n{GREY}Environment is open. Press Enter to close...{RESET}")
            input()

        # Evaluate if function exists
        if env and env.evaluate_task_fn and task_config:
            print(f"\n{CYAN}Running evaluation...{RESET}")
            task_cfg = task_config.get('_task_cfg')
            if task_cfg and session:
                result = await env.evaluate_task_fn(task_cfg, session)
                print(f"{YELLOW}✓ Evaluation result: {BOLD}{result}{RESET}")

        # Cleanup
        print(f"\n{CYAN}Cleaning up environment...{RESET}")
        await cleanup()
        print(f"{GREEN}✓ Task completed successfully!{RESET}")

        return 0

    except Exception as e:
        print(f"{RED}Error running interactive environment: {e}{RESET}")
        import traceback
        traceback.print_exc()
        return 1
