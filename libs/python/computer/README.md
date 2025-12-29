<div align="center">
<h1>
  <div class="image-wrapper" style="display: inline-block;">
    <picture>
      <source media="(prefers-color-scheme: dark)" alt="logo" height="150" srcset="https://raw.githubusercontent.com/trycua/cua/main/img/logo_white.png" style="display: block; margin: auto;">
      <source media="(prefers-color-scheme: light)" alt="logo" height="150" srcset="https://raw.githubusercontent.com/trycua/cua/main/img/logo_black.png" style="display: block; margin: auto;">
      <img alt="Shows my svg">
    </picture>
  </div>

[![Python](https://img.shields.io/badge/Python-333333?logo=python&logoColor=white&labelColor=333333)](#)
[![macOS](https://img.shields.io/badge/macOS-000000?logo=apple&logoColor=F0F0F0)](#)
[![Discord](https://img.shields.io/badge/Discord-%235865F2.svg?&logo=discord&logoColor=white)](https://discord.com/invite/mVnXXpdE85)
[![PyPI](https://img.shields.io/pypi/v/cua-computer?color=333333)](https://pypi.org/project/cua-computer/)

</h1>
</div>

**cua-computer** is a Computer-Use Interface (CUI) framework powering Cua for interacting with local macOS and Linux sandboxes, PyAutoGUI-compatible, and pluggable with any AI agent systems (Cua, Langchain, CrewAI, AutoGen). Computer relies on [Lume](https://github.com/trycua/lume) for creating and managing sandbox environments.

### Get started with Computer

<div align="center">
    <img src="https://raw.githubusercontent.com/trycua/cua/main/img/computer.png"/>
</div>

```python
from computer import Computer

computer = Computer(os_type="macos", display="1024x768", memory="8GB", cpu="4")
try:
    await computer.run()

    screenshot = await computer.interface.screenshot()
    with open("screenshot.png", "wb") as f:
        f.write(screenshot)

    await computer.interface.move_cursor(100, 100)
    await computer.interface.left_click()
    await computer.interface.right_click(300, 300)
    await computer.interface.double_click(400, 400)

    await computer.interface.type_text("Hello, World!")
    await computer.interface.press_key("enter")

    await computer.interface.set_clipboard("Test clipboard")
    content = await computer.interface.copy_to_clipboard()
    print(f"Clipboard content: {content}")
finally:
    await computer.stop()
```

## Install

To install the Computer-Use Interface (CUI):

```bash
pip install "cua-computer[all]"
```

The `cua-computer` PyPi package pulls automatically the latest executable version of Lume through [pylume](https://github.com/trycua/pylume).

## Run

Refer to this notebook for a step-by-step guide on how to use the Computer-Use Interface (CUI):

- [Computer-Use Interface (CUI)](https://github.com/trycua/cua/blob/main/notebooks/computer_nb.ipynb)

## Docs

- [Computers](https://cua.ai/docs/computer-sdk/computers)
- [Commands](https://cua.ai/docs/computer-sdk/commands)
- [Computer UI](https://cua.ai/docs/computer-sdk/computer-ui)
- [Sandboxed Python](https://cua.ai/docs/computer-sdk/sandboxed-python)
