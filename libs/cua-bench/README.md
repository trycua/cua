# cua-bench

Framework for benchmarking Computer-Use Agents with verifiable cross-platform environments.

**[Documentation](https://cua.ai/docs/cuabench)** - Installation, guides, and API reference.

## Running Tests

The test suite covers the core gym interface, worker system, and benchmark runners.

### Install dev dependencies

```bash
uv pip install -e ".[dev,browser,server]"
```

Note: The `browser` extra installs Playwright for e2e tests with the simulated provider.

### Run all tests

```bash
uv run --with pytest pytest cua_bench/tests/ -v
```

### Run specific test modules

```bash
# Core gym interface (make, reset, step, evaluate)
uv run --with pytest pytest cua_bench/tests/test_gym_interface.py -v

# HTTP worker client (/reset, /step endpoints)
uv run --with pytest pytest cua_bench/tests/test_worker_client.py -v

# Worker server endpoints and action serialization
uv run --with pytest pytest cua_bench/tests/test_worker_server.py -v

# Benchmark runner functions
uv run --with pytest pytest cua_bench/tests/test_run_benchmark.py -v

# Worker manager (spawning/managing workers)
uv run --with pytest pytest cua_bench/tests/test_worker_manager.py -v

# Action parsing
uv run --with pytest pytest cua_bench/tests/test_actions.py -v
```

### Run tests with coverage

```bash
uv run --with pytest --with pytest-cov pytest cua_bench/tests/ -v --cov=cua_bench --cov-report=term-missing
```

## Test Structure

| Test Module              | What it Tests                                                     | Approach                                                                  |
| ------------------------ | ----------------------------------------------------------------- | ------------------------------------------------------------------------- |
| `test_gym_interface.py`  | Core Environment API: `make()`, `reset()`, `step()`, `evaluate()` | **E2E** - Real simulated (Playwright) environments                        |
| `test_worker_client.py`  | HTTP client for worker servers (`CBEnvWorkerClient`)              | **Mock server** - Uses `@patch("requests.post")` to mock HTTP responses   |
| `test_worker_server.py`  | FastAPI endpoints and action serialization                        | **Unit** - Action serialize/deserialize, request models, simple endpoints |
| `test_run_benchmark.py`  | `run_benchmark()`, `run_single_task()`, `run_interactive()`       | **E2E** - Real simulated (Playwright) environments                        |
| `test_worker_manager.py` | Workers + dataloader training loop                                | **E2E** - Real workers, real envs, mock model for actions                 |
| `test_actions.py`        | Action string parsing (`repr_to_action()`)                        | **Unit** - Pure function tests                                            |

### Test Approach Philosophy

- **E2E tests** use real simulated (Playwright) environments. The `simulated` provider is fast enough for testing.
- **Mock server tests** (`test_worker_client.py`) mock HTTP responses to test client logic in isolation.
- **Mock model** (`test_worker_manager.py`) uses a mock model that returns simple actions to test the dataloader training loop without requiring a real ML model.

## Infrastructure Benchmarking

Measure the throughput of the worker infrastructure:

```bash
uv run python -m cua_bench.scripts.benchmark_workers --num_workers 16 --num_steps 10
```

Options:

| Flag            | Default | Description                                    |
| --------------- | ------- | ---------------------------------------------- |
| `--num_workers` | 16      | Number of parallel workers                     |
| `--num_steps`   | 10      | Steps per worker                               |
| `--task_path`   | None    | Path to task directory (creates temp if empty) |

Output:

- Average reset time
- Average step time
- Average finish time
- Step throughput (steps/sec)
