# CUA CLI

Unified command-line interface for CUA (Computer-Use Agents).

## Installation

```bash
pip install cua-cli
```

## Usage

```bash
# Authentication
cua auth login              # Authenticate via browser
cua auth login --api-key    # Authenticate with API key
cua auth logout             # Clear credentials
cua auth env                # Export API key to .env file

# Sandbox Management
cua sb list                 # List all sandboxes
cua sb create --os linux --size medium --region north-america
cua sb get <name>           # Get sandbox details
cua sb start <name>         # Start a stopped sandbox
cua sb stop <name>          # Stop a running sandbox
cua sb restart <name>       # Restart a sandbox
cua sb suspend <name>       # Suspend a sandbox
cua sb delete <name>        # Delete a sandbox
cua sb vnc <name>           # Open sandbox in browser

# Image Management
cua image list              # List cloud images
cua image list --local      # List local images
cua image push <name>       # Upload image to cloud
cua image pull <name>       # Download image from cloud
cua image delete <name>     # Delete cloud image

# Skills Management
cua skills list             # List recorded skills
cua skills read <name>      # Read a skill's content
cua skills record <name>    # Record a new skill
cua skills replay <name>    # Replay a skill
cua skills delete <name>    # Delete a skill
cua skills clean            # Delete all skills

# MCP Server (for AI assistants)
cua serve-mcp               # Start MCP server with all permissions
cua serve-mcp --permissions sandbox:all,computer:readonly
```

## Installation Options

```bash
# Basic installation
pip install cua-cli

# With MCP server support
pip install cua-cli[mcp]

# With skills recording (VLM captioning)
pip install cua-cli[skills]

# Full installation
pip install cua-cli[all]
```

## MCP Integration

To use CUA with Claude Code or other MCP-compatible AI assistants:

```bash
# Add CUA as an MCP server
claude mcp add cua -- cua serve-mcp

# With specific permissions
claude mcp add cua -- cua serve-mcp --permissions sandbox:all,computer:readonly

# With a default sandbox
claude mcp add cua -- cua serve-mcp --sandbox my-sandbox
```

### Available Permissions

- `all` - All permissions
- `sandbox:all` - Full sandbox management
- `sandbox:readonly` - List and get sandboxes only
- `computer:all` - Full computer control
- `computer:readonly` - Screenshots only
- `skills:all` - Full skills management
- `skills:readonly` - List and read skills only

Individual permissions: `sandbox:list`, `sandbox:create`, `sandbox:delete`, `sandbox:start`, `sandbox:stop`, `sandbox:restart`, `sandbox:suspend`, `sandbox:get`, `sandbox:vnc`, `computer:screenshot`, `computer:click`, `computer:type`, `computer:key`, `computer:scroll`, `computer:drag`, `computer:hotkey`, `computer:clipboard`, `computer:file`, `computer:shell`, `computer:window`, `skills:list`, `skills:read`, `skills:record`, `skills:delete`

## Environment Variables

- `CUA_API_KEY`: API key for authentication
- `CUA_API_BASE`: API base URL (default: https://api.cua.ai)
- `CUA_WEBSITE_URL`: Website URL for OAuth (default: https://cua.ai)
- `CUA_MCP_PERMISSIONS`: Default MCP permissions (comma-separated)
- `CUA_SANDBOX`: Default sandbox name for computer commands
