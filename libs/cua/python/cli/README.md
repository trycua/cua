# cua-cli

The `cua` command line for the [`cua`](https://pypi.org/project/cua/) Python
package: the Rust `cua` CLI (sandboxes, Spaces, the cua daemon, MCP, images)
as a platform wheel.

```bash
pip install cua      # depends on cua-cli
cua --version
```

`cua` depends on this package at the same version. It installs the `cua`
binary as `cua_cli/bin/cua`, where the `cua` console command (and so the SDK,
which starts the cua daemon through it) runs it. The CLI
also installs standalone, without Python:

```bash
curl -fsSL https://cua.ai/install.sh | sh
```
