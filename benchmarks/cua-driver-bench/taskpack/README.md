# Cua Driver Bench task pack (public)

The multi-app tasks of the public Bench v2 task set. Bench v2 runs each one as a
GUI-only task (no coding tools) under the ids in the table; the harness reads
them from here (`CDB_TASKPACK`, see
[`../automated-eval/macos_bench/cdb_adapter.py`](../automated-eval/macos_bench/cdb_adapter.py)).

| Pack task | Bench v2 id | Apps |
| --- | --- | --- |
| [`tasks/shared/cdb-s02`](tasks/shared/cdb-s02) | CDB-G02 | Copperfield Mail, Signal Quay Chat and Daymark Calendar (Electron), LibreOffice Calc |
| [`tasks/shared/cdb-s03`](tasks/shared/cdb-s03) | CDB-G03 | Ledger Lantern Mail (Electron), LibreOffice Calc, GnuCash |
| [`tasks/shared/cdb-s04`](tasks/shared/cdb-s04) | CDB-G04 | Rillwatch Cases and Margin Lantern Manual (Electron), the Morrowglass Archive site in Chrome, a log file |

Each task directory holds:

* `brief.md`: what the agent is told;
* `task.cuabench.json`: the manifest, with a sha256 for every input;
* `apps/`: the task's Electron apps; `fixture/`: the starting data;
* `reset/`: `setup.py` builds the workspace, `verify.py` checks it byte for byte;
* `platform/launch.macos.json`: the launch descriptor the harness follows;
* `evaluator/`: the checker and its oracle (final state only, every required check);
* `tests/`: evaluator, reset and manifest tests; `tools/`: fixture and digest builders;
* `PROVENANCE.md`: origin, prior art, review and licence.

## Source and changes

These tasks were written by Cua in its private benchmark repository and are
copied from revision `16a1937a79ff2ee2e48f5b5d9bb5e6f944119b5a`, the revision
every registered Bench v2 run used. The copy differs from that revision only in
licence and provenance metadata:

* the `license` field of every `package.json` and `package-lock.json` is now
  `MIT` (it was a proprietary licence reference);
* `PROVENANCE.md` and the `provenance` block of `task.cuabench.json` describe
  the origin for a public reader, and each task's `README.md` no longer links to
  documents of the private repository;
* the manifest digests are refreshed for those files, and the `cdb-s04` oracle's
  pinned digests of the archive site's `package.json` and `package-lock.json`
  are updated to the relicensed files.

Briefs, apps, fixtures, reset scripts, launch descriptors, evaluators and the
rest of the oracles are byte-identical to the source revision. The tree digests
of the registered runs are kept in `pins.json` `cdb_pack.registered_runs_tree_sha256`;
`pins.json` `cdb_pack.tree_sha256` holds the digests of this copy, which the
preflight checks.

## Checks

From a task directory:

```bash
python3 -m unittest discover -s tests          # evaluator, reset and manifest tests
python3 tools/refresh_manifest_digests.py --check
(cd apps/<app> && node --test tests/*.test.js)  # app store tests; no install needed
```

After changing a task-owned input, run `tools/refresh_manifest_digests.py`,
the tests above, and `tools/validate_cdb.py` from the harness in a VM, then
update `pins.json` `cdb_pack.tree_sha256`. A change to a task that a registered
run used needs a dated amendment before the next run.

## Adding tasks

New multi-app tasks go into `tasks/<platform>/<id>/` with the layout above and a
`PROVENANCE.md`, plus a selector in `../automated-eval/macos_bench/probes/<ID>/task.json`
(`"kind": "cdb"`, `"pack_task"`, `"category"`, `"coding_tools": false`) and the
tree digest in `pins.json`. They join `bench_v2.V2_TASKS` only through an
amendment registered before their first trial.

## Licence

MIT, under the repository [LICENSE](../../../LICENSE.md). Copyright (c) 2026 Cua
AI, Inc. Third-party software that the apps install is listed in
[`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md); none of it is vendored.
