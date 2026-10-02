| Language | Package | Import |
| --- | --- | --- |
| Python | `pip install cua` ({{version}}; `cua[sandbox]` adds the [high-level API](/cua-sdk/reference/python)) | `import cua` |
| TypeScript (Node) | `npm install @trycua/cua` ({{version}}) | `import { embedded } from '@trycua/cua'` |
| Browser | `@trycua/cua/browser` (a WebAssembly subset) | `import { Cua } from '@trycua/cua/browser'` |
| Swift | SwiftPM package `Cua` (macOS 12+, iOS 15+) | `import Cua` |
| Kotlin | `ai.cua.sdk` (JNA; compile-checked, not published yet) | `import ai.cua.sdk.*` |
| Rust | `cua-sdk` from git (see [Rust crates](/cua-sdk/reference/rust)) | `use cua_sdk::*;` |
