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

**cua-mcp-server** is a MCP server for the Computer-Use Agent (CUA), allowing you to run CUA through Claude Desktop or other MCP clients.

### Get started with Agent

## Prerequisites

Before installing the MCP server, you'll need to set up full Computer-Use Agent capabilities. This includes:

1. Installing the Lume CLI
2. Pulling the latest macOS CUA image
3. Starting the Lume daemon service
4. Installing the required Python libraries (Optional: only needed if you want to verify the agent is working before installing MCP server)

Make sure these steps are completed and working before proceeding with the MCP server installation.

## Easy Setup Script

If you want to simplify installation, you can use this one-liner to download and run the installation script:

```bash
curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/python/mcp-server/scripts/install_mcp_server.sh | bash
```

You can then use the script in your MCP configuration like this:

```json
{ 
  "mcpServers": {
    "cua-agent": {
      "command": "/bin/bash",
      "args": ["~/.cua/start_mcp_server.sh"],
      "env": {
        "CUA_MODEL_NAME": "anthropic/claude-3-5-sonnet-20241022"
      }
    }
  }
}
```

## Development Guide

If you want to develop with the cua-mcp-server directly without installation, you can use this configuration:

```json
{
  "mcpServers": {
    "cua-agent": {
      "command": "/bin/bash",
      "args": ["~/cua/libs/python/mcp-server/scripts/start_mcp_server.sh"],
      "env": {
        "CUA_MODEL_NAME": "huggingface-local/ByteDance-Seed/UI-TARS-1.5-7B"
      }
    }
  }
}
```

This configuration:
- Uses the start_mcp_server.sh script which automatically sets up the Python path and runs the server module
- Works with Claude Desktop, Cursor, or any other MCP client
- Automatically uses your development code without requiring installation

Just add this to your MCP client's configuration and it will use your local development version of the server.

## Docs

- [Installation](https://trycua.com/docs/libraries/mcp-server/installation)
- [Configuration](https://trycua.com/docs/libraries/mcp-server/configuration)
- [Usage](https://trycua.com/docs/libraries/mcp-server/usage)
- [Tools](https://trycua.com/docs/libraries/mcp-server/tools)
- [Client Integrations](https://trycua.com/docs/libraries/mcp-server/client-integrations)
- [LLM Integrations](https://trycua.com/docs/libraries/mcp-server/llm-integrations)