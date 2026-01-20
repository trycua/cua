# Build Your Own Operator on macOS - Part 2

_Published on April 27, 2025 by Francesco Bonacci_

In our [previous post](build-your-own-operator-on-macos-1.md), we built a basic Computer-Use Operator from scratch using OpenAI's `computer-use-preview` model and our [cua-computer](https://pypi.org/project/cua-computer) package. While educational, implementing the control loop manually can be tedious and error-prone.

In this follow-up, we'll explore our [cua-agent](https://pypi.org/project/cua-agent) framework - a high-level abstraction that handles all the complexity of VM interaction, screenshot processing, model communication, and action execution automatically.

<div align="center">
  <video src="https://github.com/user-attachments/assets/0be7e3e3-eead-4646-a4a3-5bb392501ee7" width="600" controls></video>
</div>

## What You'll Learn

By the end of this tutorial, you'll be able to:

- Set up the `cua-agent` framework with various agent loop types and model providers
- Understand the different agent loop types and their capabilities
- Work with local models for cost-effective workflows
- Use a simple UI for your operator

**Prerequisites:**

- Completed setup from Part 1 ([lume CLI installed](https://github.com/trycua/cua?tab=readme-ov-file#option-2-full-computer-use-agent-capabilities), macOS Cua image already pulled)
- Python 3.10+. We recommend using Conda (or Anaconda) to create an ad hoc Python environment.
- API keys for OpenAI and/or Anthropic (optional for local models)

**Estimated Time:** 30-45 minutes

## Introduction to cua-agent

The `cua-agent` framework is designed to simplify building Computer-Use Agents. It abstracts away the complex interaction loop we built manually in Part 1, letting you focus on defining tasks rather than implementing the machinery. Among other features, it includes:

- **Multiple Provider Support**: Works with OpenAI, Anthropic, UI-Tars, local models (via Ollama), or any OpenAI-compatible model (e.g. LM Studio, vLLM, LocalAI, OpenRouter, Groq, etc.)
- **Flexible Loop Types**: Different implementations optimized for various models (e.g. OpenAI vs. Anthropic)
- **Structured Responses**: Clean, consistent output following the OpenAI Agent SDK specification we touched on in Part 1
- **Local Model Support**: Run cost-effectively with locally hosted models (Ollama, LM Studio, vLLM, LocalAI, etc.)
- **Gradio UI**: Optional visual interface for interacting with your agent

## Installation

Let's start by installing the `cua-agent` package. You can install it with all features or selectively install only what you need.

From your python 3.10+ environment, run:

```bash
# For all features
pip install "cua-agent[all]"

# Or selectively install only what you need
pip install "cua-agent[openai]"    # OpenAI support
pip install "cua-agent[anthropic]"  # Anthropic support
pip install "cua-agent[uitars]"    # UI-Tars support
pip install "cua-agent[omni]"       # OmniParser + VLMs support
pip install "cua-agent[ui]"         # Gradio UI
```

## Setting Up Your Environment

Before running any code examples, let's set up a proper environment:

1. **Create a new directory** for your project:

   ```bash
   mkdir cua-agent-tutorial
   cd cua-agent-tutorial
   ```

2. **Set up a Python environment** using one of these methods:

   **Option A: Using conda command line**

   ```bash
   # Using conda
   conda create -n cua-agent python=3.10
   conda activate cua-agent
   ```

   **Option B: Using Anaconda Navigator UI**
   - Open Anaconda Navigator
   - Click on "Environments" in the left sidebar
   - Click the "Create" button at the bottom
   - Name your environment "cua-agent"
   - Select Python 3.10
   - Click "Create"
   - Once created, select the environment and click "Open Terminal" to activate it

   **Option C: Using venv**

   ```bash
   python -m venv cua-env
   source cua-env/bin/activate  # On macOS/Linux
   ```

3. **Install the cua-agent package**:

   ```bash
   pip install "cua-agent[all]"
   ```

4. **Set up your API keys as environment variables**:

   ```bash
   # For OpenAI models
   export OPENAI_API_KEY=your_openai_key_here

   # For Anthropic models (if needed)
   export ANTHROPIC_API_KEY=your_anthropic_key_here
   ```

5. **Create a Python file or notebook**:

   **Option A: Create a Python script**

   ```bash
   # For a Python script
   touch cua_agent_example.py
   ```

   **Option B: Use VS Code notebooks**
   - Open VS Code
   - Install the Python extension if you haven't already
   - Create a new file with a `.ipynb` extension (e.g., `cua_agent_tutorial.ipynb`)
   - Select your Python environment when prompted
   - You can now create and run code cells in the notebook interface

Now you're ready to run the code examples!

## Understanding Agent Loops

If you recall from Part 1, we had to implement a custom interaction loop to interact with the compute-use-preview model.

In the `cua-agent` framework, an **Agent Loop** is the core abstraction that implements the continuous interaction cycle between an AI model and the computer environment. It manages the flow of:

1. Capturing screenshots of the computer's state
2. Processing these screenshots (with or without UI element detection)
3. Sending this visual context to an AI model along with the task instructions
4. Receiving the model's decisions on what actions to take
5. Safely executing these actions in the environment
6. Repeating this cycle until the task is complete

The loop handles all the complex error handling, retries, context management, and model-specific interaction patterns so you don't have to implement them yourself.

While the core concept remains the same across all agent loops, different AI models require specialized handling for optimal performance. To address this, the framework provides 4 different agent loop implementations, each designed for different computer-use modalities.
| Agent Loop | Supported Models | Description | Set-Of-Marks |
|:-----------|:-----------------|:------------|:-------------|
| `AgentLoop.OPENAI` | • `computer_use_preview` | Use OpenAI Operator Cua Preview model | Not Required |
| `AgentLoop.ANTHROPIC` | • `claude-sonnet-4-5-20250929`<br>• `claude-3-7-sonnet-20250219` | Use Anthropic Computer-Use Beta Tools | Not Required |
| `AgentLoop.UITARS` | • `ByteDance-Seed/UI-TARS-1.5-7B` | Uses ByteDance's UI-TARS 1.5 model | Not Required |
| `AgentLoop.OMNI` | • `claude-sonnet-4-5-20250929`<br>• `claude-3-7-sonnet-20250219`<br>• `gpt-4.5-preview`<br>• `gpt-4o`<br>• `gpt-4`<br>• `phi4`<br>• `phi4-mini`<br>• `gemma3`<br>• `...`<br>• `Any Ollama or OpenAI-compatible model` | Use OmniParser for element pixel-detection (SoM) and any VLMs for UI Grounding and Reasoning | OmniParser |

Each loop handles the same basic pattern we implemented manually in Part 1:

1. Take a screenshot of the VM
2. Send the screenshot and task to the AI model
3. Receive an action to perform
4. Execute the action
5. Repeat until the task is complete

### Why Different Agent Loops?

The `cua-agent` framework provides multiple agent loop implementations to abstract away the complexity of interacting with different Cua models. Each provider has unique API structures, response formats, conventions and capabilities that require specialized handling:

- **OpenAI Loop**: Uses the Responses API with a specific `computer_call_output` format for sending screenshots after actions. Requires handling safety checks and maintains a chain of requests using `previous_response_id`.

- **Anthropic Loop**: Implements a [multi-agent loop pattern](https://platform.claude.com/docs/en/agents-and-tools/tool-use/computer-use-tool#understanding-the-multi-agent-loop) with a sophisticated message handling system, supporting various API providers (Anthropic, Bedrock, Vertex) with token management and prompt caching capabilities.

- **UI-TARS Loop**: Requires custom message formatting and specialized parsing to extract actions from text responses using a "box token" system for UI element identification.

- **OMNI Loop**: Uses [Microsoft's OmniParser](https://github.com/microsoft/OmniParser) to create a [Set-of-Marks (SoM)](https://arxiv.org/abs/2310.11441) representation of the UI, enabling any vision-language model to interact with interfaces without specialized UI training.

- **AgentLoop.OMNI**: The most flexible option that works with virtually any vision-language model including local and open-source ones. Perfect for cost-effective development or when you need to use models without native computer-use capabilities.

These abstractions allow you to easily switch between providers without changing your application code. All loop implementations are available in the [cua-agent GitHub repository](https://github.com/trycua/cua/tree/main/libs/python/agent).

Choosing the right agent loop depends not only on your API access and technical requirements but also on the specific tasks you need to accomplish. To make an informed decision, it's helpful to understand how these underlying models perform across different computing environments – from desktop operating systems to web browsers and mobile interfaces.

## Computer-Use Model Capabilities

The performance of different Computer-Use models varies significantly across tasks. These benchmark evaluations measure an agent's ability to follow instructions and complete real-world tasks in different computing environments.

| Benchmark type   | Benchmark                                                          | UI-TARS-1.5 | OpenAI Cua | Claude 3.7 | Previous SOTA   | Human |
| ---------------- | ------------------------------------------------------------------ | ----------- | ---------- | ---------- | --------------- | ----- |
| **Computer Use** | [OSworld](https://arxiv.org/abs/2404.07972) (100 steps)            | **42.5**    | 36.4       | 28         | 38.1 (200 step) | 72.4  |
|                  | [Windows Agent Arena](https://arxiv.org/abs/2409.08264) (50 steps) | **42.1**    | -          | -          | 29.8            | -     |
| **Browser Use**  | [WebVoyager](https://arxiv.org/abs/2401.13919)                     | 84.8        | **87**     | 84.1       | 87              | -     |
|                  | [Online-Mind2web](https://arxiv.org/abs/2504.01382)                | **75.8**    | 71         | 62.9       | 71              | -     |
| **Phone Use**    | [Android World](https://arxiv.org/abs/2405.14573)                  | **64.2**    | -          | -          | 59.5            | -     |

### When to Use Each Loop

- **AgentLoop.OPENAI**: Choose when you have OpenAI Tier 3 access and need the most capable computer-use agent for web-based tasks. Uses the same [OpenAI Computer-Use Loop](https://platform.openai.com/docs/guides/tools-computer-use) as Part 1, delivering strong performance on browser-based benchmarks.

- **AgentLoop.ANTHROPIC**: Ideal for users with Anthropic API access who need strong reasoning capabilities with computer-use abilities. Works with `claude-sonnet-4-5-20250929` and `claude-3-7-sonnet-20250219` models following [Anthropic's Computer-Use tools](https://platform.claude.com/docs/en/agents-and-tools/tool-use/computer-use-tool#understanding-the-multi-agent-loop).

- **AgentLoop.UITARS**: Best for scenarios requiring more powerful OS/desktop, and latency-sensitive automation, as UI-TARS-1.5 leads in OS capabilities benchmarks. Requires running the model locally or accessing it through compatible endpoints (e.g. on Hugging Face).

- **AgentLoop.OMNI**: The most flexible option that works with virtually any vision-language model including local and open-source ones. Perfect for cost-effective development or when you need to use models without native computer-use capabilities.

Now that we understand the capabilities and strengths of different models, let's see how easy it is to implement a Computer-Use Agent using the `cua-agent` framework. Let's look at the implementation details.

## Creating Your First Computer-Use Agent

With the `cua-agent` framework, creating a Computer-Use Agent becomes remarkably straightforward. The framework handles all the complexities of model interaction, screenshot processing, and action execution behind the scenes. Let's look at a simple example of how to build your first agent:

**How to run this example:**

1. Create a new file named `simple_task.py` in your text editor or IDE (like VS Code, PyCharm, or Cursor)
2. Copy and paste the following code:

```python
import asyncio
from computer import Computer
from agent import ComputerAgent

async def run_simple_task():
    async with Computer() as macos_computer:
        # Create agent with OpenAI loop
        agent = ComputerAgent(
            model="openai/computer-use-preview",
            tools=[macos_computer]
        )

        # Define a simple task
        task = "Open Safari and search for 'Python tutorials'"

        # Run the task and process responses
        async for result in agent.run(task):
            print(f"Action: {result.get('text')}")

# Run the example
if __name__ == "__main__":
    asyncio.run(run_simple_task())
```

3. Save the file
4. Open a terminal, navigate to your project directory, and run:

   ```bash
   python simple_task.py
   ```

5. The code will initialize the macOS virtual machine, create an agent, and execute the task of opening Safari and searching for Python tutorials.

You can also run this in a VS Code notebook:

1. Create a new notebook in VS Code (.ipynb file)
2. Copy the code into a cell (without the `if __name__ == "__main__":` part)
3. Run the cell to execute the code

You can find the full code in our [notebook](https://github.com/trycua/cua/blob/main/notebooks/blog/build-your-own-operator-on-macos-2.ipynb).

Compare this to the manual implementation from Part 1 - we've reduced dozens of lines of code to just a few. The cua-agent framework handles all the complex logic internally, letting you focus on the overarching agentic system.

## Working with Multiple Tasks

Another advantage of the cua-agent framework is easily chaining multiple tasks. Instead of managing complex state between tasks, you can simply provide a sequence of instructions to be executed in order:

**How to run this example:**

1. Create a new file named `multi_task.py` with the following code:

```python
import asyncio
from computer import Computer
from agent import ComputerAgent

async def run_multi_task_workflow():
    async with Computer() as macos_computer:
        agent = ComputerAgent(
            model="anthropic/claude-sonnet-4-5-20250929",
            tools=[macos_computer]
        )

        tasks = [
            "Open Safari and go to github.com",
            "Search for 'trycua/cua'",
            "Open the repository page",
            "Click on the 'Issues' tab",
            "Read the first open issue"
        ]

        for i, task in enumerate(tasks):
            print(f"\nTask {i+1}/{len(tasks)}: {task}")
            async for result in agent.run(task):
                # Print just the action description for brevity
                if result.get("text"):
                    print(f"  → {result.get('text')}")
            print(f"✅ Task {i+1} completed")

if __name__ == "__main__":
    asyncio.run(run_multi_task_workflow())
```

2. Save the file
3. Make sure you have set your Anthropic API key:
   ```bash
   export ANTHROPIC_API_KEY=your_anthropic_key_here
   ```
4. Run the script:
   ```bash
   python multi_task.py
   ```

This pattern is particularly useful for creating workflows that navigate through multiple steps of an application or process. The agent maintains visual context between tasks, making it more likely to successfully complete complex sequences of actions.

## Understanding the Response Format

Each action taken by the agent returns a structured response following the OpenAI Agent SDK specification. This standardized format makes it easy to extract detailed information about what the agent is doing and why:

```python
async for result in agent.run(task):
    # Basic information
    print(f"Response ID: {result.get('id')}")
    print(f"Response Text: {result.get('text')}")

    # Detailed token usage statistics
    usage = result.get('usage')
    if usage:
        print(f"Input Tokens: {usage.get('input_tokens')}")
        print(f"Output Tokens: {usage.get('output_tokens')}")

    # Reasoning and actions
    for output in result.get('output', []):
        if output.get('type') == 'reasoning':
            print(f"Reasoning: {output.get('summary', [{}])[0].get('text')}")
        elif output.get('type') == 'computer_call':
            action = output.get('action', {})
            print(f"Action: {action.get('type')} at ({action.get('x')}, {action.get('y')})")
```

This structured format allows you to:

- Log detailed information about agent actions
- Provide real-time feedback to users
- Track token usage for cost monitoring
- Access the reasoning behind decisions for debugging or user explanation

## Using Local Models with OMNI

One of the most powerful features of the framework is the ability to use local models via the OMNI loop. This approach dramatically reduces costs while maintaining acceptable reliability for many agentic workflows:

**How to run this example:**

1. First, you'll need to install Ollama for running local models:
   - Visit [ollama.com](https://ollama.com) and download the installer for your OS
   - Follow the installation instructions
   - Pull the Gemma 3 model:
     ```bash
     ollama pull gemma3:4b-it-q4_K_M
     ```

2. Create a file named `local_model.py` with this code:

```python
import asyncio
from computer import Computer
from agent import ComputerAgent

async def run_with_local_model():
    async with Computer() as macos_computer:
        agent = ComputerAgent(
            model="omniparser+ollama_chat/gemma3",
            tools=[macos_computer]
        )

        task = "Open the Calculator app and perform a simple calculation"

        async for result in agent.run(task):
            print(f"Action: {result.get('text')}")

if __name__ == "__main__":
    asyncio.run(run_with_local_model())
```

3. Run the script:
   ```bash
   python local_model.py
   ```

You can also use other local model servers with the OAICOMPAT provider, which enables compatibility with any API endpoint following the OpenAI API structure:

```python
agent = ComputerAgent(
    model=LLM(
        provider=LLMProvider.OAICOMPAT,
        name="gemma-3-12b-it",
        provider_base_url="http://localhost:1234/v1"  # LM Studio endpoint
    ),
    tools=[macos_computer]
)
```

Common local endpoints include:

- LM Studio: `http://localhost:1234/v1`
- vLLM: `http://localhost:8000/v1`
- LocalAI: `http://localhost:8080/v1`
- Ollama with OpenAI compat: `http://localhost:11434/v1`

This approach is perfect for:

- Development and testing without incurring API costs
- Offline or air-gapped environments where API access isn't possible
- Privacy-sensitive applications where data can't leave your network
- Experimenting with different models to find the best fit for your use case

## Deploying and Using UI-TARS

UI-TARS is ByteDance's Computer-Use model designed for navigating OS-level interfaces. It shows excellent performance on desktop OS tasks. To use UI-TARS, you'll first need to deploy the model.

### Deployment Options

1. **Local Deployment**: Follow the [UI-TARS deployment guide](https://github.com/bytedance/UI-TARS/blob/main/README_deploy.md) to run the model locally.

2. **Hugging Face Endpoint**: Deploy UI-TARS on Hugging Face Inference Endpoints, which will give you a URL like:
   `https://**************.us-east-1.aws.endpoints.huggingface.cloud/v1`

3. **Using with cua-agent**: Once deployed, you can use UI-TARS with the cua-agent framework:

```python
agent = ComputerAgent(
    model=LLM(
        provider=LLMProvider.OAICOMPAT,
        name="tgi",
        provider_base_url="https://**************.us-east-1.aws.endpoints.huggingface.cloud/v1"
    ),
    tools=[macos_computer]
)
```

UI-TARS is particularly useful for desktop automation tasks, as it shows the highest performance on OS-level benchmarks like OSworld and Windows Agent Arena.

## Understanding Agent Responses in Detail

The `run()` method of your agent yields structured responses that follow the OpenAI Agent SDK specification. This provides a rich set of information beyond just the basic action text:

```python
async for result in agent.run(task):
    # Basic ID and text
    print("Response ID:", result.get("id"))
    print("Response Text:", result.get("text"))

    # Token usage statistics
    usage = result.get("usage")
    if usage:
        print("\nUsage Details:")
        print(f"  Input Tokens: {usage.get('input_tokens')}")
        if "input_tokens_details" in usage:
            print(f"  Input Tokens Details: {usage.get('input_tokens_details')}")
        print(f"  Output Tokens: {usage.get('output_tokens')}")
        if "output_tokens_details" in usage:
            print(f"  Output Tokens Details: {usage.get('output_tokens_details')}")
        print(f"  Total Tokens: {usage.get('total_tokens')}")

    # Detailed reasoning and actions
    outputs = result.get("output", [])
    for output in outputs:
        output_type = output.get("type")
        if output_type == "reasoning":
            print("\nReasoning:")
            for summary in output.get("summary", []):
                print(f"  {summary.get('text')}")
        elif output_type == "computer_call":
            action = output.get("action", {})
            print("\nComputer Action:")
            print(f"  Type: {action.get('type')}")
            print(f"  Position: ({action.get('x')}, {action.get('y')})")
            if action.get("text"):
                print(f"  Text: {action.get('text')}")
```

This detailed information is invaluable for debugging, logging, and understanding the agent's decision-making process in an agentic system. More details can be found in the [OpenAI Agent SDK Specification](https://platform.openai.com/docs/guides/responses-vs-chat-completions).

## Building a Gradio UI

For a visual interface to your agent, the package also includes a Gradio UI:

**How to run the Gradio UI:**

1. Create a file named `launch_ui.py` with the following code:

```python
from agent.ui.gradio.app import create_gradio_ui

# Create and launch the UI
if __name__ == "__main__":
    app = create_gradio_ui()
    app.launch(share=False)  # Set share=False for local access only
```

2. Install the UI dependencies if you haven't already:

   ```bash
   pip install "cua-agent[ui]"
   ```

3. Run the script:

   ```bash
   python launch_ui.py
   ```

4. Open your browser to the displayed URL (usually http://127.0.0.1:7860)

**Creating a Shareable Link (Optional):**

You can also create a temporary public URL to access your Gradio UI from anywhere:

```python
# In launch_ui.py
if __name__ == "__main__":
    app = create_gradio_ui()
    app.launch(share=True)  # Creates a public link
```

When you run this, Gradio will display both a local URL and a public URL like:

```
Running on local URL:  http://127.0.0.1:7860
Running on public URL: https://abcd1234.gradio.live
```

**Security Note:** Be cautious when sharing your Gradio UI publicly:

- The public URL gives anyone with the link full access to your agent
- Consider using basic authentication for additional protection:
  ```python
  app.launch(share=True, auth=("username", "password"))
  ```
- Only use this feature for personal or team use, not for production environments
- The temporary link expires when you stop the Gradio application

This provides:

- Model provider selection
- Agent loop selection
- Task input field
- Real-time display of VM screenshots
- Action history

### Setting API Keys for the UI

To use the UI with different providers, set your API keys as environment variables:

```bash
# For OpenAI models
export OPENAI_API_KEY=your_openai_key_here

# For Anthropic models
export ANTHROPIC_API_KEY=your_anthropic_key_here

# Launch with both keys set
OPENAI_API_KEY=your_key ANTHROPIC_API_KEY=your_key python launch_ui.py
```

### UI Settings Persistence

The Gradio UI automatically saves your configuration to maintain your preferences between sessions:

- Settings like Agent Loop, Model Choice, Custom Base URL, and configuration options are saved to `.gradio_settings.json` in the project's root directory
- These settings are loaded automatically when you restart the UI
- API keys entered in the custom provider field are **not** saved for security reasons
- It's recommended to add `.gradio_settings.json` to your `.gitignore` file

## Advanced Example: GitHub Repository Workflow

Let's look at a more complex example that automates a GitHub workflow:

**How to run this advanced example:**

1. Create a file named `github_workflow.py` with the following code:

```python
import asyncio
import logging
from computer import Computer
from agent import ComputerAgent

async def github_workflow():
    async with Computer(verbosity=logging.INFO) as macos_computer:
        agent = ComputerAgent(
            model="openai/computer-use-preview",
            save_trajectory=True,  # Save screenshots for debugging
            only_n_most_recent_images=3,  # Only keep last 3 images in context
            verbosity=logging.INFO,
            tools=[macos_computer]
        )

        tasks = [
            "Look for a repository named trycua/cua on GitHub.",
            "Check the open issues, open the most recent one and read it.",
            "Clone the repository in users/lume/projects if it doesn't exist yet.",
            "Open the repository with Cursor (on the dock, black background and white cube icon).",
            "From Cursor, open Composer if not already open.",
            "Focus on the Composer text area, then write and submit a task to help resolve the GitHub issue.",
        ]

        for i, task in enumerate(tasks):
            print(f"\nExecuting task {i+1}/{len(tasks)}: {task}")
            async for result in agent.run(task):
                print(f"Action: {result.get('text')}")
            print(f"✅ Task {i+1}/{len(tasks)} completed")

if __name__ == "__main__":
    asyncio.run(github_workflow())
```

2. Make sure your OpenAI API key is set:

   ```bash
   export OPENAI_API_KEY=your_openai_key_here
   ```

3. Run the script:

   ```bash
   python github_workflow.py
   ```

4. Watch as the agent completes the entire workflow:
   - The agent will navigate to GitHub
   - Find and investigate issues in the repository
   - Clone the repository to the local machine
   - Open it in Cursor
   - Use Cursor's AI features to work on a solution

This example:

1. Searches GitHub for a repository
2. Reads an issue
3. Clones the repository
4. Opens it in an IDE
5. Uses AI to write a solution

## Comparing Implementation Approaches

Let's compare our manual implementation from Part 1 with the framework approach:

### Manual Implementation (Part 1)

- Required writing custom code for the interaction loop
- Needed explicit handling of different action types
- Required direct management of the OpenAI API calls
- Around 50-100 lines of code for basic functionality
- Limited to OpenAI's computer-use model

### Framework Implementation (Part 2)

- Abstracts the interaction loop
- Handles all action types automatically
- Manages API calls internally
- Only 10-15 lines of code for the same functionality
- Works with multiple model providers
- Includes UI capabilities

## Conclusion

The `cua-agent` framework transforms what was a complex implementation task into a simple, high-level interface for building Computer-Use Agents. By abstracting away the technical details, it lets you focus on defining the tasks rather than the machinery.

### When to Use Each Approach

- **Manual Implementation (Part 1)**: When you need complete control over the interaction loop or are implementing a custom solution
- **Framework (Part 2)**: For most applications where you want to quickly build and deploy Computer-Use Agents

### Next Steps

With the basics covered, you might want to explore:

- Customizing the agent's behavior with additional parameters
- Building more complex workflows spanning multiple applications
- Integrating your agent into other applications
- Contributing to the open-source project on GitHub

### Resources

- [cua-agent GitHub repository](https://github.com/trycua/cua/tree/main/libs/python/agent)
- [Agent Notebook Examples](https://github.com/trycua/cua/blob/main/notebooks/agent_nb.ipynb)
- [OpenAI Agent SDK Specification](https://platform.openai.com/docs/api-reference/responses)
- [Anthropic API Documentation](https://platform.claude.com/docs/en/api/overview)
- [UI-TARS GitHub](https://github.com/ByteDance/UI-TARS)
- [OmniParser GitHub](https://github.com/microsoft/OmniParser)
