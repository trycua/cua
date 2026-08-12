# Release-channel component registry

`components.json` is the small, shared contract for immutable release channels.
It identifies a component's Release Please entry, disjoint stable and nightly
tag namespaces, build-time version sites, builder workflow, and change paths.
It intentionally does not describe signing, packaging, or registry publishing;
those remain owned by each component workflow.

To add a component, add one descriptor, keep its stable prefix identical to its
Release Please component tag, choose a unique `nightly-` prefix, declare only
version sites that affect built nightly artifacts, and add focused tests to
`test_release_channels.py`. Run:

```bash
python3 .github/scripts/release_channels.py validate
python3 -m pytest .github/scripts/tests/test_release_channels.py
```

Nightly versions are staged only in an ephemeral CI checkout. The script must
never rewrite stable baked installer defaults or published-version pointers.
Nightly release notes reuse the repository's PR-first attribution collector.
The first nightly is bounded by the component's current stable tag; later
nightlies are bounded by the previous published nightly. A component therefore
needs a reachable stable tag before its nightly channel is enabled.
