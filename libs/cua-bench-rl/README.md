# cua-bench-rl

RL tooling for [cua-bench](../cua-bench): parallel environment workers (a
FastAPI server per environment, an HTTP client, a worker manager), a
multi-turn dataloader and replay buffer, and a Tinker GRPO trainer.

It moved out of cua-bench in 0.3, which focuses on running benchmark
datasets. `cua_bench_rl.workers` and `cua_bench_rl.trainer` still import from here,
with a deprecation warning.

```bash
pip install cua-bench-rl            # workers, client, manager
pip install "cua-bench-rl[torch]"   # + MultiTurnDataloader / ReplayBuffer
pip install "cua-bench-rl[tinker]"  # + the Tinker GRPO trainer
python -m cua_bench_rl.workers.worker_server --port 8001
```

Tests: `uv run --extra dev pytest` (hermetic; the torch dataloader tests run
when torch is installed).
