# Cua Driver agent plugin

`cua-driver` has one generated plugin package for Claude Code, Grok Build, and
Codex CLI:

```text
plugins/cua-driver/
├── .claude-plugin/plugin.json
├── .codex-plugin/plugin.json
├── .grok-plugin/plugin.json
├── .mcp.json
└── skills/cua-driver/
```

The package uses the portable parts of the agent integration as its core:

- Agent Skills instructions from `rust/Skills/cua-driver`
- The local stdio MCP entry point `cua-driver mcp`
- Small vendor manifests generated from the Cua Driver release version

Marketplace catalog entries remain vendor-specific. They should point to this
same package at a pinned repository commit instead of copying its contents.

## Generate and validate

Regenerate the package after changing the canonical skill or plugin metadata:

```bash
python3 libs/cua-driver/scripts/generate-agent-plugin.py
```

CI-style drift check:

```bash
python3 libs/cua-driver/scripts/generate-agent-plugin.py --check
```

Validate the portable skill and the vendor manifests with the clients available
on the current host:

```bash
python3 /path/to/skill-creator/scripts/quick_validate.py \
  libs/cua-driver/plugins/cua-driver/skills/cua-driver
python3 /path/to/plugin-creator/scripts/validate_plugin.py \
  libs/cua-driver/plugins/cua-driver
claude plugin validate --strict libs/cua-driver/plugins/cua-driver
grok plugin validate libs/cua-driver/plugins/cua-driver
```

The repository test does not require vendor CLIs:

```bash
pytest -q libs/cua-driver/scripts/tests/test_generate_agent_plugin.py
```

## Distribution model

The plugin does not install the native Cua Driver binary. Marketplace listings
must direct users to the Cua installation guide and must not add hooks or
scripts that download and execute a binary.

- **Claude:** submit the package as a Claude Code plugin or reference it from a
  Claude marketplace.
- **Grok Build:** add one pinned remote-source entry to the official xAI plugin
  marketplace. Grok can consume the Claude-compatible layout, while the explicit
  Grok manifest keeps the package self-describing.
- **Codex CLI:** reference the same package from a Codex marketplace. The Codex
  manifest adds interface metadata while reusing the same skill and `.mcp.json`.
- **OpenAI public directory:** the skills are portable, but the public submission
  flow does not currently support this local stdio MCP server. Do not replace it
  with a public desktop-control endpoint merely to satisfy that transport.

For a release update, change the canonical Cua Driver version normally. Release
automation updates all three generated manifests, and the generator drift test
ensures their remaining contents still match the canonical package.
