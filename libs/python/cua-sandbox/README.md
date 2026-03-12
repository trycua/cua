# cua-sandbox

Sandboxed VM and container environments with a unified Python API.

```python
import asyncio
from cua_sandbox import Image, sandbox

async def main():
    # Windows VM
    image = Image.windows("11").winget_install("Git.Git", "VSCode")
    async with sandbox(image) as win:
        await win.shell.run("ver")
        await win.mouse.click(500, 300)

    # macOS VM
    image = Image.macos("15").run("brew install jq")
    async with sandbox(image) as mac:
        await mac.shell.run("sw_vers")
        await mac.screenshot()

    # Linux VM
    image = Image.linux("ubuntu", "24.04").apt_install("curl", "jq")
    async with sandbox(image) as vm:
        await vm.shell.run("uname -a")
        await vm.keyboard.type("hello")

    # Linux VM (from OCI registry)
    image = Image.from_registry("ghcr.io/cirruslabs/ubuntu:latest")
    async with sandbox(image) as ctr:
        await ctr.shell.run("cat /etc/os-release")
        await ctr.file.upload("data.csv", "/tmp/data.csv")

asyncio.run(main())
```
