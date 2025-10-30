# Announcing Cua Agent framework 0.4 and Composite Agents

_Published on August 26, 2025 by Dillon DuPont_

<img src="./assets/composite-agents.png" alt="Composite Agents">

So you want to build an agent that can use a computer. Great! You've probably discovered that there are now dozens of different AI models that claim they can click GUI buttons and fill out forms. Less great: actually getting them to work together is like trying to coordinate a group project where everyone speaks a different language and has invented seventeen different ways to say "click here".

Here's the thing about new GUI models: they're all special snowflakes. One model wants you to feed it images and expects coordinates back as percentages from 0 to 1. Another wants absolute pixel coordinates. A third model has invented its own numeral system with `<|loc095|><|loc821|>` tokens inside tool calls. Some models output Python code that calls `pyautogui.click(x, y)`. Others will start hallucinating coordinates if you forget to format all previous messages within a very specific GUI system prompt.

This is the kind of problem that makes you wonder if we're building the future of computing or just recreating the Tower of Babel with more GPUs.

## What we fixed

Agent framework 0.4 solves this by doing something radical: making all these different models speak the same language.

Instead of writing separate code for each model's peculiarities, you now just pick a model with a string like `"anthropic/claude-3-5-sonnet-20241022"` or `"huggingface-local/ByteDance-Seed/UI-TARS-1.5-7B"`, and everything else Just Works™. Behind the scenes, we handle all the coordinate normalization, token parsing, and image preprocessing so you don't have to.

```python
# This works the same whether you're using Anthropic, OpenAI, or that new model you found on Hugging Face
agent = ComputerAgent(
    model="anthropic/claude-3-5-sonnet-20241022",  # or any other supported model
    tools=[computer]
)
```

The output format is consistent across all providers (OpenAI, Anthropic, Vertex, Hugging Face, OpenRouter, etc.). No more writing different parsers for each model's creative interpretation of how to represent a mouse click.

## Composite Agents: Two Brains Are Better Than One

Here's where it gets interesting. We realized that you don't actually need one model to be good at everything. Some models are excellent at understanding what's on the screen—they can reliably identify buttons and text fields and figure out where to click. Other models are great at planning and reasoning but might be a bit fuzzy on the exact pixel coordinates.

So we let you combine them with a `+` sign:

```python
agent = ComputerAgent(
    # specify the grounding model first, then the planning model
    model="huggingface-local/HelloKKMe/GTA1-7B+huggingface-local/OpenGVLab/InternVL3_5-8B",
    tools=[computer]
)
```

This creates a composite agent where one model (the "grounding" model) handles the visual understanding and precise UI interactions, while the other (the "planning" model) handles the high-level reasoning and task orchestration. It's like having a pilot and a navigator, except they're both AI models and they're trying to help you star a GitHub repository.

You can even take a model that was never designed for computer use—like GPT-4o—and give it GUI capabilities by pairing it with a specialized vision model:

```python
agent = ComputerAgent(
    model="huggingface-local/HelloKKMe/GTA1-7B+openai/gpt-4o",
    tools=[computer]
)
```

## Example notebook

For a full, ready-to-run demo (install deps, local computer using Docker, and a composed agent example), see the notebook:

- https://github.com/trycua/cua/blob/models/opencua/notebooks/composite_agents_docker_nb.ipynb

## What's next

We're building integration with HUD evals, allowing us to curate and benchmark model combinations. This will help us identify which composite agent pairs work best for different types of tasks, and provide you with tested recommendations rather than just throwing model names at the wall to see what sticks.

If you try out version 0.4.x, we'd love to hear how it goes. Join us on Discord to share your results and let us know what model combinations work best for your projects.

---

## Links

- **Composite Agent Docs:** [https://docs.trycua.com/docs/agent-sdk/supported-agents/composed-agents](https://docs.trycua.com/docs/agent-sdk/supported-agents/composed-agents)
- **Discord:** [https://discord.gg/cua-ai](https://discord.gg/cua-ai)

Questions or weird edge cases? Ping us on Discord—we’re curious to see what you build.
