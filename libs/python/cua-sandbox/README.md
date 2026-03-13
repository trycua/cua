# cua-sandbox

Sandboxed VM environments with a unified Python API. Cloud by default.

```bash
pip install cua-sandbox
```

## Localhost (unsandboxed)

Direct host control — **not sandboxed**, use with caution.

```python
from cua_sandbox import localhost

async with localhost() as host:
    await host.shell.run("echo hello")
    await host.screenshot()
```

## Ephemeral Cloud VM

Created on enter, destroyed on exit.

```python
from cua_sandbox import sandbox, Image

async with sandbox(image=Image.linux()) as sb:
    await sb.shell.run("uname -a")
    await sb.screenshot()
```

## Persistent Cloud VM

Connect to an existing VM by name. Not destroyed on exit.

```python
from cua_sandbox import sandbox

async with sandbox(name="my-vm") as sb:
    result = await sb.shell.run("whoami")
    print(result.stdout)
```

## Ephemeral Local VM

Spins up a local VM using QEMU or Lume, destroyed on exit.

```python
from cua_sandbox import sandbox, Image
from cua_sandbox.runtime import QEMURuntime

async with sandbox(local=True, image=Image.linux()) as sb:
    await sb.shell.run("uname -a")
```
