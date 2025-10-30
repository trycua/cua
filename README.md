<div align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" alt="Cua logo" height="150" srcset="img/logo_white.png">
    <source media="(prefers-color-scheme: light)" alt="Cua logo" height="150" srcset="img/logo_black.png">
    <img alt="Cua logo" height="150" src="img/logo_black.png">
  </picture>

[![Python](https://img.shields.io/badge/Python-333333?logo=python&logoColor=white&labelColor=333333)](#)
[![Swift](https://img.shields.io/badge/Swift-F05138?logo=swift&logoColor=white)](#)
[![macOS](https://img.shields.io/badge/macOS-000000?logo=apple&logoColor=F0F0F0)](#)
[![Discord](https://img.shields.io/badge/Discord-%235865F2.svg?&logo=discord&logoColor=white)](https://discord.com/invite/mVnXXpdE85)
<br>
<a href="https://trendshift.io/repositories/13685" target="_blank"><img src="https://trendshift.io/api/badge/repositories/13685" alt="trycua%2Fcua | Trendshift" style="width: 250px; height: 55px;" width="250" height="55"/></a>

</div>

**Cua** ("koo-ah") is Docker for [Computer-Use Agents](https://www.oneusefulthing.org/p/when-you-give-a-claude-a-mouse) - it enables AI agents to control full operating systems in virtual containers and deploy them locally or to the cloud.

<div align="center">
  <video src="https://github.com/user-attachments/assets/c619b4ea-bb8e-4382-860e-f3757e36af20" width="600" controls></video>
</div>

With the [Computer SDK](#computer-sdk), you can:

- automate Windows, Linux, and macOS VMs with a consistent, [pyautogui-like API](https://docs.trycua.com/docs/libraries/computer#interface-actions)
- create & manage VMs [locally](https://docs.trycua.com/docs/computer-sdk/computers#cua-local-containers) or using [Cua cloud](https://www.trycua.com/)

With the [Agent SDK](#agent-sdk), you can:

- run computer-use models with a [consistent schema](https://docs.trycua.com/docs/agent-sdk/message-format)
- benchmark on OSWorld-Verified, SheetBench-V2, and more [with a single line of code using HUD](https://docs.trycua.com/docs/agent-sdk/integrations/hud) ([Notebook](https://github.com/trycua/cua/blob/main/notebooks/eval_osworld.ipynb))
- combine UI grounding models with any LLM using [composed agents](https://docs.trycua.com/docs/agent-sdk/supported-agents/composed-agents)
- use new UI agent models and UI grounding models from the Model Zoo below with just a model string (e.g., `ComputerAgent(model="openai/computer-use-preview")`)
- use API or local inference by changing a prefix (e.g., `openai/`, `openrouter/`, `ollama/`, `huggingface-local/`, `mlx/`, [etc.](https://docs.litellm.ai/docs/providers))

# Modules

<table>
<tr>
<td width="25%" align="center" valign="top">

[**Agent**](#agent-sdk)<br />
AI agent framework for automating tasks

</td>
<td width="25%" align="center" valign="top">

**[Computer](#computer-sdk)**<br />
TypeScript/Python SDK for controlling Cua environments

</td>
<td width="25%" align="center" valign="top">

**[MCP Server](#mcp-server)**<br />
MCP server for using Cua agents and computers

</td>
<td width="25%" align="center" valign="top">

**[Computer Server](#computer-server)**<br />
Server component that runs on Cua environments

</td>
</tr>
</table>

<table>
<tr>
<td width="25%" align="center" valign="top">

**[Lume](#lume)**<br />
VM management for macOS

</td>
<td width="25%" align="center" valign="top">

**[Lumier](#lumier)**<br />
Docker interface for macOS/Linux VMs

</td>
<td width="25%" align="center" valign="top">

**[SOM](#som)**<br />
Set-of-Mark library for Agent

</td>
<td width="25%" align="center" valign="top">

**[Core](#core)**<br />
Core utilities for Cua

</td>
</tr>
</table>

# Quick Start

- [Clone a starter template and run the code in <1 min](https://github.com/trycua/agent-template)
- [Get started with the Cua SDKs](https://docs.trycua.com/docs/quickstart-devs)
- [Get started with the Cua CLI](https://docs.trycua.com/docs/quickstart-cli)

# Agent SDK

Install the agent SDK:

```bash
pip install cua-agent[all]
```

Initialize a computer agent using a [model configuration string](#model-configuration) and a [computer instance](#computer-usage):

```python
from agent import ComputerAgent

# ComputerAgent works with any computer initialized with the Computer SDK

agent = ComputerAgent(
    model="anthropic/claude-3-5-sonnet-20241022",
    tools=[computer],
    max_trajectory_budget=5.0
)

messages = [{"role": "user", "content": "Take a screenshot and tell me what you see"}]

async for result in agent.run(messages):
    for item in result["output"]:
        if item["type"] == "message":
            print(item["content"][0]["text"])
```

## Output format

Cua uses the OpenAI Agent response format.

<details>
<summary>Example</summary>

```json
{
  "output": [
    {
      "role": "user",
      "content": "go to trycua on gh"
    },
    {
      "summary": [
        {
          "text": "Searching Firefox for Trycua GitHub",
          "type": "summary_text"
        }
      ],
      "type": "reasoning"
    },
    {
      "action": {
        "text": "Trycua GitHub",
        "type": "type"
      },
      "call_id": "call_QI6OsYkXxl6Ww1KvyJc4LKKq",
      "status": "completed",
      "type": "computer_call"
    },
    {
      "type": "computer_call_output",
      "call_id": "call_QI6OsYkXxl6Ww1KvyJc4LKKq",
      "output": {
        "type": "input_image",
        "image_url": "data:image/png;base64,..."
      }
    },
    {
      "type": "message",
      "role": "assistant",
      "content": [
        {
          "text": "Success! The Trycua GitHub page has been opened.",
          "type": "output_text"
        }
      ]
    }
  ],
  "usage": {
    "prompt_tokens": 150,
    "completion_tokens": 75,
    "total_tokens": 225,
    "response_cost": 0.01
  }
}
```

</details>

## Model Configuration

These are the valid model configurations for `ComputerAgent(model="...")`:

| Configuration                            | Description                                                                                                                                         |
| ---------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| `{computer-use-model}`                   | A single model to perform all computer-use tasks                                                                                                    |
| `{grounding-model}+{any-vlm-with-tools}` | [Composed](https://docs.trycua.com/docs/agent-sdk/supported-agents/composed-agents) with VLM for captioning and grounding LLM for element detection |
| `moondream3+{any-llm-with-tools}`        | [Composed](https://docs.trycua.com/docs/agent-sdk/supported-agents/composed-agents) with Moondream3 for captioning and UI element detection         |
| `human/human`                            | A [human-in-the-loop](https://docs.trycua.com/docs/agent-sdk/supported-agents/human-in-the-loop) in place of a model                                |

### Model Capabilities

The following table shows which capabilities are supported by each model:

| Model                                                                                                                            | Computer-Use | Grounding | Tools | VLM |
| -------------------------------------------------------------------------------------------------------------------------------- | :----------: | :-------: | :---: | :-: |
| [Claude Sonnet/Haiku](https://docs.claude.com/en/docs/agents-and-tools/tool-use/computer-use-tool#how-to-implement-computer-use) |      🖥️      |    🎯     |  🛠️   | 👁️  |
| [OpenAI CU Preview](https://platform.openai.com/docs/models/computer-use-preview)                                                |      🖥️      |    🎯     |       | 👁️  |
| [GLM-V](https://huggingface.co/THUDM/glm-4v-9b)                                                                                  |      🖥️      |    🎯     |  🛠️   | 👁️  |
| [Gemini CU Preview](https://ai.google.dev/gemini-api/docs/computer-use)                                                          |      🖥️      |    🎯     |       | 👁️  |
| [InternVL](https://huggingface.co/OpenGVLab/InternVL3_5-1B)                                                                      |      🖥️      |    🎯     |  🛠️   | 👁️  |
| [UI-TARS](https://huggingface.co/ByteDance-Seed/UI-TARS-1.5-7B)                                                                  |      🖥️      |    🎯     |  🛠️   | 👁️  |
| [OpenCUA](https://huggingface.co/xlangai/OpenCUA-7B)                                                                             |              |    🎯     |       |     |
| [GTA](https://huggingface.co/HelloKKMe/GTA1-7B)                                                                                  |              |    🎯     |       |     |
| [Holo](https://huggingface.co/Hcompany/Holo1.5-3B)                                                                               |              |    🎯     |       |     |
| [Moondream](https://huggingface.co/moondream/moondream3-preview)                                                                 |              |    🎯     |       |     |
| [OmniParser](https://github.com/microsoft/OmniParser)                                                                            |              |    🎯     |       |     |

### Model IDs

<details>
<summary>Examples of valid model IDs</summary>

| Model                                                                                                                            | Model IDs                                                        |
| -------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------- |
| [Claude Sonnet/Haiku](https://docs.claude.com/en/docs/agents-and-tools/tool-use/computer-use-tool#how-to-implement-computer-use) | `anthropic/claude-sonnet-4-5`, `anthropic/claude-haiku-4-5`      |
| [OpenAI CU Preview](https://platform.openai.com/docs/models/computer-use-preview)                                                | `openai/computer-use-preview`                                    |
| [GLM-V](https://huggingface.co/THUDM/glm-4v-9b)                                                                                  | `openrouter/z-ai/glm-4.5v`, `huggingface-local/zai-org/GLM-4.5V` |
| [Gemini CU Preview](https://ai.google.dev/gemini-api/docs/computer-use)                                                          | `gemini-2.5-computer-use-preview`                                |
| [InternVL](https://huggingface.co/OpenGVLab/InternVL3_5-1B)                                                                      | `huggingface-local/OpenGVLab/InternVL3_5-{1B,2B,4B,8B,...}`      |
| [UI-TARS](https://huggingface.co/ByteDance-Seed/UI-TARS-1.5-7B)                                                                  | `huggingface-local/ByteDance-Seed/UI-TARS-1.5-7B`                |
| [OpenCUA](https://huggingface.co/xlangai/OpenCUA-7B)                                                                             | `huggingface-local/xlangai/OpenCUA-{7B,32B}`                     |
| [GTA](https://huggingface.co/HelloKKMe/GTA1-7B)                                                                                  | `huggingface-local/HelloKKMe/GTA1-{7B,32B,72B}`                  |
| [Holo](https://huggingface.co/Hcompany/Holo1.5-3B)                                                                               | `huggingface-local/Hcompany/Holo1.5-{3B,7B,72B}`                 |
| [Moondream](https://huggingface.co/moondream/moondream3-preview)                                                                 | `moondream3`                                                     |
| [OmniParser](https://github.com/microsoft/OmniParser)                                                                            | `omniparser`                                                     |

</details>

Missing a model? Create a [feature request](https://github.com/trycua/cua/issues/new?assignees=&labels=enhancement&projects=&title=%5BAgent%5D%3A+Add+model+support+for+) or [contribute](https://github.com/trycua/cua/blob/main/CONTRIBUTING.md)!

Learn more in the [Agent SDK documentation](./libs/python/agent/README.md).

# Computer SDK

Install the computer SDK:

```bash
pip install cua-computer
```

Initialize a computer:

```python
from computer import Computer

computer = Computer(
    os_type="linux",  # or "macos", "windows"
    provider_type="cloud",  # or "lume", "docker", "windows_sandbox"
    name="your-sandbox-name",
    api_key="your-api-key"  # only for cloud
    # or use_host_computer_server=True for host desktop
)

try:
    await computer.run()

    # Take a screenshot
    screenshot = await computer.interface.screenshot()

    # Click and type
    await computer.interface.left_click(100, 100)
    await computer.interface.type("Hello!")
finally:
    await computer.close()
```

Learn more in the [Computer SDK documentation](./libs/python/computer/README.md).

# MCP Server

Install the MCP server:

```bash
pip install cua-mcp-server
```

Learn more in the [MCP Server documentation](./libs/python/mcp-server/README.md).

# Computer Server

Install the Computer Server:

```bash
pip install cua-computer-server
python -m computer_server
```

Learn more in the [Computer Server documentation](./libs/python/computer-server/README.md).

# Lume

Install Lume:

```bash
curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/lume/scripts/install.sh | bash
```

Learn more in the [Lume documentation](./libs/lume/README.md).

# Lumier

Install Lumier:

```bash
docker pull trycua/lumier:latest
```

Learn more in the [Lumier documentation](./libs/lumier/README.md).

# SOM

Install SOM:

```bash
pip install cua-som
```

Learn more in the [SOM documentation](./libs/python/som/README.md).

# Resources

- [Cua Blog](https://www.trycua.com/blog)
- [Cua Docs](https://docs.trycua.com)

# Community and Contributions

We welcome contributions to Cua! Please refer to our [Contributing Guidelines](CONTRIBUTING.md) for details.

Join our [Discord community](https://discord.com/invite/mVnXXpdE85) to discuss ideas, get assistance, or share your demos!

# License

Cua is open-sourced under the MIT License - see the [LICENSE](LICENSE.md) file for details.

Portions of this project, specifically components adapted from Kasm Technologies Inc., are also licensed under the MIT License. See [libs/kasm/LICENSE](libs/kasm/LICENSE) for details.

Microsoft's OmniParser, which is used in this project, is licensed under the Creative Commons Attribution 4.0 International License (CC-BY-4.0). See the [OmniParser LICENSE](https://github.com/microsoft/OmniParser/blob/master/LICENSE) for details.

## Third-Party Licenses and Optional Components

Some optional extras for this project depend on third-party packages that are licensed under terms different from the MIT License.

- The optional "omni" extra (installed via `pip install "cua-agent[omni]"`) installs the `cua-som` module, which includes `ultralytics` and is licensed under the AGPL-3.0.

When you choose to install and use such optional extras, your use, modification, and distribution of those third-party components are governed by their respective licenses (e.g., AGPL-3.0 for `ultralytics`).

## Trademarks

Apple, macOS, and Apple Silicon are trademarks of Apple Inc.  
Ubuntu and Canonical are registered trademarks of Canonical Ltd.  
Microsoft is a registered trademark of Microsoft Corporation.

This project is not affiliated with, endorsed by, or sponsored by Apple Inc., Canonical Ltd., Microsoft Corporation, or Kasm Technologies.

# Stargazers

Thank you to all our supporters!

[![Stargazers over time](https://starchart.cc/trycua/cua.svg?variant=adaptive)](https://starchart.cc/trycua/cua)

# Sponsors

Thank you to all our [GitHub Sponsors](https://github.com/sponsors/trycua)!

<img width="300" alt="coderabbit-cli" src="https://github.com/user-attachments/assets/23a98e38-7897-4043-8ef7-eb990520dccc" />
