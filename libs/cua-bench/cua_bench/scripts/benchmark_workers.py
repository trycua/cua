#!/usr/bin/env python3
"""Benchmark script for CUA-Bench worker throughput.

This script measures the performance of parallel workers with CBEnvWorkerClient,
tracking reset times, step times, and finish times.

Usage:
    uv run python -m cua_bench.scripts.benchmark_workers --num_workers 16 --num_steps 10
"""

import argparse
import asyncio
import multiprocessing
import sys
import tempfile
import time
from multiprocessing import Manager
from pathlib import Path

# ANSI colors
BLUE = "\033[94m"
GREEN = "\033[92m"
YELLOW = "\033[93m"
RED = "\033[91m"
RESET = "\033[0m"

# Simple HTML for test tasks
SIMPLE_BUTTON_HTML = """
<div style="display:flex;flex-direction:column;align-items:center;justify-content:center;height:100vh;background:#f0f0f0;">
    <h1 style="margin-bottom:20px;">Benchmark Task</h1>
    <button
        id="test-btn"
        style="background:#3b82f6;color:white;padding:12px 24px;border:none;border-radius:8px;cursor:pointer;"
        onclick="window.__clicked = true; window.__score = 1.0;"
    >
        Click Me
    </button>
</div>
<script>
    window.__clicked = false;
    window.__score = 0.0;
</script>
"""


def create_temp_task(tmp_dir: Path) -> Path:
    """Create a temporary test task directory."""
    task_dir = tmp_dir / "benchmark-task"
    task_dir.mkdir(exist_ok=True)
    (task_dir / "main.py").write_text(
        f'''
import cua_bench as cb

HTML = """{SIMPLE_BUTTON_HTML}"""

pid = None

@cb.tasks_config(split="train")
def get_tasks():
    return [cb.Task(
        description="Click the button",
        computer={{"provider": "simulated", "setup_config": {{"width": 800, "height": 600}}}}
    ) for _ in range(20)]

@cb.setup_task(split="train")
async def setup(task, session):
    global pid
    pid = await session.launch_window(html=HTML, title="Benchmark", width=400, height=300)

@cb.evaluate_task(split="train")
async def evaluate(task, session):
    global pid
    clicked = await session.execute_javascript(pid, "window.__clicked")
    return [1.0 if clicked else 0.0]
'''
    )
    return task_dir


def run_worker(
    worker_id: int,
    server_url: str,
    task_path: str,
    num_steps: int,
    results_dict: dict,
):
    """Run a single worker process."""
    from cua_bench.workers import CBEnvWorkerClient

    print(f"{BLUE}Worker {worker_id} starting...{RESET}")

    client = CBEnvWorkerClient(
        server_url=server_url,
        img_w=1024,
        img_h=768,
        max_step=100,
        max_hist=10,
        timeout=300,
    )

    # Initialize local counters
    worker_reset_time = 0.0
    worker_reset_count = 0
    worker_step_time = 0.0
    worker_step_count = 0
    worker_finish_time = 0.0
    worker_finish_count = 0

    try:
        # Reset environment
        start_time = time.time()
        env_ret, meta_info = client.reset(
            env_path=task_path,
            task_index=worker_id % 10,
            split="train",
        )
        reset_time = time.time() - start_time
        worker_reset_time += reset_time
        worker_reset_count += 1

        assert env_ret is not None
        assert meta_info is not None
        print(f"{YELLOW}Worker {worker_id} reset in {reset_time:.2f}s{RESET}")

        # Run steps
        for i in range(num_steps):
            start_time = time.time()
            action = {"type": "ClickAction", "x": 0.5, "y": 0.5}
            env_ret, meta_info = client.step(action)
            step_time = time.time() - start_time
            worker_step_time += step_time
            worker_step_count += 1

            assert env_ret is not None
            assert meta_info is not None

            if env_ret.get("done"):
                print(f"{YELLOW}Worker {worker_id} episode done at step {i+1}{RESET}")
                # Reset for next episode
                start_time = time.time()
                env_ret, meta_info = client.reset(
                    env_path=task_path,
                    task_index=(worker_id + i) % 10,
                    split="train",
                )
                reset_time = time.time() - start_time
                worker_reset_time += reset_time
                worker_reset_count += 1

            if (i + 1) % 5 == 0:
                print(
                    f"{YELLOW}Worker {worker_id} step {i+1}/{num_steps} "
                    f"in {step_time:.2f}s{RESET}"
                )

        # Finish with done action
        start_time = time.time()
        action = {"type": "DoneAction"}
        env_ret, meta_info = client.step(action)
        finish_time = time.time() - start_time
        worker_finish_time += finish_time
        worker_finish_count += 1

        assert env_ret is not None
        assert env_ret.get("done")
        print(f"{GREEN}Worker {worker_id} finish in {finish_time:.2f}s{RESET}")

        # Store results
        results_dict[worker_id] = {
            "reset_time": worker_reset_time,
            "reset_count": worker_reset_count,
            "step_time": worker_step_time,
            "step_count": worker_step_count,
            "finish_time": worker_finish_time,
            "finish_count": worker_finish_count,
        }

    except Exception as e:
        print(f"{RED}Worker {worker_id} error: {e}{RESET}")
        results_dict[worker_id] = {
            "error": str(e),
            "reset_time": worker_reset_time,
            "reset_count": worker_reset_count,
            "step_time": worker_step_time,
            "step_count": worker_step_count,
            "finish_time": worker_finish_time,
            "finish_count": worker_finish_count,
        }

    finally:
        try:
            client.shutdown()
        except Exception:
            pass


def run_benchmark(
    num_workers: int = 16,
    num_steps: int = 10,
    task_path: str | None = None,
):
    """Run the worker benchmark."""
    from cua_bench.workers import cleanup_workers, create_workers

    async def _run():
        print(f"\n{GREEN}=== CUA-Bench Worker Benchmark ==={RESET}")
        print(f"{BLUE}Workers: {num_workers}{RESET}")
        print(f"{BLUE}Steps per worker: {num_steps}{RESET}")

        # Create temp task if no task_path provided
        tmp_dir = None
        if task_path is None:
            tmp_dir = tempfile.mkdtemp(prefix="cua-bench-benchmark-")
            actual_task_path = str(create_temp_task(Path(tmp_dir)))
            print(f"{BLUE}Using temp task: {actual_task_path}{RESET}")
        else:
            actual_task_path = task_path
            print(f"{BLUE}Task path: {actual_task_path}{RESET}")

        try:
            # Start worker servers
            print(f"\n{BLUE}Starting {num_workers} worker servers...{RESET}")
            start_time = time.time()
            workers = await create_workers(
                n_workers=num_workers,
                allowed_ips=["127.0.0.1"],
                startup_timeout=120.0,
            )
            worker_startup_time = time.time() - start_time
            print(f"{GREEN}Worker servers started in {worker_startup_time:.2f}s{RESET}")

            try:
                # Create manager for shared results
                manager = Manager()
                results_dict = manager.dict()

                # Start worker processes
                print(f"\n{BLUE}Starting {num_workers} worker clients...{RESET}")
                processes = []
                for i, worker in enumerate(workers):
                    p = multiprocessing.Process(
                        target=run_worker,
                        args=(i, worker.api_url, actual_task_path, num_steps, results_dict),
                    )
                    processes.append(p)
                    p.start()
                    time.sleep(0.1)  # Small delay between starts

                # Wait for all processes
                for p in processes:
                    p.join()

                # Aggregate results
                reset_time_total = 0.0
                reset_count = 0
                step_time_total = 0.0
                step_count = 0
                finish_time_total = 0.0
                finish_count = 0
                errors = []

                for worker_id, result in results_dict.items():
                    if "error" in result:
                        errors.append(f"Worker {worker_id}: {result['error']}")
                    reset_time_total += result["reset_time"]
                    reset_count += result["reset_count"]
                    step_time_total += result["step_time"]
                    step_count += result["step_count"]
                    finish_time_total += result["finish_time"]
                    finish_count += result["finish_count"]

                # Print summary
                print(f"\n{GREEN}=== Benchmark Results ==={RESET}")
                print(f"{BLUE}Workers: {num_workers}{RESET}")
                print(f"{BLUE}Steps per worker: {num_steps}{RESET}")
                print(f"{BLUE}Total resets: {reset_count}{RESET}")
                print(f"{BLUE}Total steps: {step_count}{RESET}")
                print(f"{BLUE}Total finishes: {finish_count}{RESET}")

                if reset_count > 0:
                    print(f"{BLUE}Average reset time: {reset_time_total / reset_count:.2f}s{RESET}")
                if step_count > 0:
                    print(f"{BLUE}Average step time: {step_time_total / step_count:.2f}s{RESET}")
                    throughput = step_count / step_time_total
                    print(f"{BLUE}Step throughput: {throughput:.2f} steps/sec{RESET}")
                if finish_count > 0:
                    print(
                        f"{BLUE}Average finish time: {finish_time_total / finish_count:.2f}s{RESET}"
                    )

                if errors:
                    print(f"\n{RED}Errors ({len(errors)}):{RESET}")
                    for err in errors:
                        print(f"{RED}  {err}{RESET}")

            finally:
                print(f"\n{BLUE}Cleaning up workers...{RESET}")
                await cleanup_workers(workers)

        finally:
            # Clean up temp directory
            if tmp_dir is not None:
                import shutil

                shutil.rmtree(tmp_dir, ignore_errors=True)

    asyncio.run(_run())


def main():
    parser = argparse.ArgumentParser(description="Benchmark CUA-Bench workers")
    parser.add_argument(
        "--num_workers",
        type=int,
        default=16,
        help="Number of parallel workers (default: 16)",
    )
    parser.add_argument(
        "--num_steps",
        type=int,
        default=10,
        help="Number of steps per worker (default: 10)",
    )
    parser.add_argument(
        "--task_path",
        type=str,
        default=None,
        help="Path to task directory (default: creates temp task)",
    )
    args = parser.parse_args()

    run_benchmark(
        num_workers=args.num_workers,
        num_steps=args.num_steps,
        task_path=args.task_path,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
