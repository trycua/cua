# Cua Driver tools for an Eve agent

This example gives an [Eve](https://eve.dev) agent four Cua Driver tools that
sit on the model's tool list from the first turn: `get_desktop_state`,
`click`, `type_text` and `press_key`. Each tool is an Eve
[`defineTool`](https://eve.dev/docs/tools.md) file that calls the in-process
`@trycua/cua-driver` TypeScript SDK. The tool set and its action rules match
[`../agent-sdks/native-tools.ts`](../agent-sdks/native-tools.ts), the Claude
Agent SDK adapter.

This is an example, not a supported SDK entry point. Issue
[#4267](https://github.com/trycua/cua/issues/4267) tracks first-class Eve
support.

## Why native tools instead of an MCP connection

Eve [connections](https://eve.dev/docs/connections.md) reach MCP or OpenAPI
servers by URL. The model does not see connection tools on its
default tool list. It finds them through the built-in `connection_search` tool
and calls them as `<connection>__<tool>`. `cua-driver mcp` is a local stdio
server, so Eve needs an HTTP bridge to reach it, and the agent still has to
search before its first desktop action.

Eve runs authored tools in the app runtime, so a tool file can import the
Driver SDK directly. The model sees the Driver tools immediately, and no MCP
bridge is needed.

## Layout

```text
agent/
├── agent.ts               model, and keeps the native SDK out of eve's bundle
├── instructions.md        observe, act, verify; never blindly retry
├── lib/desktop.ts         one shared Driver client, bounded calls
└── tools/
    ├── get_desktop_state.ts
    ├── click.ts
    ├── type_text.ts
    └── press_key.ts
```

Eve names each tool after its file under `agent/tools/`. Every action tool
returns a fresh desktop observation. `toModelOutput` sends the Driver's text
summary and each screenshot to the model as content parts.

## Run it

You need Node.js 24 or newer. The Eve app runtime must run on the computer it
controls, for example through `eve dev` or a self-hosted Eve service on that
machine. A hosted deployment cannot reach your desktop.

```bash
npm install
npm run dev
```

In the Eve terminal UI, connect a vision-capable model. Then give the agent a
task, for example:

```text
Open Calculator, compute 19 * 23, and tell me the result.
```

On macOS, grant Screen Recording and Accessibility to the app that runs
`npm run dev`, such as your terminal. `agent/agent.ts` selects
`anthropic/claude-opus-5.5`; change `model` to use another vision-capable
model.

## Behavior to know before you rely on it

- **One Driver client per process.** `agent/lib/desktop.ts` creates one
  `CuaDriver` client on first use. All Eve sessions served by that process
  share its implicit Driver session, just as they share the one desktop.
- **Screenshots stay in history.** Eve keeps each image in session history
  and re-sends it on later model calls, and it warns about payloads above
  3 MiB. Long
  sessions grow quickly, so start a new session for each unrelated task.
- **Interrupted steps can re-run.** Eve replays completed steps from its
  record, but a step interrupted mid-execution runs again. A desktop action
  can therefore repeat after a crash or restart. Keep irreversible tasks
  attended, or gate those tools with `approval` from `eve/tools/approval`.
- **Unknown outcomes are reported, not retried.** When an action times out or
  reports an error, the tool returns a fresh observation that starts with an
  "outcome is unknown" notice. `instructions.md` tells the model to inspect
  that observation before trying again.
- **Coordinates only.** Like `native-tools.ts`, these tools act on absolute
  desktop coordinates on the primary display. Window-targeted and
  element-token tools would follow the same pattern with more SDK calls.

## Checks without a model or desktop access

```bash
npm run typecheck
npx eve info
```

`eve info` compiles the agent without calling a model. Its tool list should
include `click`, `get_desktop_state`, `press_key` and `type_text` next to
Eve's built-in tools.

## Not covered yet

- **A packaged tool set.** An Eve
  [extension](https://eve.dev/docs/extensions.md) can ship tools, skills and
  instructions in one npm package. Consumers mount it under a name, which
  becomes a tool prefix such as `cua__click`. Whether Cua ships an extension,
  a `@trycua/cua-driver/eve` subpath, or a generator is an open question in
  #4267.
- **Driver skills.** Eve reads `SKILL.md` skills from `agent/skills/`. The
  Driver skills are written for the CLI and MCP tool names, not these four
  tools, so this example does not install them.
