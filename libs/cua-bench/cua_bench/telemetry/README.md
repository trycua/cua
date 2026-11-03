# Telemetry

cua-bench sends anonymous usage events so we can see which features are used. It never sends personal data, and you can turn it off at any time.

## Turning it off

Any one of these disables telemetry:

```bash
export CUA_TELEMETRY=0          # also: false, no, off
export DO_NOT_TRACK=1           # any non-empty value except 0
export CUA_TELEMETRY_ENABLED=false   # legacy, still honored
export CUA_TELEMETRY_DISABLED=1      # legacy, still honored
```

In CI (`CI`, `GITHUB_ACTIONS`, `GITLAB_CI`, `BUILDKITE`, `CIRCLECI`, `JENKINS_URL`, `TF_BUILD`, `CONTINUOUS_INTEGRATION`) telemetry is off unless you set `CUA_TELEMETRY=1`.

Check the current state:

```python
from cua_bench.telemetry import is_telemetry_enabled
print(is_telemetry_enabled())
```

## What is sent

Every event carries:

- cua-bench version
- Python version (major.minor)
- OS name (`darwin`, `linux`, `windows`)
- a random installation id

User-chosen values are mapped to a fixed vocabulary before sending:

- task and environment names: sent only if they are a task shipped with cua-bench, otherwise `custom`
- dataset names: `cua-bench-basic`, `cua-bench-kicad`, `cua-bench-workflows`, otherwise `custom`
- agents: sent only if built in (`cua-agent`, `gemini`, `opencua`, `qwen3vl`, `qwen35`, `harness`), otherwise `custom`
- models: path-like, URL-like, long ids, or ids with an unknown prefix before `/` are sent as `custom`
- errors: exception class name only

### Events

| Event | Properties | When |
| --- | --- | --- |
| `cb_command_invoked` | command, subcommand, sanitized flags (agent, model, max_steps, on, kind, runtime, oracle, detach, max_parallel) | Any CLI command |
| `cb_task_execution_started` | env_name, task_index, provider_type, os_type, max_steps, run_id | A task starts |
| `cb_task_evaluation_completed` | env_name, success, reward, total_steps, duration_seconds, run_id | A task is evaluated |
| `cb_task_execution_failed` | env_name, error_type (class name), stage, run_id | A task fails in setup, step, solve or evaluate |
| `cua_bench_run_completed` | taskset, score_bucket, task_count (bucket), outcome | A `cb run` finishes, fails or is cancelled |

## What is never sent

- file paths, usernames, hostnames, IP addresses or emails
- prompts, typed text, model outputs, screenshots or traces
- API keys or credentials
- error messages or tracebacks
- your own task, dataset, agent or sandbox names

GeoIP lookup is disabled and no person profiles are created.

## Where it goes

- Installation id: `~/.config/cua/installation_id`, created only while telemetry is on
- Events: PostHog EU (`eu.i.posthog.com`)

## Debugging

`CUA_TELEMETRY_DEBUG=on` turns on the PostHog client's own debug logging.

## Questions

Open an issue on [GitHub](https://github.com/trycua/cua).
