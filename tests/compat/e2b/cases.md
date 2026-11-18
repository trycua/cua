# Shared E2B cases

`cases/*.json` is one set of behaviour cases run by two runners that share
no code: `runner.py` (Python `e2b`) and `runner.mjs` (TypeScript `e2b`). Both
print the same JSON shape, so `compare.py` can diff a run (Cua gateway, the
`cua-e2b` shim) against a recording from the real E2B.

```
python runner.py [--only a,b] [--skip a,b] [--tag t] > run.json
node runner.mjs --sdk <node_modules/e2b> [--only ...] > run.json
python compare.py fixtures/cases-py-2.56.0.json run.json --rules rules-space.json
```

Recordings are per language and SDK version (`fixtures/cases-<lang>-<version>.json`)
because the SDKs word their client-side errors differently. `tests/compat/run_cases.sh`
runs both languages for the pinned and the latest SDK; `--record` writes the
recordings (against the real E2B only).

## Case language

A case is `{"name", "tags", "sandbox", "create", "skip", "sync", "steps"}`.

- `sandbox`: `shared` (default; one sandbox for the whole run, variable `sb`), `own` (a fresh sandbox
  made with `create`, killed afterwards) or `none` (the case creates its own).
- A step is `[op, args, kw, extras]`. `op` is a Python SDK path (`commands.run`, `files.write`,
  `Sandbox.list`, `pty.create`), or `@name` on a stored value (`@wait`, `@kill`), or one of the
  runner's own ops: `sleep`, `http`, `parallel`, `attr`.
- Arguments use the Python names and units (seconds, snake_case); the TypeScript runner converts
  them to millisecond, camelCase option objects. `kw_ts`/`args_ts` override for one language.
- `extras`: `as` (store the result under a name), `on` (a stored value, `sb` by default; dotted
  paths work), `pick` (keep only these fields, dotted), `grep` (keep list items containing this
  text), `sort`, `first`, `paginate`, `consume` (read a stream to the end), `quiet` (record only
  failures), `skip` (`["py"]`/`["ts"]`).
- Values: `"$name.field"` and `"${name}"` read stored results (`$run_id` is unique per run),
  `{"$bytes": {"n", "seed"}}`, `{"$stream": ...}`, `{"$utf8": "text"}`, `{"$repeat": ...}`,
  `{"$query": ...}`, `{"$size": {"rows", "cols"}}`, and `{"$cb": "name", ...}` (a callback whose
  calls are collected; `as` is `text` or `event`; `join` joins them, `probe` records which of the
  given substrings appeared). Collected names ending in `_set` are sorted sets, `_count` counts.

An observation records, for each step, the normalised result (ids, times and versions masked, big
strings hashed, bytes as length and hash) or the exception (class, MRO, message and fields).
