# cua-cli

The `cua` command line for the [`cua`](https://pypi.org/project/cua/) Python
package: the Rust `cua` CLI (sandboxes, Spaces, the cua daemon, MCP, images)
as a platform wheel.

```bash
pip install cua      # depends on cua-cli
cua --version
```

`cua` depends on this package at the same version. The wheel is a few KB:
the first `cua` run downloads the signed CLI of the same release from GitHub
Releases (about 85 MB, once), checks its SHA-256 against the one in the
wheel, and caches it.

| Variable | Use |
| --- | --- |
| `CUA_CLI_BINARY` | Run this `cua` binary; nothing is downloaded. |
| `CUA_CLI_DOWNLOAD_BASE` | Download the archives from a mirror (or `file:///dir/`); hashes still apply. |
| `CUA_CLI_CACHE_DIR` | Where downloaded CLIs are kept. |

`python -m cua_cli fetch` downloads it ahead of time, for example in a Docker
image build. The CLI also installs standalone, without Python:

```bash
curl -fsSL https://cua.ai/install.sh | sh
```
