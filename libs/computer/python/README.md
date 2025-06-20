<div align="center">
<h1>
  <div class="image-wrapper" style="display: inline-block;">
    <picture>
      <source media="(prefers-color-scheme: dark)" alt="logo" height="150" srcset="../../img/logo_white.png" style="display: block; margin: auto;">
      <source media="(prefers-color-scheme: light)" alt="logo" height="150" srcset="../../img/logo_black.png" style="display: block; margin: auto;">
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
    <img src="../../img/computer.png"/>
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

    await computer.interface.type("Hello, World!")
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

- [Computer-Use Interface (CUI)](../../notebooks/computer_nb.ipynb)

## Using the Gradio Computer UI

The computer module includes a Gradio UI for creating and sharing demonstration data. We make it easy for people to build community datasets for better computer use models with an upload to Huggingface feature.

```bash
# Install with UI support
pip install "cua-computer[ui]"
```

> **Note:** For precise control of the computer, we recommend using VNC or Screen Sharing instead of the Computer Gradio UI.

### Building and Sharing Demonstrations with Huggingface

Follow these steps to contribute your own demonstrations:

#### 1. Set up Huggingface Access

Set your HF_TOKEN in a .env file or in your environment variables:

```bash
# In .env file
HF_TOKEN=your_huggingface_token
```

#### 2. Launch the Computer UI

```python
# launch_ui.py
from computer.ui.gradio.app import create_gradio_ui
from dotenv import load_dotenv
load_dotenv('.env')

app = create_gradio_ui()
app.launch(share=False)
```

For examples, see [Computer UI Examples](../../examples/computer_ui_examples.py)

#### 3. Record Your Tasks

<details open>
<summary>View demonstration video</summary>
<video src="https://github.com/user-attachments/assets/de3c3477-62fe-413c-998d-4063e48de176" controls width="600"></video>
</details>

Record yourself performing various computer tasks using the UI.

#### 4. Save Your Demonstrations

<details open>
<summary>View demonstration video</summary>
<video src="https://github.com/user-attachments/assets/5ad1df37-026a-457f-8b49-922ae805faef" controls width="600"></video>
</details>

Save each task by picking a descriptive name and adding relevant tags (e.g., "office", "web-browsing", "coding").

#### 5. Record Additional Demonstrations

Repeat steps 3 and 4 until you have a good amount of demonstrations covering different tasks and scenarios.

#### 6. Upload to Huggingface

<details open>
<summary>View demonstration video</summary>
<video src="https://github.com/user-attachments/assets/c586d460-3877-4b5f-a736-3248886d2134" controls width="600"></video>
</details>

Upload your dataset to Huggingface by:
- Naming it as `{your_username}/{dataset_name}`
- Choosing public or private visibility
- Optionally selecting specific tags to upload only tasks with certain tags

#### Examples and Resources

- Example Dataset: [ddupont/test-dataset](https://huggingface.co/datasets/ddupont/test-dataset)
- Find Community Datasets: 🔍 [Browse CUA Datasets on Huggingface](https://huggingface.co/datasets?other=cua)

