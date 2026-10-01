# @trycua/computer

Computer-use interface for controlling local and legacy Cloud macOS, Linux, and
Windows sandboxes.

## Use Fleet for sandbox lifecycle

New applications should use `@trycua/fleet` to create templates, pools, and
claims and to send authenticated requests to services inside Fleet sandboxes:

```bash
npm install @trycua/fleet
```

Use the runtime-specific entry point in application code:

```ts
import { createFleetClient } from '@trycua/fleet/node';
// Browser applications use @trycua/fleet/browser.
```

Do not call the legacy VM API or implement a second copy of Fleet lifecycle
logic through `@trycua/computer`. See
[Create a sandbox pool with TypeScript](https://cua.ai/docs/how-to-guides/sandbox/create-pool-with-typescript)
for complete Node.js and browser examples, including screenshots.

`@trycua/computer` remains available for existing computer-control integrations.
Its Fleet compatibility provider delegates lifecycle operations to
`@trycua/fleet`; new Fleet integrations should use the Fleet package directly.

**[Documentation](https://cua.ai/docs/cua/reference/computer-sdk)** - Installation, guides, and configuration.
