# Lume

CLI and framework for macOS and Linux VMs using Apple Virtualization Framework.

**[Documentation](https://cua.ai/docs/lume)** - Installation, guides, and API reference.

## Vanilla macOS VMs

Create a fresh macOS VM from an Apple restore image with the offline unattended
setup enabled:

```bash
lume create macos-tahoe --ipsw ~/Downloads/macos-tahoe.ipsw --unattended tahoe
lume run macos-tahoe
```

The built-in `sequoia` and `tahoe` presets configure the installed guest
without GUI automation. They create the `lume` user, enable SSH, configure
autologin, and disable sleep and screen locking. The default SSH credentials
are `lume` / `lume`.

Tahoe is E2E verified from a local IPSW. Sequoia can still present the
Accessibility step of Setup Assistant on its first display boot; this is
tracked in [#2155](https://github.com/trycua/cua/issues/2155).

To use the other built-in preset, replace `tahoe` with `sequoia` in the
`--unattended` option and choose a matching VM name.

## Optional dependencies

The `lume sip` command uses `vncdotool` to control macOS Recovery over VNC.
Install it only if you need to enable or disable SIP:

```bash
pip3 install vncdotool
```

Other Lume commands do not require this package.
