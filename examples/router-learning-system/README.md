# Router Learning System — Self-Improving Task Routing for Cua

This example demonstrates a production-ready task routing pattern for computer-use agents:

- **Multi-tier fallback**: CLI → CDP → Cua Driver → Vision
- **Self-learning**: SQLite-backed history that dynamically reorders paths based on historical execution time and reliability (after 5+ samples)
- **Failure detection**: Skips paths with 3+ consecutive failures
- **Cold start**: Works with static defaults, optimizes after 5+ samples

## Usage

```bash
python3 examples/router-learning-system/router.py git_commit
python3 examples/router-learning-system/router.py stats
```

## Why this matters for Cua

The router shows how to build agents that **gracefully degrade** when a tool fails — instead of crashing, they escalate to the next available tier. This is essential for production computer-use agents where reliability is the #1 concern.
