# Harness marks

The real marks for the coding-agent harnesses the AGENTS panel can run, used to
tell one harness from another at a glance, the same job an app's icon does in
the window rows above it.

Neither harness ships a mark we could read locally: both are npm CLIs, not
`.app` bundles, so there is no `NSWorkspace.iconForFile` to ask (which is how
`space_app_icon` gets a guest app's real icon). These are the official marks,
taken from Wikimedia Commons:

| file                | source                              | licence       |
|---------------------|-------------------------------------|---------------|
| `claude-code.svg`   | Commons `File:Claude AI symbol.svg` | CC0           |
| `openai-codex.svg`  | Commons `File:OpenAI logo 2025 (symbol).svg` | Public domain |

Both are used nominatively, to identify which harness a run belongs to. Local
edits are limited to making them themeable: the Claude mark's colour is pinned
to its brand coral as a hex (the upload carried an `hsl()` and a Tailwind
class), and the OpenAI mark is set to `currentColor` so the panel can colour it
per theme instead of baking one in. No path data was changed.

A harness with no mark here gets no stand-in: `AgentGlyph` renders nothing
rather than a generic glyph that would misidentify it.
