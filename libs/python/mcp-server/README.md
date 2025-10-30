<div align="center">
<h1>
  <div class="image-wrapper" style="display: inline-block;">
    <picture>
      <source media="(prefers-color-scheme: dark)" alt="logo" height="150" srcset="https://raw.githubusercontent.com/trycua/cua/main/img/logo_white.png" style="display: block; margin: auto;">
      <source media="(prefers-color-scheme: light)" alt="logo" height="150" srcset="https://raw.githubusercontent.com/trycua/cua/main/img/logo_black.png" style="display: block; margin: auto;">
      <img alt="Shows my svg">
    </picture>
  </div>

[![Python](https://img.shields.io/badge/Python-333333?logo=python&logoColor=white&labelColor=333333)](#)
[![macOS](https://img.shields.io/badge/macOS-000000?logo=apple&logoColor=F0F0F0)](#)
[![Discord](https://img.shields.io/badge/Discord-%235865F2.svg?&logo=discord&logoColor=white)](https://discord.com/invite/mVnXXpdE85)
[![PyPI](https://img.shields.io/pypi/v/cua-computer?color=333333)](https://pypi.org/project/cua-computer/)

</h1>
</div>

# cua-mcp-server

cua-mcp-server is an MCP server for the Computer-Use Agent (CUA). It enables CUA to run through MCP clients such as Claude Desktop and Cursor.

## Prerequisites

- Install lume: https://github.com/trycua/cua/blob/main/libs/lume/README.md#install
- Python 3.10+
- pip, venv, setuptools

## Install

Download and run the installation script:

```bash
curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/python/mcp-server/scripts/install_mcp_server.sh | bash
```

Add this to your MCP client configuration:

```json
{
  "mcpServers": {
    "cua-agent": {
      "command": "/usr/bin/env",
      "args": [
        "bash",
        "-lc",
        "export CUA_MODEL_NAME='anthropic/claude-3-5-sonnet-20241022'; ~/.cua/start_mcp_server.sh"
      ]
    }
  }
}
```

## Development (run from a local checkout)

Use an absolute path to the repository root in the arguments below.

```json
{
  "mcpServers": {
    "cua-agent": {
      "command": "/usr/bin/env",
      "args": [
        "bash",
        "-lc",
        "export CUA_MODEL_NAME='huggingface-local/ByteDance-Seed/UI-TARS-1.5-7B'; /Users/your-username/Documents/GitHub/cua/libs/python/mcp-server/scripts/start_mcp_server.sh"
      ]
    }
  }
}
```

Notes:

- Replace `/Users/your-username/Documents/GitHub/cua` with the absolute path to your clone.
- The script sets `PYTHONPATH` for local libs and runs the server module.

## Quick Start

After configuring your MCP client, restart it and invoke one of these tools:

- Take a screenshot

```json
{
  "tool": "screenshot_cua",
  "args": {}
}
```

- Run a task

```json
{
  "tool": "run_cua_task",
  "args": { "task": "Open Safari and search for University of Toronto" }
}
```

Expected results:

- Assistant messages streamed during execution
- A final screenshot image

## Documentation

- Installation: https://trycua.com/docs/libraries/mcp-server/installation
- Configuration: https://trycua.com/docs/libraries/mcp-server/configuration
- Usage: https://trycua.com/docs/libraries/mcp-server/usage
- Tools: https://trycua.com/docs/libraries/mcp-server/tools
- Client Integrations: https://trycua.com/docs/libraries/mcp-server/client-integrations
- LLM Integrations: https://trycua.com/docs/libraries/mcp-server/llm-integrations

## Troubleshooting

Server reports disconnected in MCP client:

- Use an absolute path in the `args` command.
- Launch via `/usr/bin/env bash -lc` so the shell initializes and expands paths.
- Run the script manually to verify:
  ```bash
  /usr/bin/env bash -lc '/Users/your-username/Documents/GitHub/cua/libs/python/mcp-server/scripts/start_mcp_server.sh'
  ```

pip not found in venv:

```bash
python3 -m ensurepip --upgrade
python3 -m pip install -U pip setuptools wheel
```

Pydantic schema error related to Image:

```bash
python3 -m pip install -U "mcp>=1.2.0" "fastmcp>=0.4.7" "pydantic>=2.7,<2.12"
```

If issues persist, capture logs from your MCP client and the server startup script for diagnosis.
