"""Tests for async utilities."""

import asyncio

import pytest
from cua_cli.utils.async_utils import run_async


class TestRunAsync:
    """Tests for run_async function."""

    def test_runs_coroutine(self):
        """Test that coroutine is executed."""

        async def sample_coro():
            return "result"

        result = run_async(sample_coro())

        assert result == "result"

    def test_returns_correct_value(self):
        """Test that return value is correct."""

        async def add(a, b):
            return a + b

        result = run_async(add(2, 3))

        assert result == 5

    def test_propagates_exception(self):
        """Test that exceptions are propagated."""

        async def failing_coro():
            raise ValueError("Test error")

        with pytest.raises(ValueError) as exc_info:
            run_async(failing_coro())

        assert "Test error" in str(exc_info.value)

    def test_handles_async_sleep(self):
        """Test that async sleep works correctly."""

        async def with_sleep():
            await asyncio.sleep(0.01)
            return "done"

        result = run_async(with_sleep())

        assert result == "done"

    def test_handles_nested_coroutines(self):
        """Test that nested coroutines work."""

        async def inner():
            return 42

        async def outer():
            return await inner()

        result = run_async(outer())

        assert result == 42

    def test_handles_none_result(self):
        """Test that None result is handled."""

        async def returns_none():
            pass

        result = run_async(returns_none())

        assert result is None

    def test_handles_list_result(self):
        """Test that list result is handled."""

        async def returns_list():
            return [1, 2, 3]

        result = run_async(returns_list())

        assert result == [1, 2, 3]

    def test_handles_dict_result(self):
        """Test that dict result is handled."""

        async def returns_dict():
            return {"key": "value"}

        result = run_async(returns_dict())

        assert result == {"key": "value"}
