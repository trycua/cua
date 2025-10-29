# Testing Guide for CUA

Quick guide to running tests and understanding the test architecture.

## 🚀 Quick Start

```bash
# Install dependencies
pip install pytest pytest-asyncio pytest-mock pytest-cov

# Install package
cd libs/python/core
pip install -e .

# Run tests
export CUA_TELEMETRY_DISABLED=1  # or $env:CUA_TELEMETRY_DISABLED="1" on Windows
pytest tests/ -v
```

## 🧪 Running Tests

```bash
# All packages
pytest libs/python/*/tests/ -v

# Specific package
cd libs/python/core && pytest tests/ -v

# With coverage
pytest tests/ --cov --cov-report=html

# Specific test
pytest tests/test_telemetry.py::TestTelemetryEnabled::test_telemetry_enabled_by_default -v
```

## 🏗️ Test Architecture

**Principles**: SRP (Single Responsibility) + Vertical Slices + Testability

```
libs/python/
├── core/tests/           # Tests ONLY core
├── agent/tests/          # Tests ONLY agent
└── computer/tests/       # Tests ONLY computer
```

Each test file = ONE feature. Each test class = ONE concern.

## ➕ Adding New Tests

1. Create `test_*.py` in the appropriate package's `tests/` directory
2. Follow the pattern:

```python
"""Unit tests for my_feature."""
import pytest
from unittest.mock import patch

class TestMyFeature:
    """Test MyFeature class."""

    def test_initialization(self):
        """Test that feature initializes."""
        from my_package import MyFeature
        feature = MyFeature()
        assert feature is not None
```

3. Mock external dependencies:

```python
@pytest.fixture
def mock_api():
    with patch("my_package.api_client") as mock:
        yield mock
```

## 🔄 CI/CD

Tests run automatically on every PR via GitHub Actions (`.github/workflows/python-tests.yml`):

- Matrix strategy: each package tested separately
- Python 3.12
- ~2 minute runtime

## 🐛 Troubleshooting

**ModuleNotFoundError**: Run `pip install -e .` in package directory

**Tests fail in CI but pass locally**: Set `CUA_TELEMETRY_DISABLED=1`

**Async tests error**: Install `pytest-asyncio` and use `@pytest.mark.asyncio`

**Mock not working**: Patch at usage location, not definition:

```python
# ✅ Right
@patch("my_package.module.external_function")

# ❌ Wrong
@patch("external_library.function")
```

---

**Questions?** Check existing tests for examples or open an issue.
