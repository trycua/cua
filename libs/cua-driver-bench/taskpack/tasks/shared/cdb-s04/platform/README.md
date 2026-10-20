# CDB-S04 platform descriptors

All three descriptors expose identical task semantics and workspace layout.
Only local executable paths, command syntax, and environment conventions vary
by platform. Rillwatch Cases and Margin Lantern Manual are separate Electron
applications. The Morrowglass Archive server binds only to loopback, and the
browser uses a task-scoped profile removed by reset.

Regenerate and verify with:

```text
python3 tasks/shared/cdb-s04/tools/build_platform_descriptors.py
python3 tasks/shared/cdb-s04/tools/build_platform_descriptors.py --check
```
