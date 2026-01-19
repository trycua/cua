# Telemetry

cua-bench collects anonymous usage statistics to help improve the product. No personal data, file contents, or sensitive information is collected. You can opt out completely at any time.

## What's Collected

### Enabled by Default

Telemetry is **enabled by default**. You can disable it anytime (see "Disabling Telemetry" below).

**System info:**

- OS name and version
- Python version
- cua-bench version
- Whether running in CI environment

**Usage metrics:**

- CLI commands used (e.g., `run`, `interact`, `trace`)
- Task execution metadata (environment name, provider type, OS type)
- Performance data (duration, step counts, success/failure)
- Batch job configuration (task count, parallelism level)

**Session tracking:**

- Anonymous installation UUID (persisted locally)
- No personal identifiers

### Never Collected

- Personal information or user identifiers
- API keys or credentials
- File contents, paths, or application data
- Screenshots or traces
- Full error tracebacks (only error type + truncated message)
- Model outputs or prompts

## Disabling Telemetry

### Environment Variable

```bash
export CUA_TELEMETRY_ENABLED=false
```

### Check Status

```python
from cua_bench.telemetry import is_telemetry_enabled
print(is_telemetry_enabled())  # True or False
```

## Events Reference

### CLI Events

| Event                | Data                           | Trigger         |
| -------------------- | ------------------------------ | --------------- |
| `cb_command_invoked` | command, subcommand, safe args | Any CLI command |

### Task Lifecycle Events

| Event                          | Data                                           | Trigger        |
| ------------------------------ | ---------------------------------------------- | -------------- |
| `cb_task_execution_started`    | env_name, provider_type, os_type, agent, model | Task begins    |
| `cb_task_evaluation_completed` | success, reward, total_steps, duration         | Task completes |
| `cb_task_execution_failed`     | error_type, error_message (truncated), stage   | Task fails     |

### Batch Events

| Event                     | Data                                                 | Trigger                            |
| ------------------------- | ---------------------------------------------------- | ---------------------------------- |
| `cb_batch_job_started`    | dataset_name, task_count, variant_count, parallelism | Batch job begins                   |
| `cb_batch_task_completed` | env_name, success, reward, duration                  | Individual task in batch completes |

## Why We Collect This

This data helps us:

1. **Understand what features are used** - So we can focus development on what matters
2. **Identify what's not used** - So we can simplify or remove unused features
3. **Track success/failure patterns** - So we can improve reliability
4. **Understand usage scale** - So we can optimize for real-world workloads

## Data Storage

- **Installation ID:** Stored locally at `~/.cua-bench/installation_id`
- **Events:** Sent to PostHog (EU region: `eu.posthog.com`)
- **Retention:** Standard PostHog data retention policies

## Debug Mode

To see what's being sent:

```bash
export CUA_TELEMETRY_DEBUG=on
export CUA_TELEMETRY_ENABLED=true
cb platform list
```

You'll see PostHog debug output showing exact event payloads.

## Questions

Questions about telemetry? Open an issue on [GitHub](https://github.com/trycua/cua).
