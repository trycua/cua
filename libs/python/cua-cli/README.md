# CUA CLI

Unified command-line interface for CUA (Computer-Use Agents).

## Installation

```bash
pip install cua-cli
```

## Usage

```bash
# Authentication
cua auth login              # Authenticate with run.cua.ai device authorization
cua auth login --no-browser # Print the verification URL without opening a browser
cua auth status             # Show local session status
cua auth logout             # Revoke the refresh token and remove local credentials

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

- Authentication uses OIDC device authorization discovered from `https://auth.cua.ai/realms/cyclops-cs`, while authenticated cloud requests use `https://run.cua.ai`. Tokens are stored in the operating system credential vault; the CLI does not read API keys from environment variables or write them to `.env` files.
- `CUA_MCP_PERMISSIONS`: Default MCP permissions (comma-separated)
- `CUA_SANDBOX`: Default sandbox name for computer commands

## Cloud Authentication

`cua auth login` uses the standard OAuth device authorization flow. It discovers Keycloak endpoints from `https://auth.cua.ai/realms/cyclops-cs`, while the short verification UI is served from `https://run.cua.ai/device`. The CLI prints a verification URL and code, so it also works over SSH or in CI-style terminals; use `--no-browser` to prevent an automatic browser attempt. Complete the verification in any browser, then return to the terminal while the CLI polls for approval.

Access tokens refresh automatically before authenticated `run.cua.ai` requests. `cua auth logout` asks the issuer to revoke the refresh token and always removes the local credential-vault entry, even when the network is unavailable. The CLI never prints access or refresh tokens.
