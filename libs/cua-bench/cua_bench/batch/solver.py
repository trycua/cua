"""Standalone task solver: run one variant against an existing sandbox.

``cb run`` does not need this (it runs variants in-process); it exists for
running a task from inside a sandbox, CI jobs or custom harnesses.

Environment variables:
    CUA_ENV_API_URL: the sandbox: a ref (local:<name>, cloud:<name>,
        direct:<host:port>) or a cua-spacesd URL (http://localhost:3211). Required.
    CUA_ENV_TOKEN: spacesd token for that URL, if it needs one.
    CUA_ENV_TYPE: linux | windows | macos | android (default linux).
    CUA_OUTPUT_DIR: where traces go (default /tmp/td_output).
    BATCH_TASK_INDEX: variant to run when --task-index is not given.

Usage:
    CUA_ENV_API_URL=http://localhost:3211 python -m cua_bench.batch.solver tasks/my_task
    python -m cua_bench.batch.solver tasks/my_task --eval --agent cua-agent --model ...
    python -m cua_bench.batch.solver tasks/my_task --dump   # setup + evaluate only
"""

import asyncio
import os
import sys
from pathlib import Path

# Writable HuggingFace cache in read-only images.
os.environ.setdefault("HF_HOME", "/tmp/hf_cache")


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
    args = parse_args(sys.argv)
    if args["env_path"] is None:
        print(__doc__)
        sys.exit(1)

    from cua_bench import make
    from cua_bench.runner.episode import AgentOptions, EpisodeError, run_episode

    task_index = args["task_index"]
    if task_index is None:
        task_index = int(os.environ.get("BATCH_TASK_INDEX", "0"))
    output_dir = Path(os.environ.get("CUA_OUTPUT_DIR", "/tmp/td_output"))
    opts = AgentOptions(
        oracle=not args["eval_mode"],
        agent=args["agent_name"],
        agent_import_path=args["agent_import_path"],
        model=args["model"],
        max_steps=args["max_steps"],
        dump=args["dump_mode"],
        save_pngs=args["save_pngs"],
        filter_events=args["filter_events"],
    )

    if os.environ.get("CUA_PROVIDER", "").strip().lower() in ("simulated", "webtop"):
        print("Error: CUA_PROVIDER=simulated was removed in cua-bench 0.3; set CUA_ENV_API_URL.")
        sys.exit(1)
    url = os.environ.get("CUA_ENV_API_URL", "")
    if not url:
        print("Error: CUA_ENV_API_URL (a cua-spacesd URL) is required.")
        sys.exit(1)

    env = make(str(args["env_path"]))
    session = None
    try:
        from cua_bench.computers.remote import RemoteDesktopSession
        from cua_sandbox import Sandbox

        from cua_bench.computers.remote import is_sandbox_ref

        if is_sandbox_ref(url):
            sandbox = await Sandbox.connect(url)
        else:
            sandbox = await Sandbox.connect(url=url, token=os.environ.get("CUA_ENV_TOKEN"))
        session = RemoteDesktopSession.attach(
            sandbox, os_type=os.environ.get("CUA_ENV_TYPE", "linux")
        )
        result = await run_episode(env, task_index, opts, output_dir, session=session)
    except EpisodeError as error:
        print(f"Error: {error}")
        sys.exit(1)
    except Exception as error:  # noqa: BLE001
        import traceback

        print(f"Error running task {task_index}: {error}")
        traceback.print_exc()
        sys.exit(1)
    finally:
        if session is not None:
            try:
                await session.sandbox.disconnect()
            except Exception:  # noqa: BLE001
                pass
    if not result.success:
        sys.exit(1)
    print(f"\n✓ Task {task_index} completed successfully!")


if __name__ == "__main__":
    asyncio.run(main())
