# Cua XFCE Container

Vanilla XFCE desktop container for Computer-Using Agents.

The image includes both `cua-computer-server` and the released `cua-driver`
Python SDK with its bundled executable. Computer-server remains the active
remote API on port 8000; Cua Driver is installed for co-located applications
and future transport-adapter experiments but is not started automatically.
The pinned Driver version is declared in
[`requirements-cua-driver.txt`](requirements-cua-driver.txt).

**[Documentation](https://cua.ai/docs/cua/reference/desktop-sandbox/linux-container/xfce)** - Setup and configuration.
