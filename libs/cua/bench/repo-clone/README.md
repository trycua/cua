# repo-clone

A fresh agent in a fresh Space runs `git clone` as its first bash call. This
bench times what the repo cache changes about that: every trial reports the
Space create, the first no-op bash call, the clone and the total apart, for a
small, a medium and a large public repository, in four cases:

| Case | What the cache holds when the Space clones |
|---|---|
| `off` | the cache is off |
| `miss` | the cache is on and empty: the clone downloads |
| `hit` | a verified mirror; the helper asks the remote for its tip, so the clone is never older than the remote |
| `hit_lease` | a mirror verified a moment ago: the clone needs no network |

```bash
cua bench run repo-clone                              # MODE=sim: small and medium, 5 trials each
REPOS=small,medium,large N=10 cua bench run repo-clone
MODE=space cua bench run repo-clone                   # real Spaces (a Linux container runtime)
CASES=off,hit cua bench run repo-clone --n 3
AGENT_CMD='true' cua bench run repo-clone             # also time "the agent is ready"
```

`MODE=sim` (the default) makes no Space: a temporary directory carries the git
config, helper and mirror a container Space sees, so the clone runs exactly as
it does in one. It measures the clone, not the Space (`space_create_ms` is the
directory). `MODE=space` uses `cua spaces create` and `cua sb exec` with a
daemon of the same `cua` binary and a Spaces home of the run's own.

## What is checked

A trial that fails a check is counted as failed and is not measured:

- the clone's `HEAD` is the remote's `HEAD` (never stale);
- `git fsck --connectivity-only` passes;
- the clone has no alternate (it does not depend on the mirror) and no
  credential in `.git/config`, and the environment holds no token;
- a `hit` or `hit_lease` clone was served from the cache, and an `off` or
  `miss` clone was not.

The canary check (`--canary`, on under `cua bench run`) writes an object in one
clone and requires that another clone, and the mirror, never see it.

## The loop's use

`bench.py` writes `repo-clone.json` (schema `cua.repo-clone/v1`): per
repository and case the raw trials, the failed count and p50, p95, min and max
of every phase. `cua bench run repo-clone` converts it to the shared result
format. A loop that changes the cache (clone strategy, lease, mirror refresh)
runs it before and after and keeps the change only if the medians improve
beyond the run to run spread and no check fails.

The value is the clone p50 of the first repository in the first hit case. It
has no perf guard: the targets are not set yet.
