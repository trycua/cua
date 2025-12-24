---
title: FAQ
---

### Where are the VMs stored?

VMs are stored in `~/.lume` by default. You can configure additional storage locations using the `lume config` command.

### How are images cached?

Images are cached in `~/.lume/cache`. When doing `lume pull <image>`, it will check if the image is already cached. If not, it will download the image and cache it, removing any older versions.

### Where is the configuration file stored?

Lume follows the XDG Base Directory specification for the configuration file:

- Configuration is stored in `$XDG_CONFIG_HOME/lume/config.yaml` (defaults to `~/.config/lume/config.yaml`)

By default, other data is stored in:

- VM data: `~/.lume`
- Cache files: `~/.lume/cache`

The config file contains settings for:

- VM storage locations and the default location
- Cache directory location
- Whether caching is enabled

You can view and modify these settings using the `lume config` commands:

```bash
# View current configuration
lume config get

# Manage VM storage locations
lume config storage list                 # List all VM storage locations
lume config storage add <name> <path>    # Add a new VM storage location
lume config storage remove <name>        # Remove a VM storage location
lume config storage default <name>       # Set the default VM storage location

# Manage cache settings
lume config cache get                    # Get current cache directory
lume config cache set <path>             # Set cache directory

# Manage image caching settings
lume config caching get                  # Show current caching status
lume config caching set <boolean>        # Enable or disable image caching
```

### How do I use multiple VM storage locations?

Lume supports storing VMs in different locations (e.g., internal drive, external SSD). After configuring storage locations, you can specify which location to use with the `--storage` parameter in various commands:

```bash
# Create a VM in a specific storage location
lume create my-vm --os macos --ipsw latest --storage ssd

# Run a VM from a specific storage location
lume run my-vm --storage ssd

# Delete a VM from a specific storage location
lume delete my-vm --storage ssd

# Pull an image to a specific storage location
lume pull macos-sequoia-vanilla:latest --name my-vm --storage ssd

# Clone a VM between storage locations
lume clone source-vm cloned-vm --source-storage default --dest-storage ssd
```

If you don't specify a storage location, Lume will use the default one or search across all configured locations.

### Are VM disks taking up all the disk space?

No, macOS uses sparse files, which only allocate space as needed. For example, VM disks totaling 50 GB may only use 20 GB on disk.

### How do I get the latest macOS restore image URL?

```bash
lume ipsw
```

### How do I delete a VM?

```bash
lume delete <name>
```

### How to Install macOS from an IPSW Image

#### Create a new macOS VM using the latest supported IPSW image:

Run the following command to create a new macOS virtual machine using the latest available IPSW image:

```bash
lume create <name> --os macos --ipsw latest
```

#### Create a new macOS VM using a specific IPSW image:

To create a macOS virtual machine from an older or specific IPSW file, first download the desired IPSW (UniversalMac) from a trusted source.

Then, use the downloaded IPSW path:

```bash
lume create <name> --os macos --ipsw <downloaded_ipsw_path>
```

### How do I install a custom Linux image?

The process for creating a custom Linux image differs than macOS, with IPSW restore files not being used. You need to create a linux VM first, then mount a setup image file to the VM for the first boot.

```bash
lume create <name> --os linux

lume run <name> --mount <path-to-setup-image>

lume run <name>
```
