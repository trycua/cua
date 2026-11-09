# Platform launch descriptors

All three descriptors expose identical task semantics and workspace layout.
Only local installation paths, commands, and environment syntax vary by
platform. GnuCash receives the book path explicitly. The descriptors set the
supported task-scoped data and configuration directories to limit retained
GnuCash state between trials; reset removes the task workspace runtime
directories.

Regenerate and verify with:

```text
python3 tasks/shared/cdb-s03/tools/build_platform_descriptors.py
python3 tasks/shared/cdb-s03/tools/build_platform_descriptors.py --check
```
