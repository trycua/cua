# When Agents Need Human Wisdom - Introducing Human-In-The-Loop Support

_Published on August 29, 2025 by Francesco Bonacci_

Sometimes the best AI agent is a human. Whether you're creating training demonstrations, evaluating complex scenarios, or need to intervene when automation hits a wall, our new Human-In-The-Loop integration puts you directly in control.

With yesterday's [HUD evaluation integration](hud-agent-evals.md), you could benchmark any agent at scale. Today's update lets you _become_ the agent when it matters most—seamlessly switching between automated intelligence and human judgment.

<div align="center">
  <video src="https://github.com/user-attachments/assets/9091b50f-26e7-4981-95ce-40e5d42a1260" width="600" controls></video>
</div>

## What you get

- **One-line human takeover** for any agent configuration with `human/human` or `model+human/human`
- **Interactive web UI** to see what your agent sees and control what it does
- **Zero context switching** - step in exactly where automation left off
- **Training data generation** - create perfect demonstrations by doing tasks yourself
- **Ground truth evaluation** - validate agent performance with human expertise

## Why Human-In-The-Loop?

Even the most sophisticated agents encounter edge cases, ambiguous interfaces, or tasks requiring human judgment. Rather than failing gracefully, they can now fail _intelligently_—by asking for human help.

This approach bridges the gap between fully automated systems and pure manual control, letting you:

- **Demonstrate complex workflows** that agents can learn from
- **Evaluate tricky scenarios** where ground truth requires human assessment
- **Intervene selectively** when automated agents need guidance
- **Test and debug** your tools and environments manually

## Getting Started

Launch the human agent interface:

```bash
python -m agent.human_tool
```

The web UI will show pending completions. Click any completion to take control of the agent and see exactly what it sees.

## Usage Examples

### Direct Human Control

Perfect for creating demonstrations or when you want full manual control:

```python
from agent import ComputerAgent
from agent.computer import computer

agent = ComputerAgent(
    "human/human",
    tools=[computer]
)

# You'll get full control through the web UI
async for _ in agent.run("Take a screenshot, analyze the UI, and click on the most prominent button"):
    pass
```

### Hybrid: AI Planning + Human Execution

Combine model intelligence with human precision—let AI plan, then execute manually:

```python
agent = ComputerAgent(
    "huggingface-local/HelloKKMe/GTA1-7B+human/human",
    tools=[computer]
)

# AI creates the plan, human executes each step
async for _ in agent.run("Navigate to the settings page and enable dark mode"):
    pass
```

### Fallback Pattern

Start automated, escalate to human when needed:

```python
# Primary automated agent
primary_agent = ComputerAgent("openai/computer-use-preview", tools=[computer])

# Human fallback agent
fallback_agent = ComputerAgent("human/human", tools=[computer])

try:
    async for result in primary_agent.run(task):
        if result.confidence < 0.7:  # Low confidence threshold
            # Seamlessly hand off to human
            async for _ in fallback_agent.run(f"Continue this task: {task}"):
                pass
except Exception:
    # Agent failed, human takes over
    async for _ in fallback_agent.run(f"Handle this failed task: {task}"):
        pass
```

## Interactive Features

The human-in-the-loop interface provides a rich, responsive experience:

### **Visual Environment**

- **Screenshot display** with live updates as you work
- **Click handlers** for direct interaction with UI elements
- **Zoom and pan** to see details clearly

### **Action Controls**

- **Click actions** - precise cursor positioning and clicking
- **Keyboard input** - type text naturally or send specific key combinations
- **Action history** - see the sequence of actions taken
- **Undo support** - step back when needed

### **Tool Integration**

- **Full OpenAI compatibility** - standard tool call format
- **Custom tools** - integrate your own tools seamlessly
- **Real-time feedback** - see tool responses immediately

### **Smart Polling**

- **Responsive updates** - UI refreshes when new completions arrive
- **Background processing** - continue working while waiting for tasks
- **Session persistence** - resume interrupted sessions

## Real-World Use Cases

### **Training Data Generation**

Create perfect demonstrations for fine-tuning:

```python
# Generate training examples for spreadsheet tasks
demo_agent = ComputerAgent("human/human", tools=[computer])

tasks = [
    "Create a budget spreadsheet with income and expense categories",
    "Apply conditional formatting to highlight overbudget items",
    "Generate a pie chart showing expense distribution"
]

for task in tasks:
    # Human demonstrates each task perfectly
    async for _ in demo_agent.run(task):
        pass  # Recorded actions become training data
```

### **Evaluation and Ground Truth**

Validate agent performance on complex scenarios:

```python
# Human evaluates agent performance
evaluator = ComputerAgent("human/human", tools=[computer])

async for _ in evaluator.run("Review this completed form and rate accuracy (1-10)"):
    pass  # Human provides authoritative quality assessment
```

### **Interactive Debugging**

Step through agent behavior manually:

```python
# Test a workflow step by step
debug_agent = ComputerAgent("human/human", tools=[computer])

async for _ in debug_agent.run("Reproduce the agent's failed login sequence"):
    pass  # Human identifies exactly where automation breaks
```

### **Edge Case Handling**

Handle scenarios that break automated agents:

```python
# Complex UI interaction requiring human judgment
edge_case_agent = ComputerAgent("human/human", tools=[computer])

async for _ in edge_case_agent.run("Navigate this CAPTCHA-protected form"):
    pass  # Human handles what automation cannot
```

## Configuration Options

Customize the human agent experience:

- **UI refresh rate**: Adjust polling frequency for your workflow
- **Image quality**: Balance detail vs. performance for screenshots
- **Action logging**: Save detailed traces for analysis and training
- **Session timeout**: Configure idle timeouts for security
- **Tool permissions**: Restrict which tools humans can access

## When to Use Human-In-The-Loop

| **Scenario**                 | **Why Human Control**                                 |
| ---------------------------- | ----------------------------------------------------- |
| **Creating training data**   | Perfect demonstrations for model fine-tuning          |
| **Evaluating complex tasks** | Human judgment for subjective or nuanced assessment   |
| **Handling edge cases**      | CAPTCHAs, unusual UIs, context-dependent decisions    |
| **Debugging workflows**      | Step through failures to identify breaking points     |
| **High-stakes operations**   | Critical tasks requiring human oversight and approval |
| **Testing new environments** | Validate tools and environments work as expected      |

## Learn More

- **Interactive examples**: Try human-in-the-loop control with sample tasks
- **Training data pipelines**: Learn how to convert human demonstrations into model training data
- **Evaluation frameworks**: Build human-validated test suites for your agents
- **API documentation**: Full reference for human agent configuration

Ready to put humans back in the loop? The most sophisticated AI system knows when to ask for help.

---

_Questions about human-in-the-loop agents? Join the conversation in our [Discord community](https://discord.gg/cua-ai) or check out our [documentation](https://docs.trycua.com/docs/agent-sdk/supported-agents/human-in-the-loop)._
