# cua-agent-setup

Agent onboarding for cua. Detects installed AI coding agents, installs the
bundled cua skills into their skills directories, and registers the cua MCP
server (`cua mcp`) in their MCP configs. Used by `cua agents ...`,
`cua auth login` onboarding, the SDK (`Cua.agent_setup()`) and the Spaces app.

## Behavior

- **Detect** (read-only): an agent is installed when one of its binaries is on
  PATH, one of its config dirs exists, or (macOS) its app bundle is in
  `/Applications` or `~/Applications`.
- **Skills**: each bundled skill is copied to `<skills dir>/<name>/`. Agents
  that read the cross-agent `~/.agents/skills` share one copy. A folder cua
  did not write, or one edited since, is skipped unless forced (the old folder
  is then kept as `<name>.cua-backup-<time>`).
- **MCP**: the config is parsed and edited structurally. JSON, JSONC and
  JSON5-style files go through a concrete syntax tree (comments, trailing
  commas and formatting kept); TOML through `toml_edit`; YAML (Goose,
  Hermes) by splicing the entry's lines under its top-level mapping, so
  comments and every other line stay byte for byte (the result must parse
  back to exactly the expected document; other layouts are edited only when
  the file has no comments to lose). Other servers are never touched. A
  malformed file is reported and left as is. The first write to each file
  takes `<file>.cua-backup-<time>`; writes are atomic, keep the file mode
  (new files are 0600) and go through symlinks.
- **State**: `~/.cua/agent-setup.json` (or `$CUA_HOME`) records each entry and
  skill folder cua wrote. `remove` deletes only entries that still hold the
  value cua wrote (restoring a pre-existing non-cua entry of the same name)
  and only skill folders nobody edited. `update` refreshes skill folders cua
  owns and re-points managed MCP entries.
- **Host safety**: everything resolves from a `HostEnv` (HOME, XDG and agent
  override variables, PATH, app folders). `HostEnv::isolated(home)` is fully
  hermetic; all tests use it with a temporary home.

## Agents

User scope only. Sources checked 2026-09-22 (Hermes: 2026-09-30). "Unverified" items are best
effort and listed in `registry.rs`.

| Agent | Skills dir | MCP config (key) | Entry |
|---|---|---|---|
| Claude Code | `~/.claude/skills` (`$CLAUDE_CONFIG_DIR/skills`) | `~/.claude.json` (`mcpServers`); written with `claude mcp add-json --scope user` when `claude` is on PATH | `type: stdio`, command, args, env |
| OpenAI Codex | `~/.agents/skills` | `~/.codex/config.toml` (`$CODEX_HOME`), `[mcp_servers.cua]` | command, args, env |
| Cursor | `~/.agents/skills` | `~/.cursor/mcp.json` (`mcpServers`) | `type: stdio`, command, args, env |
| Gemini CLI | `~/.agents/skills` | `~/.gemini/settings.json` (`$GEMINI_CLI_HOME/.gemini`) (`mcpServers`) | command, args, env |
| Cline | `~/.cline/skills` | `~/.cline/data/settings/cline_mcp_settings.json` (`$CLINE_DATA_DIR`) (`mcpServers`) | command, args, env, `disabled: false` |
| Kiro | `~/.kiro/skills` | `~/.kiro/settings/mcp.json` (`mcpServers`) | command, args, env, `disabled: false` |
| OpenClaw | `~/.agents/skills` | `~/.openclaw/openclaw.json` JSON5 (`mcp.servers`) | command, args, env |
| OpenCode | `~/.agents/skills` | `~/.config/opencode/opencode.json[c]` (`mcp`) | `type: local`, `command: [cmd, ...args]`, environment, `enabled` |
| Pi | `~/.agents/skills` | none (Pi has no MCP) | |
| Windsurf (Devin Desktop) | `~/.agents/skills` | `~/.config/devin/mcp_config.json` (`mcpServers`) | command, args, env |
| GitHub Copilot CLI | `~/.agents/skills` | `~/.copilot/mcp-config.json` (`$COPILOT_HOME`) (`mcpServers`) | `type: local`, command, args, env, `tools: ["*"]` |
| Amp | `~/.agents/skills` | `~/.config/amp/settings.json` (`amp.mcpServers`, a literal key) | command, args, env |
| Goose | `~/.agents/skills` | `~/.config/goose/config.yaml` (`extensions`) | `type: stdio`, name, cmd, args, enabled, envs, timeout |
| Zed | `~/.agents/skills` | `~/.config/zed/settings.json` (`context_servers`) | command, args, env |
| VS Code (Copilot agent mode) | `~/.agents/skills` | `<Code user dir>/mcp.json` (`servers`) (path unverified) | `type: stdio`, command, args, env |
| Google Antigravity | `~/.gemini/config/skills` | `~/.gemini/config/mcp_config.json` (`mcpServers`) | command, args, env |
| Hermes | `~/.hermes/skills` (`$HERMES_HOME/skills`; `%LOCALAPPDATA%\hermes\skills` on Windows) | `~/.hermes/config.yaml` (`$HERMES_HOME`; `%LOCALAPPDATA%\hermes` on Windows) (`mcp_servers`) | command, args, env |

Not supported: Continue (global MCP config is a YAML list; no documented
skills), Roo Code (global MCP file location undocumented).

Hermes reads `~/.agents/skills` only when it is listed in
`skills.external_dirs`, so it gets its own copies in `~/.hermes/skills`. Its
`config.yaml` is seeded from a long commented template, which the YAML splice
keeps intact. Hermes passes stdio MCP servers a filtered environment (PATH,
HOME and a few others plus the entry's `env`); `cua mcp` reads its
credentials from `~/.cua`, so it needs nothing more. Named profiles
(`hermes profile use`) are not configured: only `$HERMES_HOME` or the
default home. Hermes picks up a new server in a new session (or
`/reload-mcp`).

Cursor, OpenCode and VS Code also load `~/.claude/skills`. When Claude Code is
set up too (or cua's copies are already there), they are served from
`~/.claude/skills` and get no second copy in `~/.agents/skills`. They can still
list a skill twice when another agent that reads only `~/.agents/skills`
(Codex, for example) is set up as well.

### Sources

- Claude Code: https://code.claude.com/docs/en/skills, https://code.claude.com/docs/en/mcp
- Codex: https://learn.chatgpt.com/docs/build-skills, https://learn.chatgpt.com/docs/extend/mcp?surface=cli, https://learn.chatgpt.com/docs/config-file/environment-variables
- Cursor: https://cursor.com/docs/context/skills, https://cursor.com/docs/context/mcp, https://cursor.com/docs/cli/mcp
- Gemini CLI: https://geminicli.com/docs/cli/skills/, https://geminicli.com/docs/tools/mcp-server/, https://geminicli.com/docs/reference/configuration/
- Cline: https://docs.cline.bot/customization/skills, https://docs.cline.bot/getting-started/config, https://docs.cline.bot/cli/cli-reference
- Kiro: https://kiro.dev/docs/skills/, https://kiro.dev/docs/mcp/configuration/
- OpenClaw: https://docs.openclaw.ai/tools/skills, https://docs.openclaw.ai/gateway/config-extensions, https://docs.openclaw.ai/help/environment
- OpenCode: https://opencode.ai/docs/skills/, https://opencode.ai/docs/mcp-servers/, https://opencode.ai/docs/config/
- Pi: https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/skills.md
- Windsurf / Devin Desktop: https://docs.devin.ai/desktop/cascade/skills, https://docs.devin.ai/desktop/cascade/mcp
- Copilot CLI: https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-skills, https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-mcp-servers
- Amp: https://ampcode.com/docs/customize/skills, https://ampcode.com/docs/customize/mcp
- Goose: https://goose-docs.ai/docs/guides/context-engineering/using-skills/, https://goose-docs.ai/docs/guides/config-files/
- Zed: https://zed.dev/docs/ai/skills, https://zed.dev/docs/ai/mcp
- VS Code: https://code.visualstudio.com/docs/copilot/customization/agent-skills, https://code.visualstudio.com/docs/copilot/customization/mcp-servers
- Antigravity: https://antigravity.google/docs/skills/, https://antigravity.google/docs/mcp?app=antigravity-cli
- Hermes: https://hermes-agent.nousresearch.com/docs/user-guide/features/skills, https://hermes-agent.nousresearch.com/docs/reference/mcp-config-reference, https://hermes-agent.nousresearch.com/docs/getting-started/installation (install layout); the home and config path from `hermes_constants.py` (`_get_platform_default_hermes_home`) and `hermes_cli/config.py` (`get_config_path`), the `mcp_servers` loader from `tools/mcp_tool_config.py` (`_load_mcp_config`) in NousResearch/hermes-agent, checked 2026-09-30

## Bundled skills

`libs/cua/skills/` is bundled with `include_dir`:

- `cua-driver` and `gui-automation` are copies of
  `libs/cua-driver/rust/Skills/cua-driver` and `skills/gui-automation`.
  `scripts/sync-skills.sh` refreshes them; `--check` fails on drift.
- `cua-sandboxes`, `cua-spaces` and `cua-volume` live only here.

## Tests

```bash
cargo test -p cua-agent-setup
```

Every test runs in a temporary home with `HostEnv::isolated`.
