# App-Use: Control Individual Applications with Cua Agents

_Published on May 31, 2025 by The Cua Team_

Today, we are excited to introduce a new experimental feature landing in the [Cua GitHub repository](https://github.com/trycua/cua): **App-Use**. App-Use allows you to create lightweight virtual desktops that limit agent access to specific applications, improving precision of your agent's trajectory. Perfect for parallel workflows, and focused task execution.

> **Note:** App-Use is currently experimental. To use it, you need to enable it by passing `experiments=["app-use"]` feature flag when creating your Computer instance.

Check out an example of a Cua Agent automating Cua's team Taco Bell order through the iPhone Mirroring app:

<div align="center">
  <video src="https://github.com/user-attachments/assets/6362572e-f784-4006-aa6e-bce10991fab9" width="600" controls></video>
</div>

## What is App-Use?

App-Use lets you create virtual desktop sessions scoped to specific applications. Instead of giving an agent access to your entire screen, you can say "only work with Safari and Notes" or "just control the iPhone Mirroring app."

```python
# Create a macOS VM with App Use experimental feature enabled
computer = Computer(experiments=["app-use"])

# Create a desktop limited to specific apps
desktop = computer.create_desktop_from_apps(["Safari", "Notes"])

# Your agent can now only see and interact with these apps
agent = ComputerAgent(
    model="anthropic/claude-3-5-sonnet-20241022",
    tools=[desktop]
)
```

## Key Benefits

### 1. Lightweight and Fast

App-Use creates visual filters, not new processes. Your apps continue running normally - we just control what the agent can see and click on. The virtual desktops are composited views that require no additional compute resources beyond the existing window manager operations.

### 2. Run Multiple Agents in Parallel

Deploy a team of specialized agents, each focused on their own apps:

```python
# Create a Computer with App Use enabled
computer = Computer(experiments=["app-use"])

# Research agent focuses on browser
research_desktop = computer.create_desktop_from_apps(["Safari"])
research_agent = ComputerAgent(tools=[research_desktop], ...)

# Writing agent focuses on documents
writing_desktop = computer.create_desktop_from_apps(["Pages", "Notes"])
writing_agent = ComputerAgent(tools=[writing_desktop], ...)

async def run_agent(agent, task):
    async for result in agent.run(task):
        print(result.get('text', ''))

# Run both simultaneously
await asyncio.gather(
    run_agent(research_agent, "Research AI trends for 2025"),
    run_agent(writing_agent, "Draft blog post outline")
)
```

## How To: Getting Started with App-Use

### Requirements

To get started with App-Use, you'll need:

- Python 3.11+
- macOS Sequoia (15.0) or later

### Getting Started

```bash
# Install packages and launch UI
pip install -U "cua-computer[all]" "cua-agent[all]"
python -m agent.ui.gradio.app
```

```python
import asyncio
from computer import Computer
from agent import ComputerAgent

async def main():
    computer = Computer()
    await computer.run()

    # Create app-specific desktop sessions
    desktop = computer.create_desktop_from_apps(["Notes"])

    # Initialize an agent
    agent = ComputerAgent(
        model="anthropic/claude-3-5-sonnet-20241022",
        tools=[desktop]
    )

    # Take a screenshot (returns bytes by default)
    screenshot = await desktop.interface.screenshot()
    with open("app_screenshot.png", "wb") as f:
        f.write(screenshot)

    # Run an agent task
    async for result in agent.run("Create a new note titled 'Meeting Notes' and add today's agenda items"):
        print(f"Agent: {result.get('text', '')}")

if __name__ == "__main__":
    asyncio.run(main())
```

## Use Case: Automating Your iPhone with Cua

### ⚠️ Important Warning

Computer-use agents are powerful tools that can interact with your devices. This guide involves using your own macOS and iPhone instead of a VM. **Proceed at your own risk.** Always:

- Review agent actions before running
- Start with non-critical tasks
- Monitor agent behavior closely

Remember with Cua it is still advised to use a VM for a better level of isolation for your agents.

### Setting Up iPhone Automation

### Step 1: Start the cua-computer-server

First, you'll need to start the cua-computer-server locally to enable access to iPhone Mirroring via the Computer interface:

```bash
# Install the server
pip install cua-computer-server

# Start the server
python -m computer_server
```

### Step 2: Connect iPhone Mirroring

Then, you'll need to open the "iPhone Mirroring" app on your Mac and connect it to your iPhone.

### Step 3: Create an iPhone Automation Session

Finally, you can create an iPhone automation session:

```python
import asyncio
from computer import Computer
from cua_agent import Agent

async def automate_iphone():
    # Connect to your local computer server
    my_mac = Computer(use_host_computer_server=True, os_type="macos", experiments=["app-use"])
    await my_mac.run()

    # Create a desktop focused on iPhone Mirroring
    my_iphone = my_mac.create_desktop_from_apps(["iPhone Mirroring"])

    # Initialize an agent for iPhone automation
    agent = ComputerAgent(
        model="anthropic/claude-3-5-sonnet-20241022",
        tools=[my_iphone]
    )

    # Example: Send a message
    async for result in agent.run("Open Messages and send 'Hello from Cua!' to John"):
        print(f"Agent: {result.get('text', '')}")

    # Example: Set a reminder
    async for result in agent.run("Create a reminder to call mom at 5 PM today"):
        print(f"Agent: {result.get('text', '')}")

if __name__ == "__main__":
    asyncio.run(automate_iphone())
```

### iPhone Automation Use Cases

With Cua's iPhone automation, you can:

- **Automate messaging**: Send texts, respond to messages, manage conversations
- **Control apps**: Navigate any iPhone app using natural language
- **Manage settings**: Adjust iPhone settings programmatically
- **Extract data**: Read information from apps that don't have APIs
- **Test iOS apps**: Automate testing workflows for iPhone applications

## Important Notes

- **Visual isolation only**: Apps share the same files, OS resources, and user session
- **Dynamic resolution**: Desktops automatically scale to fit app windows and menu bars
- **macOS only**: Currently requires macOS due to compositing engine dependencies
- **Not a security boundary**: This is for agent focus, not security isolation

## When to Use What: App-Use vs Multiple Cua Containers

### Use App-Use within the same macOS Cua Container:

- ✅ You need lightweight, fast agent focusing (macOS only)
- ✅ You want to run multiple agents on one desktop
- ✅ You're automating personal devices like iPhones
- ✅ Window layout isolation is sufficient
- ✅ You want low computational overhead

### Use Multiple Cua Containers:

- ✅ You need maximum isolation between agents
- ✅ You require cross-platform support (Mac/Linux/Windows)
- ✅ You need guaranteed resource allocation
- ✅ Security and complete isolation are critical
- ⚠️ Note: Most computationally expensive option

## Pro Tips

1. **Start Small**: Test with one app before creating complex multi-app desktops
2. **Screenshot First**: Take a screenshot to verify your desktop shows the right apps
3. **Name Your Apps Correctly**: Use exact app names as they appear in the system
4. **Consider Performance**: While lightweight, too many parallel agents can still impact system performance
5. **Plan Your Workflows**: Design agent tasks to minimize app switching for best results

### How It Works

When you create a desktop session with `create_desktop_from_apps()`, App Use:

- Filters the visual output to show only specified application windows
- Routes input events only to those applications
- Maintains window layout isolation between different sessions
- Shares the underlying file system and OS resources
- **Dynamically adjusts resolution** to fit the window layout and menu bar items

The resolution of these virtual desktops is dynamic, automatically scaling to accommodate the applications' window sizes and menu bar requirements. This ensures that agents always have a clear view of the entire interface they need to interact with, regardless of the specific app combination.

Currently, App Use is limited to macOS only due to its reliance on Quartz, Apple's powerful compositing engine, for creating these virtual desktops. Quartz provides the low-level window management and rendering capabilities that make it possible to composite multiple application windows into isolated visual environments.

## Conclusion

App Use brings a new dimension to computer automation - lightweight, focused, and parallel. Whether you're building a personal iPhone assistant or orchestrating a team of specialized agents, App Use provides the perfect balance of functionality and efficiency.

Ready to try it? Update to the latest Cua version and start focusing your agents today!

```bash
pip install -U "cua-computer[all]" "cua-agent[all]"
```

Happy automating! 🎯🤖
