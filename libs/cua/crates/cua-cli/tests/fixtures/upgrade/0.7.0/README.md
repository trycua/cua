# cua 0.7.0 command tree

`cli-tree.txt` lists every `cua agent`, `cua volume`, `cua daemon` and
`cua mcp` subcommand, alias and flag that cua 0.7.0 accepted. It was read from
the `--help` output of the `cua` in Cua Spaces 0.13.1 (the binary and how it
was obtained are described in
`libs/cua/crates/cua-spaces-ext/tests/fixtures/upgrade/0.7.0/README.md`). A
script walked `cua <path> --help` for each subcommand and wrote one line per
command (`path`), alias (`path = alias`) and flag (`path :: --flag`, `-f`).
`src/upgrade_070_tests.rs` fails when any of them stops parsing.

`mcp-tools.txt` is the `tools/list` of that binary's `cua daemon mcp` (the last
line of the Spaces fixture's `golden/mcp.jsonl`); the same test fails when a
tool name is no longer served.
