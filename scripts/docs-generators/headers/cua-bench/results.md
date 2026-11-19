A run writes one directory per variant and a `summary.json` next to them:

```text output
<output-dir>/
  summary.json
  <task>_v<variant>/          (with _a<attempt> for --attempts repeats)
    result.json
    run.log
    trajectory.json           (ATIF-v1.8, screenshots under imgs/)
    task_<variant>_trace/     (a Hugging Face dataset of the trace events)
```

Both files carry `schema_version`; new keys are additive, and a rename or removal bumps the version.
