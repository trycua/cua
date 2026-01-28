"""Tests for the benchmark runner functions.

This module tests run_benchmark, run_single_task, and run_interactive
which use the gym interface (make, reset, step, evaluate) directly.
"""

from pathlib import Path

import pytest
from cua_bench import (
    BenchmarkResult,
    DoneAction,
    Task,
    TaskResult,
    run_benchmark,
    run_interactive,
    run_single_task,
)

# Simple HTML for test tasks
SIMPLE_BUTTON_HTML = """
<div class="flex flex-col items-center justify-center h-full w-full bg-gray-100 p-4">
    <h1 class="text-xl mb-4">Test Task</h1>
    <button
        id="test-btn"
        class="btn bg-blue-500 text-white px-4 py-2 rounded"
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


class TestTaskResult:
    """Tests for TaskResult dataclass."""

    def test_task_result_creation(self):
        """Test creating a TaskResult."""
        result = TaskResult(
            task_path="./task",
            variant_id=0,
            success=True,
            reward=1.0,
            steps=5,
        )
        assert result.task_path == "./task"
        assert result.variant_id == 0
        assert result.success is True
        assert result.reward == 1.0
        assert result.steps == 5
        assert result.error is None

    def test_task_result_with_error(self):
        """Test TaskResult with error."""
        result = TaskResult(
            task_path="./task",
            variant_id=0,
            success=False,
            reward=0.0,
            steps=0,
            error="Task failed: timeout",
        )
        assert result.success is False
        assert result.error == "Task failed: timeout"


class TestBenchmarkResult:
    """Tests for BenchmarkResult dataclass."""

    def test_benchmark_result_creation(self):
        """Test creating a BenchmarkResult."""
        result = BenchmarkResult(
            run_id="test-run-123",
            task_results=[
                {"task_path": "./task1", "success": True, "reward": 1.0},
                {"task_path": "./task2", "success": False, "reward": 0.0},
            ],
            total_tasks=2,
            success_count=1,
            failed_count=1,
            avg_reward=0.5,
            duration_seconds=120.5,
        )
        assert result.run_id == "test-run-123"
        assert result.total_tasks == 2
        assert result.success_count == 1
        assert result.failed_count == 1
        assert result.avg_reward == 0.5
        assert len(result.task_results) == 2

    def test_benchmark_result_all_success(self):
        """Test BenchmarkResult with all successful tasks."""
        result = BenchmarkResult(
            run_id="success-run",
            task_results=[{"success": True, "reward": 1.0} for _ in range(5)],
            total_tasks=5,
            success_count=5,
            failed_count=0,
            avg_reward=1.0,
            duration_seconds=60.0,
        )
        assert result.success_count == result.total_tasks
        assert result.failed_count == 0

    def test_benchmark_result_all_failed(self):
        """Test BenchmarkResult with all failed tasks."""
        result = BenchmarkResult(
            run_id="failed-run",
            task_results=[{"success": False, "reward": 0.0} for _ in range(3)],
            total_tasks=3,
            success_count=0,
            failed_count=3,
            avg_reward=0.0,
            duration_seconds=30.0,
        )
        assert result.success_count == 0
        assert result.failed_count == result.total_tasks


class TestRunSingleTaskE2E:
    """E2E tests for run_single_task function."""

    @pytest.mark.asyncio
    async def test_run_single_task_oracle(self, tmp_path):
        """Test run_single_task with oracle mode."""
        task_dir = tmp_path / "task"
        task_dir.mkdir()
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
    )]

@cb.setup_task(split="train")
async def setup(task, session):
    global pid
    pid = await session.launch_window(html=HTML, title="Test", width=400, height=300)

@cb.solve_task(split="train")
async def solve(task, session):
    global pid
    await session.click_element(pid, "#test-btn")

@cb.evaluate_task(split="train")
async def evaluate(task, session):
    global pid
    clicked = await session.execute_javascript(pid, "window.__clicked")
    return [1.0 if clicked else 0.0]
'''
        )

        result = await run_single_task(task_dir, oracle=True)

        assert result.success is True
        assert result.reward == 1.0

    @pytest.mark.asyncio
    async def test_run_single_task_with_agent(self, tmp_path):
        """Test run_single_task with custom agent that immediately finishes."""
        task_dir = tmp_path / "task"
        task_dir.mkdir()
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
    )]

@cb.setup_task(split="train")
async def setup(task, session):
    global pid
    pid = await session.launch_window(html=HTML, title="Test", width=400, height=300)

@cb.evaluate_task(split="train")
async def evaluate(task, session):
    global pid
    clicked = await session.execute_javascript(pid, "window.__clicked")
    return [1.0 if clicked else 0.0]
'''
        )

        def done_agent(screenshot: bytes, task: Task):
            return DoneAction()

        result = await run_single_task(task_dir, agent_fn=done_agent)

        # Agent just returns DoneAction, so button won't be clicked
        assert result.reward == 0.0
        assert result.steps == 1

    @pytest.mark.asyncio
    async def test_run_single_task_failure(self):
        """Test run_single_task handles errors."""
        result = await run_single_task(Path("/nonexistent/task"))

        assert result.success is False
        assert result.reward == 0.0
        assert result.error is not None


class TestRunBenchmark:
    """Tests for run_benchmark function."""

    @pytest.mark.asyncio
    async def test_run_benchmark_not_found(self):
        """Test run_benchmark with non-existent dataset."""
        with pytest.raises(FileNotFoundError, match="Dataset not found"):
            await run_benchmark(Path("/nonexistent/dataset"))

    @pytest.mark.asyncio
    async def test_run_benchmark_empty_dataset(self, tmp_path):
        """Test run_benchmark with empty dataset."""
        dataset_path = tmp_path / "empty"
        dataset_path.mkdir()

        with pytest.raises(ValueError, match="No tasks found"):
            await run_benchmark(dataset_path)

    @pytest.mark.asyncio
    async def test_run_benchmark_task_filter_no_match(self, tmp_path):
        """Test run_benchmark with filter that matches nothing."""
        dataset_path = tmp_path / "dataset"
        dataset_path.mkdir()
        task_dir = dataset_path / "my-task"
        task_dir.mkdir()
        (task_dir / "main.py").write_text(
            """
import cua_bench as cb

@cb.tasks_config(split="train")
def get_tasks():
    return [cb.Task(description="Test")]
"""
        )

        with pytest.raises(ValueError, match="No tasks match filter"):
            await run_benchmark(dataset_path, task_filter="nonexistent-*")


class TestRunBenchmarkE2E:
    """E2E tests for run_benchmark function."""

    @pytest.mark.asyncio
    async def test_run_benchmark_oracle(self, tmp_path):
        """Test run_benchmark with oracle mode."""
        dataset_path = tmp_path / "dataset"
        dataset_path.mkdir()

        task_dir = dataset_path / "click-task"
        task_dir.mkdir()
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
    )]

@cb.setup_task(split="train")
async def setup(task, session):
    global pid
    pid = await session.launch_window(html=HTML, title="Test", width=400, height=300)

@cb.solve_task(split="train")
async def solve(task, session):
    global pid
    await session.click_element(pid, "#test-btn")

@cb.evaluate_task(split="train")
async def evaluate(task, session):
    global pid
    clicked = await session.execute_javascript(pid, "window.__clicked")
    return [1.0 if clicked else 0.0]
'''
        )

        result = await run_benchmark(dataset_path, oracle=True, max_parallel=1)

        assert isinstance(result, BenchmarkResult)
        assert result.total_tasks == 1
        assert result.success_count == 1
        assert result.avg_reward == 1.0

    @pytest.mark.asyncio
    async def test_run_benchmark_task_filter(self, tmp_path):
        """Test run_benchmark with task filter."""
        dataset_path = tmp_path / "dataset"
        dataset_path.mkdir()

        # Create multiple tasks
        for name in ["click-task", "type-task", "other-task"]:
            task_dir = dataset_path / name
            task_dir.mkdir()
            (task_dir / "main.py").write_text(
                f'''
import cua_bench as cb

HTML = """{SIMPLE_BUTTON_HTML}"""

pid = None

@cb.tasks_config(split="train")
def get_tasks():
    return [cb.Task(
        description="{name}",
        computer={{"provider": "simulated", "setup_config": {{"width": 800, "height": 600}}}}
    )]

@cb.setup_task(split="train")
async def setup(task, session):
    global pid
    pid = await session.launch_window(html=HTML, title="Test", width=400, height=300)

@cb.solve_task(split="train")
async def solve(task, session):
    global pid
    await session.click_element(pid, "#test-btn")

@cb.evaluate_task(split="train")
async def evaluate(task, session):
    global pid
    clicked = await session.execute_javascript(pid, "window.__clicked")
    return [1.0 if clicked else 0.0]
'''
            )

        # Run with filter matching only click-task
        result = await run_benchmark(
            dataset_path,
            oracle=True,
            task_filter="click-*",
            max_parallel=1,
        )

        assert result.total_tasks == 1


class TestRunInteractiveE2E:
    """E2E tests for run_interactive function."""

    @pytest.mark.asyncio
    async def test_run_interactive_returns_env(self, tmp_path):
        """Test run_interactive returns environment."""
        task_dir = tmp_path / "task"
        task_dir.mkdir()
        (task_dir / "main.py").write_text(
            f'''
import cua_bench as cb

HTML = """{SIMPLE_BUTTON_HTML}"""

pid = None

@cb.tasks_config(split="train")
def get_tasks():
    return [cb.Task(
        description="Interactive task",
        computer={{"provider": "simulated", "setup_config": {{"width": 800, "height": 600}}}}
    )]

@cb.setup_task(split="train")
async def setup(task, session):
    global pid
    pid = await session.launch_window(html=HTML, title="Test", width=400, height=300)
'''
        )

        env, screenshot, task_cfg = await run_interactive(task_dir)

        try:
            assert env is not None
            assert isinstance(screenshot, bytes)
            assert len(screenshot) > 0
            assert task_cfg.description == "Interactive task"
        finally:
            await env.close()
