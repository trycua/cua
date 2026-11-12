# PyPI retention

This page says which releases of our Python packages stay on PyPI and when
an old one may be removed. It covers every project trycua publishes on PyPI,
including `cua-driver`, `cua`, `cua-cli`, `cua-sandbox` and `cua-agent`.

## The rule

1. **The newest patch of every minor stays on PyPI permanently.** For
   example, if `cua-driver` 0.45.4 is the last 0.45.x, it is never removed.
   Pin to a minor's newest patch and the pin keeps working.
2. **Nothing younger than 90 days is ever deleted.** Age counts from the
   first upload of the release.
3. **Every deletion is announced 30 days ahead, and the release is yanked
   first.** The announcement is in the release notes of a GitHub release and
   names each version. Yanking happens on the day of the announcement: pip
   then skips the version unless something pins it exactly, so an `==` pin
   keeps installing while you move it.
4. **A version that one of our published packages still needs is never
   deleted.** If a release of any trycua PyPI project requires it (for
   example `cua-sandbox` requiring `cua-driver==X`), or a pin in our
   repositories resolves only to it, the deletion is refused.
5. **The GitHub release a PyPI release downloads from is kept** for as long
   as that PyPI release exists.

Deleting a PyPI release is permanent: the same version number can never be
uploaded again. Yanking alone frees no space, so the 30 days between yank and
deletion are your window to move a pin.

## What this means if you pin a version

- Pin the newest patch of a minor (`cua-driver==0.45.4`) or a compatible
  range (`cua-driver~=0.45.0`). Both stay installable for good.
- If you pin an older patch, watch the release notes. A removal is announced
  30 days ahead, and the version shows as yanked on PyPI from that day.

## How a deletion is checked

Every delete list goes through
[`.github/scripts/pypi_retention_guard.py`](../.github/scripts/pypi_retention_guard.py)
before anything is removed. It refuses the list, and exits non-zero, if any
version breaks a rule above. It checks PyPI's own data (upload times, yank
state, every release's `requires_dist`) and scans repository checkouts for
pins in requirements files, `pyproject.toml`, `uv.lock`, Dockerfiles, docs
and workflows.

```bash
# pypi-delete-list.txt
# announced: 2026-12-01 https://github.com/trycua/cua/releases/tag/cua-driver-rs-v0.48.0
0.12.3
0.12.4
```

```bash
python3 .github/scripts/pypi_retention_guard.py --project cua-driver \
  --delete-list pypi-delete-list.txt --repo-root . --repo-root ../other-checkout
```

The test `.github/scripts/tests/test_pypi_pins_exist.py` is the backstop: CI
fails if any pin of `cua-driver`, `cua` or `cua-cli` in this repository no
longer resolves on PyPI.

## Why the rule exists

Each release of our native packages is a set of platform wheels of 30 to 80 MB,
and PyPI caps a project at 10 GB. On 12 Nov 2026 a cleanup to get under that
cap deleted releases that people and our own packages still pinned. Fewer,
smaller PyPI releases now keep us under the cap; this rule keeps any cleanup
from breaking an install.
