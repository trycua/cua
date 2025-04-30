<div align="center">
<h1>
  <div class="image-wrapper" style="display: inline-block;">
    <picture>
      <source media="(prefers-color-scheme: dark)" alt="logo" height="150" srcset="img/logo_white.png" style="display: block; margin: auto;">
      <source media="(prefers-color-scheme: light)" alt="logo" height="150" srcset="img/logo_black.png" style="display: block; margin: auto;">
      <img alt="Shows my svg">
    </picture>
  </div>

  [![Swift 6](https://img.shields.io/badge/Swift_6-F54A2A?logo=swift&logoColor=white&labelColor=F54A2A)](#)
  [![macOS](https://img.shields.io/badge/macOS-000000?logo=apple&logoColor=F0F0F0)](#)
  [![Discord](https://img.shields.io/badge/Discord-%235865F2.svg?&logo=discord&logoColor=white)](https://discord.com/invite/mVnXXpdE85)
</h1>
</div>


**lume** is a lightweight Command Line Interface and local API server to create, run and manage macOS and Linux virtual machines (VMs) with near-native performance on Apple Silicon, using Apple's `Virtualization.Framework`.

### Run prebuilt macOS images in just 1 step

<div align="center">
<img src="../../img/cli.png" alt="lume cli">
</div>


```bash
lume run macos-sequoia-vanilla:latest
```

## Development Environment

If you're working on Lume in the context of the CUA monorepo, we recommend using the dedicated VS Code workspace configuration:

```bash
# Open VS Code workspace from the root of the monorepo
code .vscode/lume.code-workspace
```
This workspace is preconfigured with Swift language support, build tasks, and debug configurations.

## Usage

```bash
lume <command>

Commands:
  lume create <name>            Create a new macOS or Linux VM
  lume run <name>               Run a VM
  lume ls                       List all VMs
  lume get <name>               Get detailed information about a VM
  lume set <name>               Modify VM configuration
  lume stop <name>              Stop a running VM
  lume delete <name>            Delete a VM
  lume pull <image>             Pull a macOS image from container registry
  lume push <name> <image:tag>  Push a VM image to a container registry
  lume clone <name> <new-name>  Clone an existing VM
  lume config                   Get or set lume configuration
  lume images                   List available macOS images in local cache
  lume ipsw                     Get the latest macOS restore image URL
  lume prune                    Remove cached images
  lume serve                    Start the API server

Options:
  --help     Show help [boolean]
  --version  Show version number [boolean]

Command Options:
  create:
    --os <os>            Operating system to install (macOS or linux, default: macOS)
    --cpu <cores>        Number of CPU cores (default: 4)
    --memory <size>      Memory size, e.g., 8GB (default: 4GB)
    --disk-size <size>   Disk size, e.g., 50GB (default: 40GB)
    --display <res>      Display resolution (default: 1024x768)
    --ipsw <path>        Path to IPSW file or 'latest' for macOS VMs
    --storage <name>     VM storage location to use

  run:
    --no-display                Do not start the VNC client app
    --shared-dir <dir>          Share directory with VM (format: path[:ro|rw])
    --mount <path>              For Linux VMs only, attach a read-only disk image
    --registry <url>            Container registry URL (default: ghcr.io)
    --organization <org>        Organization to pull from (default: trycua)
    --vnc-port <port>           Port to use for the VNC server (default: 0 for auto-assign)
    --recovery-mode <boolean>   For MacOS VMs only, start VM in recovery mode (default: false)
    --storage <name>            VM storage location to use

  set:
    --cpu <cores>        New number of CPU cores (e.g., 4)
    --memory <size>      New memory size (e.g., 8192MB or 8GB)
    --disk-size <size>   New disk size (e.g., 40960MB or 40GB)
    --display <res>      New display resolution in format WIDTHxHEIGHT (e.g., 1024x768)
    --storage <name>     VM storage location to use

  delete:
    --force              Force deletion without confirmation
    --storage <name>     VM storage location to use

  pull:
    --registry <url>     Container registry URL (default: ghcr.io)
    --organization <org> Organization to pull from (default: trycua)
    --storage <name>     VM storage location to use

  push:
    --additional-tags <tags...>  Additional tags to push the same image to
    --registry <url>            Container registry URL (default: ghcr.io)
    --organization <org>        Organization/user to push to (default: trycua)
    --storage <name>            VM storage location to use
    --chunk-size-mb <size>      Chunk size for disk image upload in MB (default: 512)
    --verbose                   Enable verbose logging
    --dry-run                   Prepare files and show plan without uploading
    --reassemble                Verify integrity by reassembling chunks (requires --dry-run)

  get:
    -f, --format <format> Output format (json|text)
    --storage <name>      VM storage location to use

  stop:
    --storage <name>     VM storage location to use

  clone:
    --source-storage <name> Source VM storage location
    --dest-storage <name>   Destination VM storage location

  config:
    get                  Get current configuration
    storage              Manage VM storage locations
      add <name> <path>  Add a new VM storage location
      remove <name>      Remove a VM storage location
      list               List all VM storage locations
      default <name>     Set the default VM storage location
    cache                Manage cache settings
      get                Get current cache directory
      set <path>         Set cache directory
    caching              Manage image caching settings
      get                Show current caching status
      set <boolean>      Enable or disable image caching

  serve:
    --port <port>        Port to listen on (default: 3000)
```

## Install

Install with a single command:

```bash
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/lume/scripts/install.sh)"
```

By default, Lume is installed as a background service that starts automatically on login. If you prefer to start the Lume API service manually when needed, you can use the `--no-background-service` option:

```bash
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/lume/scripts/install.sh) --no-background-service"
```

**Note:** With this option, you'll need to manually start the Lume API service by running `lume serve` in your terminal whenever you need to use tools or libraries that rely on the Lume API (such as the Computer-Use Agent).

You can also download the `lume.pkg.tar.gz` archive from the [latest release](https://github.com/trycua/lume/releases), extract it, and install the package manually.

## Prebuilt Images

Pre-built images are available in the registry [ghcr.io/trycua](https://github.com/orgs/trycua/packages). 

**Important Note (v0.2.0+):** Images are being re-uploaded with sparse file system optimizations enabled, resulting in significantly lower actual disk usage. Older images (without the `-sparse` suffix) are now **deprecated**. The last version of `lume` fully supporting the non-sparse images was `v0.1.x`. Starting from `v0.2.0`, lume will automatically pull images optimized with sparse file system support.

These images come with an SSH server pre-configured and auto-login enabled.

For the security of your VM, change the default password `lume` immediately after your first login.

| Image | Tag | Description | Logical Size |
|-------|------------|-------------|------|
| `macos-sequoia-vanilla` | `latest`, `15.2` | macOS Sequoia 15.2 image | 20GB |
| `macos-sequoia-xcode` | `latest`, `15.2` | macOS Sequoia 15.2 image with Xcode command line tools | 22GB |
| `macos-sequoia-cua` | `latest`, `15.3` | macOS Sequoia 15.3 image compatible with the Computer interface | 24GB |
| `ubuntu-noble-vanilla` | `latest`, `24.04.1` | [Ubuntu Server for ARM 24.04.1 LTS](https://ubuntu.com/download/server/arm) with Ubuntu Desktop | 20GB |

For additional disk space, resize the VM disk after pulling the image using the `lume set <name> --disk-size <size>` command. Note that the actual disk space used by sparse images will be much lower than the logical size listed.

## Local API Server
  
`lume` exposes a local HTTP API server that listens on `http://localhost:3000/lume`, enabling automated management of VMs.

```bash
lume serve
```

For detailed API documentation, please refer to [API Reference](docs/API-Reference.md).

## Docs

- [API Reference](docs/API-Reference.md)
- [Development](docs/Development.md)
- [FAQ](docs/FAQ.md)

## Contributing

We welcome and greatly appreciate contributions to lume! Whether you're improving documentation, adding new features, fixing bugs, or adding new VM images, your efforts help make lume better for everyone. For detailed instructions on how to contribute, please refer to our [Contributing Guidelines](CONTRIBUTING.md).

Join our [Discord community](https://discord.com/invite/mVnXXpdE85) to discuss ideas or get assistance.

## License

lume is open-sourced under the MIT License - see the [LICENSE](LICENSE) file for details.

## Trademarks

Apple, macOS, and Apple Silicon are trademarks of Apple Inc. Ubuntu and Canonical are registered trademarks of Canonical Ltd. This project is not affiliated with, endorsed by, or sponsored by Apple Inc. or Canonical Ltd.
