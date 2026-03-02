# Cua CLI (TypeScript) - DEPRECATED

> **This package is deprecated.** Please use the Python CLI instead:
>
> ```bash
> pip install cua-cli
> ```
>
> The Python CLI provides the same functionality with better integration into the Cua ecosystem.
> See the [Quickstart Guide](https://cua.ai/docs/cua/guide/get-started/quickstart) for installation and usage.

---

The TypeScript SDKs (`@trycua/computer`, `@trycua/agent`) are still actively maintained for building applications. Only the CLI is deprecated.

## Migration

| TypeScript CLI           | Python CLI               |
| ------------------------ | ------------------------ |
| `cua auth login`         | `cua auth login`         |
| `cua sandbox list`       | `cua sandbox list`       |
| `cua sandbox create`     | `cua sandbox create`     |
| `cua sandbox vnc <name>` | `cua sandbox vnc <name>` |
| `cua image list`         | `cua image list`         |
| `cua skills list`        | `cua skills list`        |
| `cua serve-mcp`          | `cua serve-mcp`          |

Commands are identical - just install the Python CLI and continue using the same commands.
