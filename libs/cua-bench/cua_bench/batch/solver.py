"""Batch task solver - runs individual tasks against golden environments via HTTP.

This solver connects to a running golden environment (started by LocalEnvironmentProvider)
via HTTP API and executes tasks against it.

Environment variables:
    CUA_ENV_API_URL: Required. URL of the golden environment API (e.g., http://localhost:5000)
    CUA_ENV_VNC_URL: Optional. URL for VNC access (e.g., http://localhost:8006)
    CUA_ENV_TYPE: Optional. Environment type (linux, windows, android). Default: linux
    BATCH_TASK_INDEX: Task index to run (default: 0)
    BATCH_TASK_COUNT: Total number of tasks (default: 1)

Usage:
    # With environment already running:
    CUA_ENV_API_URL=http://localhost:5000 python -m cua_bench.batch.solver tasks/my_task

    # With agent:
    CUA_ENV_API_URL=http://localhost:5000 python -m cua_bench.batch.solver tasks/my_task --agent claude

    # Dump mode (skip solver, just setup + evaluate):
    CUA_ENV_API_URL=http://localhost:5000 python -m cua_bench.batch.solver tasks/my_task --dump
"""

import asyncio
import importlib
import os
import sys
import time
from pathlib import Path

# Set HuggingFace cache to writable location to avoid permission issues
# in containerized environments where /usr/local may not be writable
os.environ.setdefault("HF_HOME", "/tmp/hf_cache")


def load_agent_from_path(import_path: str):
    """Load an agent class from an import path like 'path.to.module:ClassName'."""
    if ":" not in import_path:
        raise ValueError("Agent import path must be in format 'module.path:ClassName'")

    module_path, class_name = import_path.split(":", 1)
    module = importlib.import_module(module_path)
    agent_class = getattr(module, class_name)
    return agent_class


def parse_args(argv: list[str]) -> dict:
    """Parse command line arguments."""
    args = {
        "env_path": None,
        "dump_mode": False,
        "eval_mode": False,
        "agent_name": None,
        "agent_import_path": None,
        "model": None,
        "max_steps": None,
        "save_pngs": False,
        "filter_events": None,
        "task_index": None,
    }

    if len(argv) < 2:
        return args

    args["env_path"] = Path(argv[1])
    args["dump_mode"] = "--dump" in argv
    args["eval_mode"] = "--eval" in argv
    args["save_pngs"] = "--save-pngs" in argv

    for i, arg in enumerate(argv):
        if arg == "--agent" and i + 1 < len(argv):
            args["agent_name"] = argv[i + 1]
        elif arg == "--agent-import-path" and i + 1 < len(argv):
            args["agent_import_path"] = argv[i + 1]
        elif arg == "--model" and i + 1 < len(argv):
            args["model"] = argv[i + 1]
        elif arg == "--max-steps" and i + 1 < len(argv):
            args["max_steps"] = int(argv[i + 1])
        elif arg == "--filter" and i + 1 < len(argv):
            args["filter_events"] = [name.strip() for name in argv[i + 1].split(",")]
        elif arg == "--task-index" and i + 1 < len(argv):
            args["task_index"] = int(argv[i + 1])

    return args


async def main():
    """Main entry point for batch task solver.

    Connects to a running golden environment via HTTP and executes tasks.
    """
    # Debug: Print all input variables
    print("=" * 60)
    print("DEBUG: Input Variables")
    print("=" * 60)
    print(f"sys.argv: {sys.argv}")
    print("")

    args = parse_args(sys.argv)

    print("Parsed arguments:")
    for key, value in args.items():
        print(f"  {key}: {value}")
    print("")

    print("Environment variables:")
    env_vars = [
        "CUA_PROVIDER",
        "CUA_ENV_API_URL",
        "CUA_ENV_VNC_URL",
        "CUA_ENV_TYPE",
        "BATCH_TASK_INDEX",
        "BATCH_TASK_COUNT",
        "HF_HOME",
    ]
    for var in env_vars:
        value = os.environ.get(var, "<not set>")
        print(f"  {var}: {value}")
    print("=" * 60)
    print("")

    if args["env_path"] is None:
        print("Usage: python -m cua_bench.batch.solver <env_path> [OPTIONS]")
        print("")
        print("Options:")
        print("  --dump                 Skip solver, just setup + evaluate")
        print("  --eval                 Run agent instead of oracle")
        print("  --agent <name>         Agent name from registry")
        print("  --agent-import-path <path>  Agent class import path")
        print("  --model <model>        Model to use for agent")
        print("  --max-steps <n>        Maximum steps for agent")
        print("  --save-pngs            Save screenshots as PNGs")
        print("  --filter <events>      Filter trace events")
        print("  --task-index <n>       Override task index")
        print("")
        print("Environment variables:")
        print("  CUA_ENV_API_URL        Required. Golden env API URL")
        print("  CUA_ENV_VNC_URL        Optional. VNC URL for debugging")
        print("  CUA_ENV_TYPE           Optional. Environment type (linux/windows/android)")
        sys.exit(1)

    env_path = args["env_path"]

    # Check provider type
    provider = os.environ.get("CUA_PROVIDER", "remote")

    # Get environment connection info
    env_api_url = os.environ.get("CUA_ENV_API_URL", "")
    env_vnc_url = os.environ.get("CUA_ENV_VNC_URL", "")
    env_type = os.environ.get("CUA_ENV_TYPE", "linux")

    # Get task index
    task_index = args["task_index"]
    if task_index is None:
        task_index = int(os.environ.get("BATCH_TASK_INDEX", "0"))
    task_count = int(os.environ.get("BATCH_TASK_COUNT", "1"))

    print(f"Starting task {task_index} of {task_count}")
    print(f"Environment: {env_path}")
    print(f"Provider: {provider}")
    if provider == "simulated":
        print("Using local Playwright session")
    else:
        print(f"API URL: {env_api_url}")
        if env_vnc_url:
            print(f"VNC URL: {env_vnc_url}")

    try:
        # Load task definition
        from cua_bench import make

        env = make(str(env_path))

        # Set headless mode (always headless in container)
        env.headless = True

        session = None
        task_cfg = None

        if provider == "simulated":
            # Simulated provider - use env.reset() to create local Playwright session
            print("Creating local simulated session...")

            # Start tracing
            try:
                tid = env.tracing.start()
                print(f"Tracing started. trajectory_id={tid}")
            except Exception:
                print("Failed to start tracing; continuing without trace.")

            # Reset will create the session and run setup
            screenshot, task_cfg = await env.reset(task_id=task_index)
            session = env.session

            # Wait 2.0s to ensure environment is ready
            # (For some reason playwright sessions need a short delay here)
            await asyncio.sleep(2.0)

            print("✓ Simulated session created and task setup complete")
            print(f"  Screenshot: {len(screenshot)} bytes")
            print(f"  Task: {task_cfg.description}")
        else:
            # Remote provider - connect to remote environment via API
            if not env_api_url:
                print(
                    "Error: CUA_ENV_API_URL environment variable is required for remote provider."
                )
                print("")
                print("Start a golden environment first:")
                print("  cb env start linux-docker")
                print("  cb env start windows-qemu")
                print("")
                print("Then set CUA_ENV_API_URL to the environment's API URL.")
                sys.exit(1)

            from cua_bench.computers.remote import RemoteDesktopSession

            session = RemoteDesktopSession(
                api_url=env_api_url,
                vnc_url=env_vnc_url,
                os_type=env_type,
            )

            # Wait for environment to be ready
            # Windows needs longer boot time than Linux
            boot_timeout = 300 if env_type == "windows" else 120
            print(f"Waiting for environment to be ready (timeout={boot_timeout}s)...")
            if not await session.wait_until_ready(timeout=boot_timeout):
                print("Error: Environment did not become ready within timeout")
                sys.exit(1)
            print("✓ Environment is ready")

            # Load all tasks
            if env.tasks_config_fn is None:
                print("Error: No @cb.tasks_config function found")
                sys.exit(1)

            tasks = env.tasks_config_fn()

            if task_index >= len(tasks):
                print(f"Task index {task_index} >= number of tasks {len(tasks)}, skipping")
                sys.exit(0)

            task_cfg = tasks[task_index]
            print(f"Running task {task_index}: {task_cfg.description}")

            # Create output directory
            output_dir = Path("/tmp/td_output")
            output_dir.mkdir(exist_ok=True)

            # Start tracing
            try:
                tid = env.tracing.start()
                print(f"Tracing started. trajectory_id={tid}")
            except Exception:
                print("Failed to start tracing; continuing without trace.")

            # Run setup
            _t0 = time.perf_counter()
            if env.setup_task_fn:
                await env.setup_task_fn(task_cfg, session)

            # Wait a moment for setup to execute before taking screenshot
            await asyncio.sleep(2.0)

            screenshot = await session.screenshot()
            _elapsed = time.perf_counter() - _t0
            print(f"✓ Setup complete in {_elapsed:.2f}s (screenshot: {len(screenshot)} bytes)")

            # Record reset event with screenshot
            try:
                env.tracing.record(
                    "reset",
                    {
                        "task": repr(task_cfg),
                        "task_index": task_index,
                        "elapsed": _elapsed,
                    },
                    [screenshot],
                )
            except Exception as e:
                print(f"Warning: Failed to record reset event: {e}")

        # Create output directory (after task setup)
        output_dir = Path("/tmp/td_output")
        output_dir.mkdir(exist_ok=True)

        # Run solver or agent
        if not args["dump_mode"]:
            if args["eval_mode"]:
                # Run agent
                agent_class = None
                if args["agent_import_path"]:
                    agent_class = load_agent_from_path(args["agent_import_path"])
                elif args["agent_name"]:
                    from cua_bench.agents import get_agent, list_agents

                    agent_class = get_agent(args["agent_name"])
                    if agent_class is None:
                        print(f"Error: Unknown agent '{args['agent_name']}'")
                        print(f"Available agents: {', '.join(list_agents())}")
                        sys.exit(1)

                if agent_class:
                    agent_kwargs = {}
                    if args["model"]:
                        agent_kwargs["model"] = args["model"]
                    if args["max_steps"] is not None:
                        agent_kwargs["max_steps"] = args["max_steps"]
                    agent = agent_class(**agent_kwargs)

                    logging_dir = output_dir / f"task_{task_index}_agent_logs"
                    logging_dir.mkdir(exist_ok=True, parents=True)

                    print(f"Running agent: {agent.name()}")
                    try:
                        agent_result = await agent.perform_task(
                            task_description=task_cfg.description,
                            session=session,
                            logging_dir=logging_dir,
                            tracer=env.tracing,
                        )
                        print(f"✓ Agent complete: {agent_result}")

                        from cua_bench.agents.base import FailureMode

                        if agent_result.failure_mode not in (FailureMode.UNSET, FailureMode.NONE):
                            print(f"✗ Agent failed with failure mode: {agent_result.failure_mode}")
                            await session.close()
                            sys.exit(1)
                    except Exception as e:
                        print(f"Agent execution failed: {e}")
                        import traceback

                        traceback.print_exc()
                        await session.close()
                        sys.exit(1)
                else:
                    print("ℹ Eval mode without agent: skipping to evaluation")
            else:
                # Run oracle solution
                if env.solve_task_fn:
                    await env.solve_task_fn(task_cfg, session)
                    screenshot = await session.screenshot()
                    print(f"✓ Solution complete (screenshot: {len(screenshot)} bytes)")

                    # Record solution event with screenshot
                    try:
                        env.tracing.record(
                            "solve",
                            {"completed": True},
                            [screenshot],
                        )
                    except Exception as e:
                        print(f"Warning: Failed to record solve event: {e}")
                else:
                    print("Warning: No @cb.solve_task function found")
        else:
            print("ℹ Dump mode: skipping solver")

        # Evaluate
        if env.evaluate_task_fn:
            result = await env.evaluate_task_fn(task_cfg, session)
            print(f"✓ Evaluation result: {result}")

            # Record evaluation event
            try:
                env.tracing.record(
                    "evaluate",
                    {"result": result},
                )
            except Exception as e:
                print(f"Warning: Failed to record evaluate event: {e}")

        # Save trace (non-fatal - task success doesn't depend on trace saving)
        trace_dir = output_dir / f"task_{task_index}_trace"
        image_dir = output_dir / "imgs"
        try:
            env.tracing.save_to_disk(
                str(trace_dir),
                save_pngs=args["save_pngs"],
                image_dir=str(image_dir),
                filter_events=args["filter_events"],
            )
            print(f"✓ Trace saved to {trace_dir}")
        except Exception as e:
            # Trace saving failure is non-fatal - the task still succeeded
            print(f"Warning: Failed to save trace (task still completed successfully): {e}")
            if "Permission denied" in str(e) or "Keys mismatch" in str(e):
                print("  Hint: If using containers, ensure HF_HOME is set to a writable path")

        # Close session
        if provider == "simulated":
            await env.close()
        else:
            await session.close()
        print(f"\n✓ Task {task_index} completed successfully!")

    except Exception as e:
        print(f"Error running task {task_index}: {e}")
        import traceback

        traceback.print_exc()
        # Close session on error
        try:
            if provider == "simulated" and "env" in locals():
                await env.close()
            elif session:
                await session.close()
        except Exception:
            pass
        sys.exit(1)


if __name__ == "__main__":
    asyncio.run(main())
