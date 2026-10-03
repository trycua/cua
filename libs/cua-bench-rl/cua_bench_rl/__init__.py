"""cua-bench-rl: RL tooling on top of cua-bench.

* :mod:`cua_bench_rl.workers`: FastAPI env workers, their HTTP client and
  manager, and the multi-turn dataloader (``[torch]`` extra).
* :mod:`cua_bench_rl.trainer`: the Tinker off-policy GRPO loop (``[tinker]``).

These lived in ``cua_bench_rl.workers`` / ``cua_bench_rl.trainer`` before
cua-bench 0.3; those import paths still work, with a deprecation warning,
when this package is installed.
"""

__version__ = "0.1.0"
