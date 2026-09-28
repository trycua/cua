# Running cua-s1 tests on Intel macOS (x86_64)

Local scratch notes from testing `cua-s1` on an Intel Mac. Not an official
part of the test docs — see the top-level `TESTING.md` for the canonical
commands.

## Problem

A plain `uv sync --project libs/cua-s1/python --extra pdf --group test` fails
on x86_64 macOS because two dependencies in the resolved graph have dropped
Intel-macOS wheels:

- `torch` (base dependency, `>=2.2,<3`) resolves to `2.14.0` by default, which
  only ships wheels for `macosx_14_0_arm64`, plus Linux/Windows. The last
  `torch` release with an x86_64 macOS wheel is `2.2.2`, and even that only
  goes up to `cp311` (Python 3.11).
- `cryptography` (transitive, via the `pdf` extra's
  `pdfplumber -> pdfminer-six -> cryptography`) dropped x86_64-specific macOS
  wheels after `46.0.3`. `46.0.3` still ships a `universal2` wheel that covers
  Intel, and `pdfminer-six` only requires `cryptography>=36.0.0`, so pinning
  back works.
- Once `torch` is pinned back to `2.2.2`, a newer `numpy` (pulled in by other
  deps, e.g. `2.4.6`) is ABI-incompatible with it (`_ARRAY_API not found`,
  `RuntimeError: Numpy is not available` inside checkpoint round-trip tests).
  Pinning `numpy==1.26.4` (last one before the 2.x ABI break, and well within
  the base `numpy>=1.26` requirement) fixes this.

## Workaround

Add to `libs/cua-s1/python/pyproject.toml` under `[tool.uv]`, run the sync,
then revert:

```toml
[tool.uv]
override-dependencies = ["torch==2.2.2", "cryptography==46.0.3", "numpy==1.26.4"]
```

```bash
uv sync --project libs/cua-s1/python --extra all --extra training --group test -p 3.11
uv run --project libs/cua-s1/python pytest libs/cua-s1/python/tests -v
```

`--extra all` covers `mcp`, `pdf`, `transformers`, `pillow`, `peft`.
`--extra training` adds the local `cua-bench-s1` editable dependency needed by
`test_train_4b.py`, `test_train_4b_v2.py`, `test_train_4b_rl.py`, and
`test_train_nano.py`. `four-b` was left out — it conflicts with `all` in
`[tool.uv] conflicts`, and `test_four_b.py` doesn't import anything from it at
module scope, so it isn't needed.

Result: **128 passed**, 0 failed, 8 benign warnings (a `TransformerEncoder`
nested-tensor config note), in ~4.6s.

## Why this isn't a real fix

This is a local override, not a lockfile or `pyproject.toml` change meant to
ship. It pins three dependencies backward from what the project actually
declares, purely to route around missing Intel-macOS wheels. It should be
reverted after use (`git checkout -- libs/cua-s1/python/pyproject.toml`) and
not merged. The project appears to only be realistically tested on Apple
Silicon or Linux CI going forward; Intel-mac support for the full dependency
graph (`torch`, `cryptography`, and whatever else follows the same trend) is
likely to keep degrading upstream.
