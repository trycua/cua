# Capability vocabulary

Capability identifiers describe behavior without prescribing tool names,
transport, or interface shape. Task manifests use these identifiers in
`required_capabilities`; future driver manifests will use the same schema for
provided capabilities. Unknown identifiers fail schema validation before task
setup.

| Identifier | Meaning |
| --- | --- |
| `observe.screen` | Capture pixels from a desktop, display, window, or region. |
| `observe.accessibility` | Read an operating system or application accessibility tree. |
| `observe.window-metadata` | Read window identity, bounds, visibility, and focus state. |
| `act.pointer` | Deliver pointer movement, button, click, drag, or scroll input. |
| `act.keyboard` | Deliver keyboard input. |
| `act.accessibility` | Invoke actions through an accessibility interface. |
| `target.window` | Direct observation or action to a specific window. |
| `target.process` | Direct observation or action to a specific process. |
| `operate.background` | Observe or act on a target while another window owns focus. |
| `preserve.physical-pointer` | Act without moving the user's physical pointer. |
| `recover.surface-change` | Reacquire a target after its surface identity or geometry changes. |

The enum in `v0.1.0/capability.schema.json` is canonical. This table explains
the terms and must change with that schema.
