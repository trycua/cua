# Cua Driver 0.12.6 compatibility fixtures

These fixtures lock selected stable public contract fields from the
`cua-driver-rs-v0.12.6` release tag at commit
`9eb1f481b8a12cd6ffda2ad5af21653a9e5aa9e5`.

The snapshots were derived from the release-tagged package sources, generated
bindings, `cua-driver --help`, `cua-driver manifest`, and MCP JSON-RPC responses.
The corresponding tests compare semantic fields instead of whole-process
output. This intentionally excludes executable paths, socket paths, PIDs,
session identifiers, platform-specific prose, and other volatile values.
Release version fields are checked for their documented shape rather than
frozen to `0.12.6`, because a compatible later release must change them.

- `python-package.json` locks package-root exports and callable signatures.
- `typescript-package.json` locks package subpath exports, declaration exports,
  and the generated `CuaDriver` declaration methods.
- `cli.json` locks the CLI help header/catalog and selected manifest fields.
- `mcp.json` locks initialize, tools-list envelope/tool fields, and the
  method-not-found error category.

Additive manifest fields, tools, and package metadata are permitted. Removing
or changing an item recorded here requires an explicit compatibility decision
and an intentional fixture update.
