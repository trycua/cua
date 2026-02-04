"""Tests for the core gym interface (make, reset, step, evaluate).

This module tests the fundamental gym-style environment API using
end-to-end tests with the simulated (Playwright) provider.
"""

import pytest
from cua_bench import (
    ClickAction,
    DoneAction,
    Environment,
    TypeAction,
    WaitAction,
    make,
)

# Simple HTML for test tasks
SIMPLE_BUTTON_HTML = """
<div class="flex flex-col items-center justify-center h-full w-full bg-gray-100 p-4">
    <h1 class="text-xl mb-4">Test Task</h1>
    <button
        id="test-btn"
        class="btn bg-blue-500 text-white px-4 py-2 rounded"
        onclick="window.__clicked = true"
    >
        Click Me
    </button>
</div>
<script>
    window.__clicked = false;
    window.__score = 0.0;
</script>
"""


class TestMakeFunction:
    """Tests for the make() function."""

    def test_make_returns_environment(self, tmp_path):
        """Test that make() returns an Environment instance."""
        task_dir = tmp_path / "test-task"
        task_dir.mkdir()
        (task_dir / "main.py").write_text(
            """
import cua_bench as cb

@cb.tasks_config(split="train")
def get_tasks():
    return [cb.Task(description="Test task")]
"""
        )

        env = make(str(task_dir))
        assert isinstance(env, Environment)

    def test_make_file_not_found(self):
        """Test make() with non-existent path."""
        with pytest.raises(FileNotFoundError):
            make("/nonexistent/path")

    def test_make_no_main_py(self, tmp_path):
        """Test make() with directory missing main.py."""
        task_dir = tmp_path / "empty-task"
        task_dir.mkdir()

        with pytest.raises(FileNotFoundError):
            make(str(task_dir))

    def test_make_with_split(self, tmp_path):
        """Test make() with different splits."""
        task_dir = tmp_path / "split-task"
        task_dir.mkdir()
        (task_dir / "main.py").write_text(
            """
import cua_bench as cb

@cb.tasks_config(split="train")
def get_train_tasks():
    return [cb.Task(description="Train task")]

@cb.tasks_config(split="test")
def get_test_tasks():
    return [cb.Task(description="Test task")]
"""
        )

        train_env = make(str(task_dir), split="train")
        assert train_env.split == "train"

        test_env = make(str(task_dir), split="test")
        assert test_env.split == "test"


class TestEnvironmentResetE2E:
    """E2E tests for Environment.reset() with simulated provider."""

    @pytest.mark.asyncio
    async def test_reset_returns_screenshot_and_task(self, tmp_path):
        """Test that reset() returns screenshot bytes and task config."""
        task_dir = tmp_path / "reset-task"
        task_dir.mkdir()
        (task_dir / "main.py").write_text(
            f'''
import cua_bench as cb

HTML = """{SIMPLE_BUTTON_HTML}"""

@cb.tasks_config(split="train")
def get_tasks():
    return [cb.Task(
        description="Click the button",
        computer={{"provider": "simulated", "setup_config": {{"width": 800, "height": 600}}}}
    )]

@cb.setup_task(split="train")
async def setup(task, session):
    await session.launch_window(html=HTML, title="Test", width=400, height=300)
'''
        )

        env = make(str(task_dir))
        try:
            screenshot, task_cfg = await env.reset(task_id=0)

            assert isinstance(screenshot, bytes)
            assert len(screenshot) > 0
            assert task_cfg is not None
            assert task_cfg.description == "Click the button"
        finally:
            await env.close()

    @pytest.mark.asyncio
    async def test_reset_clears_step_count(self, tmp_path):
        """Test that reset() clears the step counter."""
        task_dir = tmp_path / "reset-steps-task"
        task_dir.mkdir()
        (task_dir / "main.py").write_text(
            f'''
import cua_bench as cb

HTML = """{SIMPLE_BUTTON_HTML}"""

@cb.tasks_config(split="train")
def get_tasks():
    return [cb.Task(
        description="Test",
        computer={{"provider": "simulated", "setup_config": {{"width": 800, "height": 600}}}}
    )]

@cb.setup_task(split="train")
async def setup(task, session):
    await session.launch_window(html=HTML, title="Test", width=400, height=300)
'''
        )

        env = make(str(task_dir))
        try:
            await env.reset(task_id=0)

            # Take some steps
            await env.step(WaitAction(seconds=0.1))
            await env.step(WaitAction(seconds=0.1))
            assert env.step_count == 2

            # Reset should clear step count
            env.current_task = None  # Allow re-reset
            await env.reset(task_id=0)
            assert env.step_count == 0
        finally:
            await env.close()


class TestEnvironmentStepE2E:
    """E2E tests for Environment.step() with simulated provider."""

    @pytest.mark.asyncio
    async def test_step_executes_click_action(self, tmp_path):
        """Test that step() executes a click action."""
        task_dir = tmp_path / "step-click-task"
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

        env = make(str(task_dir))
        try:
            await env.reset(task_id=0)

            # Click in the center of the window (where button should be)
            screenshot = await env.step(ClickAction(x=400, y=300))

            assert isinstance(screenshot, bytes)
            assert len(screenshot) > 0
            assert env.step_count == 1
        finally:
            await env.close()

    @pytest.mark.asyncio
    async def test_step_increments_counter(self, tmp_path):
        """Test that step() increments the step counter."""
        task_dir = tmp_path / "step-counter-task"
        task_dir.mkdir()
        (task_dir / "main.py").write_text(
            f'''
import cua_bench as cb

HTML = """{SIMPLE_BUTTON_HTML}"""

@cb.tasks_config(split="train")
def get_tasks():
    return [cb.Task(
        description="Test",
        computer={{"provider": "simulated", "setup_config": {{"width": 800, "height": 600}}}}
    )]

@cb.setup_task(split="train")
async def setup(task, session):
    await session.launch_window(html=HTML, title="Test", width=400, height=300)
'''
        )

        env = make(str(task_dir))
        try:
            await env.reset(task_id=0)

            assert env.step_count == 0

            await env.step(WaitAction(seconds=0.1))
            assert env.step_count == 1

            await env.step(WaitAction(seconds=0.1))
            assert env.step_count == 2

            await env.step(ClickAction(x=100, y=100))
            assert env.step_count == 3
        finally:
            await env.close()


class TestEnvironmentEvaluateE2E:
    """E2E tests for Environment.evaluate() with simulated provider."""

    @pytest.mark.asyncio
    async def test_evaluate_returns_result(self, tmp_path):
        """Test that evaluate() returns evaluation result."""
        task_dir = tmp_path / "eval-task"
        task_dir.mkdir()
        (task_dir / "main.py").write_text(
            '''
import cua_bench as cb

HTML = """
<div class="flex items-center justify-center h-full w-full">
    <span id="status">Ready</span>
</div>
<script>
    window.__score = 0.75;
</script>
"""

pid = None

@cb.tasks_config(split="train")
def get_tasks():
    return [cb.Task(
        description="Test evaluation",
        computer={"provider": "simulated", "setup_config": {"width": 800, "height": 600}}
    )]

@cb.setup_task(split="train")
async def setup(task, session):
    global pid
    pid = await session.launch_window(html=HTML, title="Test", width=400, height=300)

@cb.evaluate_task(split="train")
async def evaluate(task, session):
    global pid
    score = await session.execute_javascript(pid, "window.__score")
    return [float(score)]
'''
        )

        env = make(str(task_dir))
        try:
            await env.reset(task_id=0)

            result = await env.evaluate()

            assert result == [0.75]
        finally:
            await env.close()


class TestEnvironmentSolveE2E:
    """E2E tests for Environment.solve() with simulated provider."""

    @pytest.mark.asyncio
    async def test_solve_runs_solver(self, tmp_path):
        """Test that solve() runs the solver function."""
        task_dir = tmp_path / "solve-task"
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
    # Click the button using bot helper
    await session.click_element(pid, "#test-btn")

@cb.evaluate_task(split="train")
async def evaluate(task, session):
    global pid
    clicked = await session.execute_javascript(pid, "window.__clicked")
    return [1.0 if clicked else 0.0]
'''
        )

        env = make(str(task_dir))
        try:
            await env.reset(task_id=0)

            # Before solve, button not clicked
            result_before = await env.evaluate()
            assert result_before == [0.0]

            # Run solver
            await env.solve()

            # After solve, button should be clicked
            result_after = await env.evaluate()
            assert result_after == [1.0]
        finally:
            await env.close()


class TestEnvironmentCloseE2E:
    """E2E tests for Environment.close()."""

    @pytest.mark.asyncio
    async def test_close_cleans_up_session(self, tmp_path):
        """Test that close() cleans up the session."""
        task_dir = tmp_path / "close-task"
        task_dir.mkdir()
        (task_dir / "main.py").write_text(
            f'''
import cua_bench as cb

HTML = """{SIMPLE_BUTTON_HTML}"""

@cb.tasks_config(split="train")
def get_tasks():
    return [cb.Task(
        description="Test",
        computer={{"provider": "simulated", "setup_config": {{"width": 800, "height": 600}}}}
    )]

@cb.setup_task(split="train")
async def setup(task, session):
    await session.launch_window(html=HTML, title="Test", width=400, height=300)
'''
        )

        env = make(str(task_dir))
        await env.reset(task_id=0)

        assert env.session is not None

        await env.close()

        assert env.session is None

    @pytest.mark.asyncio
    async def test_close_without_session(self):
        """Test that close() handles no session gracefully."""
        env = Environment()
        env.session = None

        # Should not raise
        await env.close()


class TestActionTypes:
    """Tests for action type creation."""

    def test_click_action(self):
        """Test ClickAction creation."""
        action = ClickAction(x=100, y=200)
        assert action.x == 100
        assert action.y == 200

    def test_type_action(self):
        """Test TypeAction creation."""
        action = TypeAction(text="Hello World")
        assert action.text == "Hello World"

    def test_done_action(self):
        """Test DoneAction creation."""
        action = DoneAction()
        assert isinstance(action, DoneAction)

    def test_wait_action_default(self):
        """Test WaitAction with default seconds."""
        action = WaitAction()
        assert action.seconds == 1.0

    def test_wait_action_custom(self):
        """Test WaitAction with custom seconds."""
        action = WaitAction(seconds=2.5)
        assert action.seconds == 2.5
