# Pinned upstream schema

`schema-2026-07-28.json` copies the JSON schema from
<https://raw.githubusercontent.com/modelcontextprotocol/modelcontextprotocol/e76e9c572c6f2bfcb730357101acc90f2f802e02/schema/2026-07-28/schema.json>.

Upstream SHA-256: `ef70b61f99b6d2e5e3b46863822eab08dff6a45bedc7a08914e0e5b133f40203`.

`schema-LICENSE` reproduces the upstream license notice at that commit. This
fixture retains the upstream licensing terms, including its transition notice.

The schema covers the base MCP protocol. SEP-2640 Skills remains an extension:
the probe checks its list/get structure, metadata, manifest, content integrity,
and frontmatter explicitly rather than claiming base-schema validation of it.

The local fixture normalizes trailing blank lines to one final newline; its JSON
content is unchanged. Local SHA-256: `e8c8ae56f7dd8eab465fb2740db54042761c4f9e0e19b23b98504cec27f4c91d`.
