# Your Windows PC is Already the Perfect Development Environment for Computer-Use Agents

_Published on June 18, 2025 by Dillon DuPont_

Over the last few months, our enterprise users kept asking the same type of question: _"When are you adding support for AutoCAD?"_ _"What about SAP integration?"_ _"Can you automate our MES system?"_ - each request was for different enterprise applications we'd never heard of.

At first, we deflected. We've been building Cua to work across different environments - from [Lume for macOS VMs](./lume-to-containerization) to cloud containers. But these requests kept piling up. AutoCAD automation. SAP integration. Specialized manufacturing systems.

Then it hit us: **they all ran exclusively on Windows**.

Most of us develop on macOS, so we hadn't considered Windows as a primary target for agent automation. But we were missing out on helping customers automate the software that actually runs their businesses.

So last month, we started working on Windows support for [RPA (Robotic Process Automation)](https://en.wikipedia.org/wiki/Robotic_process_automation). Here's the twist: **the perfect development environment was already sitting on every Windows machine** - we just had to unlock it.

<div align="center">
  <video src="https://github.com/user-attachments/assets/7abbdaf4-054f-4965-8260-81dab497c6ba" width="600" controls></video>
</div>

## Our Journey to Windows CUA Support

When we started Cua, we focused on making computer-use agents work everywhere - we built [Lume for macOS](https://github.com/trycua/cua/tree/main/libs/lume), created cloud infrastructure, and worked on Linux support. But no matter what we built, Windows kept coming up in every enterprise conversation.

The pattern became clear during customer calls: **the software that actually runs businesses lives on Windows**. Engineering teams wanted agents to automate AutoCAD workflows. Manufacturing companies needed automation for their MES systems. Finance teams were asking about Windows-only trading platforms and legacy enterprise software.

We could have gone straight to expensive Windows cloud infrastructure, but then we discovered Microsoft had already solved the development problem: [Windows Sandbox](https://learn.microsoft.com/en-us/windows/security/application-security/application-isolation/windows-sandbox/). Lightweight, free, and sitting on every Windows machine waiting to be used.

Windows Sandbox support is our first step - **Windows cloud instances are coming later this month** for production workloads.

## What is Windows Sandbox?

Windows Sandbox is Microsoft's built-in lightweight virtualization technology. Despite the name, it's actually closer to a disposable virtual machine than a traditional "sandbox" - it creates a completely separate, lightweight Windows environment rather than just containerizing applications.

Here's how it compares to other approaches:

```bash
Traditional VM Testing:
┌─────────────────────────────────┐
│         Your Windows PC         │
├─────────────────────────────────┤
│      VMware/VirtualBox VM       │
│  (Heavy, Persistent, Complex)   │
├─────────────────────────────────┤
│        Agent Testing            │
└─────────────────────────────────┘

Windows Sandbox:
┌─────────────────────────────────┐
│         Your Windows PC         │
├─────────────────────────────────┤
│      Windows Sandbox           │
│   (Built-in, Fast, Disposable) │
├─────────────────────────────────┤
│    Separate Windows Instance    │
└─────────────────────────────────┘
```

> ⚠️ **Important Note**: Windows Sandbox supports **one virtual machine at a time**. For production workloads or running multiple agents simultaneously, you'll want our upcoming cloud infrastructure - but for learning and testing, this local setup is perfect to get started.

## Why Windows Sandbox is Perfect for Local Computer-Use Agent Testing

First, it's incredibly lightweight. We're talking seconds to boot up a fresh Windows environment, not the minutes you'd wait for a traditional VM. And since it's built into Windows 10 and 11, there's literally no setup cost - it's just sitting there waiting for you to enable it.

But the real magic is how disposable it is. Every time you start Windows Sandbox, you get a completely clean slate. Your agent messed something up? Crashed an application? No problem - just close the sandbox and start fresh. It's like having an unlimited supply of pristine Windows machines for testing.

## Getting Started: Three Ways to Test Agents

We've made Windows Sandbox agent testing as simple as possible. Here are your options:

### Option A: Quick Start with Agent UI (Recommended)

**Perfect for**: First-time users who want to see agents in action immediately

```bash
# One-time setup
pip install -U git+git://github.com/karkason/pywinsandbox.git
pip install -U "cua-computer[all]" "cua-agent[all]"

# Launch the Agent UI
python -m agent.ui
```

**What you get**:

- Visual interface in your browser
- Real-time agent action viewing
- Natural language task instructions
- No coding required

### Option B: Python API Integration

**Perfect for**: Developers building agent workflows

```python
import asyncio
from computer import Computer, VMProviderType
from agent import ComputerAgent, LLM

async def test_windows_agent():
    # Create Windows Sandbox computer
    computer = Computer(
        provider_type=VMProviderType.WINSANDBOX,
        os_type="windows",
        memory="4GB",
    )

    # Start the VM (~35s)
    await computer.run()

    # Create agent with your preferred model
    agent = ComputerAgent(
        model="openai/computer-use-preview",
        save_trajectory=True,
        tools=[computer]
    )

    # Give it a task
    async for result in agent.run("Open Calculator and compute 15% tip on $47.50"):
        print(f"Agent action: {result}")

    # Shutdown the VM
    await computer.stop()

asyncio.run(test_windows_agent())
```

**What you get**:

- Full programmatic control
- Custom agent workflows
- Integration with your existing code
- Detailed action logging

### Option C: Manual Configuration

**Perfect for**: Advanced users who want full control

1. Enable Windows Sandbox in Windows Features
2. Create custom .wsb configuration files
3. Integrate with your existing automation tools

## Comparing Your Options

Let's see how different testing approaches stack up:

### Windows Sandbox + Cua

- **Perfect for**: Quick testing and development
- **Cost**: Free (built into Windows)
- **Setup time**: Under 5 minutes
- **Safety**: Complete isolation from host system
- **Limitation**: One sandbox at a time
- **Requires**: Windows 10/11 with 4GB+ RAM

### Traditional VMs

- **Perfect for**: Complex testing scenarios
- **Full customization**: Any Windows version
- **Heavy resource usage**: Slow to start/stop
- **Complex setup**: License management required
- **Cost**: VM software + Windows licenses

## Real-World Windows RPA Examples

Here's what our enterprise users are building with Windows Sandbox:

### CAD and Engineering Automation

```python
# Example: AutoCAD drawing automation
task = """
1. Open AutoCAD and create a new drawing
2. Draw a basic floor plan with rooms and dimensions
3. Add electrical symbols and circuit layouts
4. Generate a bill of materials from the drawing
5. Export the drawing as both DWG and PDF formats
"""
```

### Manufacturing and ERP Integration

```python
# Example: SAP workflow automation
task = """
1. Open SAP GUI and log into the production system
2. Navigate to Material Management module
3. Create purchase orders for stock items below minimum levels
4. Generate vendor comparison reports
5. Export the reports to Excel and email to procurement team
"""
```

### Financial Software Automation

```python
# Example: Trading platform automation
task = """
1. Open Bloomberg Terminal or similar trading software
2. Monitor specific stock tickers and market indicators
3. Execute trades based on predefined criteria
4. Generate daily portfolio performance reports
5. Update risk management spreadsheets
"""
```

### Legacy Windows Application Integration

```python
# Example: Custom Windows application automation
task = """
1. Open legacy manufacturing execution system (MES)
2. Input production data from CSV files
3. Generate quality control reports
4. Update inventory levels across multiple systems
5. Create maintenance scheduling reports
"""
```

## System Requirements and Performance

### What You Need

- **Windows 10/11**: Any edition that supports Windows Sandbox
- **Memory**: 4GB minimum (8GB recommended for CAD/professional software)
- **CPU**: Virtualization support (enabled by default on modern systems)
- **Storage**: A few GB free space

### Performance Tips

- **Close unnecessary applications** before starting Windows Sandbox
- **Allocate appropriate memory** based on your RPA workflow complexity
- **Use SSD storage** for faster sandbox startup
- **Consider dedicated hardware** for resource-intensive applications like CAD software

**Stay tuned** - we'll be announcing Windows Cloud Instances later this month.

But for development, prototyping, and learning Windows RPA workflows, **Windows Sandbox gives you everything you need to get started right now**.

## Learn More

- [Windows Sandbox Documentation](https://learn.microsoft.com/en-us/windows/security/application-security/application-isolation/windows-sandbox/)
- [Cua GitHub Repository](https://github.com/trycua/cua)
- [Agent UI Documentation](https://github.com/trycua/cua/tree/main/libs/agent)
- [Join our Discord Community](https://discord.gg/cua-ai)

---

_Ready to see AI agents control your Windows applications? Come share your testing experiences on Discord!_
