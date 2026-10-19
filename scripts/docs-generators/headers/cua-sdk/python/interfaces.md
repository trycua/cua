| Needs | Interfaces |
| --- | --- |
| Nothing in the guest | `sb.service(name)`, `sb.public_url()`, `sb.tunnel`, `sb.mcp()` ([services](/cua-sdk/reference/python/services)) |
| cua-spacesd (the canonical `Image.linux()` has it) | `sb.shell`, `sb.files`, `sb.mouse`, `sb.keyboard`, `sb.screen`, `sb.clipboard`, `sb.terminal`, `sb.window`, `sb.apps` |

Without cua-spacesd these fall back to the runtime's agentless path (QMP, VNC, SSH or ADB) where one exists, and otherwise raise `SpacesdNotAvailable`. Coordinates are screen pixels; input goes through Cua Driver in the guest.
