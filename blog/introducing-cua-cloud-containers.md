# Introducing Cua Cloud Sandbox: Computer-Use Agents in the Cloud

_Published on May 28, 2025 by Francesco Bonacci_

Welcome to the next chapter in our Computer-Use Agent journey! In [Part 1](./build-your-own-operator-on-macos-1), we showed you how to build your own Operator on macOS. In [Part 2](./build-your-own-operator-on-macos-2), we explored the cua-agent framework. Today, we're excited to introduce **Cua Cloud Sandbox** – the easiest way to deploy Computer-Use Agents at scale.

<div align="center">
  <video src="https://github.com/user-attachments/assets/63a2addf-649f-4468-971d-58d38dd43ee6" width="600" controls></video>
</div>

## What is Cua Cloud?

Think of Cua Cloud as **Docker for Computer-Use Agents**. Instead of managing VMs, installing dependencies, and configuring environments, you can launch pre-configured Cloud Sandbox instances with a single command. Each sandbox comes with a **full desktop environment** accessible via browser (via noVNC), all CUA-related dependencies pre-configured (with a PyAutoGUI-compatible server), and **pay-per-use pricing** that scales with your needs.

## Why Cua Cloud Sandbox?

Four months ago, we launched [**Lume**](https://github.com/trycua/cua/tree/main/libs/lume) and [**Cua**](https://github.com/trycua/cua) with the goal to bring sandboxed VMs and Computer-Use Agents on Apple Silicon. The developer's community response was incredible 🎉

Going from prototype to production revealed a problem though: **local macOS VMs don't scale**, neither are they easily portable.

Our Discord community, YC peers, and early pilot customers kept hitting the same issues. Storage constraints meant **20-40GB per VM** filled laptops fast. Different hardware architectures (Apple Silicon ARM vs Intel x86) prevented portability of local workflows. Every new user lost a day to setup and configuration.

**Cua Cloud** eliminates these constraints while preserving everything developers are familiar with about our Computer and Agent SDK.

### What We Built

Over the past month, we've been iterating over Cua Cloud with partners and beta users to address these challenges. You use the exact same `Computer` and `ComputerAgent` classes you already know, but with **zero local setup** or storage requirements. VNC access comes with **built-in encryption**, you pay only for compute time (not idle resources), and can bring your own API keys for any LLM provider.

The result? **Instant deployment** in seconds instead of hours, with no infrastructure to manage. Scale elastically from **1 to 100 agents** in parallel, with consistent behavior across all deployments. Share agent trajectories with your team for better collaboration and debugging.

## Getting Started

### Step 1: Get Your API Key

Sign up at [**trycua.com**](https://trycua.com) to get your API key.

```bash
# Set your API key in environment variables
export CUA_API_KEY=your_api_key_here
export CUA_CONTAINER_NAME=my-agent-container
```

### Step 2: Launch Your First Sandbox

```python
import asyncio
from computer import Computer, VMProviderType
from agent import ComputerAgent

async def run_cloud_agent():
    # Create a remote Linux computer with Cua Cloud
    computer = Computer(
        os_type="linux",
        api_key=os.getenv("CUA_API_KEY"),
        name=os.getenv("CUA_CONTAINER_NAME"),
        provider_type=VMProviderType.CLOUD,
    )

    # Create an agent with your preferred loop
    agent = ComputerAgent(
        model="openai/gpt-4o",
        save_trajectory=True,
        verbosity=logging.INFO,
        tools=[computer]
    )

    # Run a task
    async for result in agent.run("Open Chrome and search for AI news"):
        print(f"Response: {result.get('text')}")

# Run the agent
asyncio.run(run_cloud_agent())
```

### Available Tiers

We're launching with **three compute tiers** to match your workload needs:

- **Small** (1 vCPU, 4GB RAM) - Perfect for simple automation tasks and testing
- **Medium** (2 vCPU, 8GB RAM) - Ideal for most production workloads
- **Large** (8 vCPU, 32GB RAM) - Built for complex, resource-intensive operations

Each tier includes a **full Linux with Xfce desktop environment** with pre-configured browser, **secure VNC access** with SSL, persistent storage during your session, and automatic cleanup on termination for sandboxes.

## How some customers are using Cua Cloud today

### Example 1: Automated GitHub Workflow

Let's automate a complete GitHub workflow:

```python
import asyncio
import os
from computer import Computer, VMProviderType
from agent import ComputerAgent

async def github_automation():
    """Automate GitHub repository management tasks."""
    computer = Computer(
        os_type="linux",
        api_key=os.getenv("CUA_API_KEY"),
        name="github-automation",
        provider_type=VMProviderType.CLOUD,
    )

    agent = ComputerAgent(
        model="openai/gpt-4o",
        save_trajectory=True,
        verbosity=logging.INFO,
        tools=[computer]
    )

    tasks = [
        "Look for a repository named trycua/cua on GitHub.",
        "Check the open issues, open the most recent one and read it.",
        "Clone the repository if it doesn't exist yet.",
        "Create a new branch for the issue.",
        "Make necessary changes to resolve the issue.",
        "Commit the changes with a descriptive message.",
        "Create a pull request."
    ]

    for i, task in enumerate(tasks):
        print(f"\nExecuting task {i+1}/{len(tasks)}: {task}")
        async for result in agent.run(task):
            print(f"Response: {result.get('text')}")

            # Check if any tools were used
            tools = result.get('tools')
            if tools:
                print(f"Tools used: {tools}")

        print(f"Task {i+1} completed")

# Run the automation
asyncio.run(github_automation())
```

### Example 2: Parallel Web Scraping

Run multiple agents in parallel to scrape different websites:

```python
import asyncio
from computer import Computer, VMProviderType
from agent import ComputerAgent

async def scrape_website(site_name, url):
    """Scrape a website using a cloud agent."""
    computer = Computer(
        os_type="linux",
        api_key=os.getenv("CUA_API_KEY"),
        name=f"scraper-{site_name}",
        provider_type=VMProviderType.CLOUD,
    )

    agent = ComputerAgent(
        model="openai/gpt-4o",
        save_trajectory=True,
        tools=[computer]
    )

    results = []
    tasks = [
        f"Navigate to {url}",
        "Extract the main headlines or article titles",
        "Take a screenshot of the page",
        "Save the extracted data to a file"
    ]

    for task in tasks:
        async for result in agent.run(task):
            results.append({
                'site': site_name,
                'task': task,
                'response': result.get('text')
            })

    return results

async def parallel_scraping():
    """Scrape multiple websites in parallel."""
    sites = [
        ("ArXiv", "https://arxiv.org"),
        ("HackerNews", "https://news.ycombinator.com"),
        ("TechCrunch", "https://techcrunch.com")
    ]

    # Run all scraping tasks in parallel
    tasks = [scrape_website(name, url) for name, url in sites]
    results = await asyncio.gather(*tasks)

    # Process results
    for site_results in results:
        print(f"\nResults from {site_results[0]['site']}:")
        for result in site_results:
            print(f"  - {result['task']}: {result['response'][:100]}...")

# Run parallel scraping
asyncio.run(parallel_scraping())
```

## Cost Optimization Tips

To optimize your costs, use appropriate sandbox sizes for your workload and implement timeouts to prevent runaway tasks. Batch related operations together to minimize sandbox spin-up time, and always remember to terminate sandboxes when your work is complete.

## Security Considerations

Cua Cloud runs all sandboxes in isolated environments with encrypted VNC connections. Your API keys are never exposed in trajectories.

## What's Next for Cua Cloud

We're just getting started! Here's what's coming in the next few months:

### Elastic Autoscaled Sandbox Pools

Soon you'll be able to create elastic sandbox pools that automatically scale based on demand. Define minimum and maximum sandbox counts, and let Cua Cloud handle the rest. Perfect for batch processing, scheduled automations, and handling traffic spikes without manual intervention.

### Windows and macOS Cloud Support

While we're launching with Linux sandboxes, Windows and macOS cloud machines are coming soon. Run Windows-specific automations, test cross-platform workflows, or leverage macOS-exclusive applications – all in the cloud with the same simple API.

Stay tuned for updates and join our [**Discord**](https://discord.gg/cua-ai) to vote on which features you'd like to see first!

## Get Started Today

Ready to deploy your Computer-Use Agents in the cloud?

Visit [**trycua.com**](https://trycua.com) to sign up and get your API key. Join our [**Discord community**](https://discord.gg/cua-ai) for support and explore more examples on [**GitHub**](https://github.com/trycua/cua).

Happy RPA 2.0! 🚀
