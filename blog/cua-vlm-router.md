# Cua VLM Router: One Provider for All Your Computer-Use Models

If you've been building computer-use agents, you know the reality: every model provider has its own specification and deployment process. Anthropic has one API format, OpenAI another, Google something else entirely. Want to try a Hugging Face model? That's a completely different setup. Self-hosting? Even more complexity. Each provider requires learning their specific API, managing their credentials, and adapting your code to their particular requirements.

Today we're launching the **Cua VLM Router**: a managed inference API that gives you unified access to multiple vision-language model providers through a single API key. We're starting with Anthropic's Claude models (Sonnet 4.5 and Haiku 4.5)—some of the most loved and widely-used computer-use models in the Cua ecosystem - with more providers coming soon.

![Cua VLM Router Banner](https://github.com/user-attachments/assets/1b978f62-2cae-4cf7-932a-55ac8c8f2e06)

## What You Get

The Cua VLM Router handles the infrastructure so you can focus on building:

**Single API Key**

- One key for all model providers (no juggling multiple credentials)
- Works for both model inference and sandbox access
- Manage everything from one dashboard at cua.ai

**Smart Routing**

- Automatic provider selection for optimal availability and performance
- For Anthropic models, we route to the best provider (Anthropic, AWS Bedrock, or Microsoft Foundry)
- No configuration needed—just specify the model and we handle the rest

**Cost Tracking & Optimization**

- Unified usage dashboard across all models
- Real-time credit balance tracking
- Detailed cost breakdown per request (gateway cost + upstream cost)

**Production-Ready**

- OpenAI-compatible API (drop-in replacement for existing code)
- Full streaming support with Server-Sent Events
- Metadata about routing decisions in every response

## Available Models (Launch)

We're starting with Anthropic's latest Claude models:

| Model                             | Best For                           |
| --------------------------------- | ---------------------------------- |
| `cua/anthropic/claude-sonnet-4.5` | General-purpose tasks, recommended |
| `cua/anthropic/claude-haiku-4.5`  | Fast responses, cost-effective     |

## How It Works

When you request an Anthropic model through Cua, we automatically route to the best available provider—whether that's Anthropic directly, AWS Bedrock, or Microsoft Foundry. You just specify `cua/anthropic/claude-sonnet-4.5`, and we handle the provider selection, failover, and optimization behind the scenes. No need to manage multiple accounts or implement fallback logic yourself.

## Getting Started

Sign up at [cua.ai/signin](https://cua.ai/signin) and create your API key from **Dashboard > API Keys > New API Key** (save it immediately—you won't see it again).

Use it with the Agent SDK (make sure to set your environment variable):

```python
import asyncio
from agent import ComputerAgent
from computer import Computer

async def main():
  # Initialize cloud computer
  computer = Computer(
    os_type="linux",
    provider_type="cloud",
    name="your-container-name",
    api_key="your-cua-api-key"
  )

  # Initialize agent with Claude Sonnet 4.5
  agent = ComputerAgent(
    tools=[computer],
    model="cua/anthropic/claude-sonnet-4.5",
    api_key="your-cua-api-key",
    instructions="You are a helpful assistant that can control computers",
    only_n_most_recent_images=3
  )

  # Run a task
  async for result in agent.run("Open a browser and search for Python tutorials"):
    print(result)

if __name__ == "__main__":
  asyncio.run(main())
```

## Migration is Simple

Already using Anthropic directly? Just add the `cua/` prefix:

**Before:**

```python
export ANTHROPIC_API_KEY="sk-ant-..."
agent = ComputerAgent(model="anthropic/claude-sonnet-4-5-20250929")
```

**After:**

```python
export CUA_API_KEY="sk_cua-api01_..."
agent = ComputerAgent(model="cua/anthropic/claude-sonnet-4.5")
```

Same code structure. No other changes needed.

## Direct API Access

The router exposes an OpenAI-compatible API at `https://inference.cua.ai/v1`:

```bash
curl -X POST https://inference.cua.ai/v1/messages \
  -H "Authorization: Bearer ${CUA_API_KEY}" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "anthropic/claude-sonnet-4.5",
    "messages": [{"role": "user", "content": "Hello!"}],
    "stream": true
  }'
```

Works with any OpenAI-compatible client library.

## FAQs

<details>
<summary><strong>Do I still need provider API keys?</strong></summary>

No. Cua manages all provider API keys and infrastructure. You only need one Cua API key for everything—model inference and sandbox access.

</details>

<details>
<summary><strong>How does pricing work?</strong></summary>

Requests are billed in credits, deducted from your Cua account balance. Every response includes both the Cua gateway cost and the actual upstream API cost for transparency.

</details>

<details>
<summary><strong>Can I still use my own Anthropic key (BYOK)?</strong></summary>

Yes. The agent SDK still supports direct provider access. Just use `anthropic/claude-sonnet-4-5-20250929` instead of the `cua/` prefix and set your `ANTHROPIC_API_KEY`. See [Supported Model Providers](https://cua.ai/docs/cua/guide/fundamentals/vlms) for details.

</details>

<details>
<summary><strong>What about other providers?</strong></summary>

We're starting with Anthropic and adding more providers based on what people actually use. Request access to specific models in [Discord](https://discord.gg/cua-ai).

</details>

<details>
<summary><strong>Does streaming work?</strong></summary>

Yes. Set `"stream": true` in your request to receive Server-Sent Events. Works identically to OpenAI's streaming API.

</details>

## What's Next

This is just the beginning. We're actively iterating based on feedback:

- Additional model providers
- Custom model routing rules
- Usage alerts and budget controls
- Team collaboration features

If there's a model or feature you need, let us know in [Discord](https://discord.gg/cua-ai).

## Need Help?

- **Documentation**: [cua.ai/docs/cua/guide/fundamentals/cua-vlm-router](https://cua.ai/docs/cua/guide/fundamentals/cua-vlm-router)
- **Quickstart Guide**: [cua.ai/docs/cua/guide/get-started/using-agent-sdk](https://cua.ai/docs/cua/guide/get-started/using-agent-sdk)
- **Discord Community**: [discord.gg/cua-ai](https://discord.gg/cua-ai)

---

Get started at [cua.ai](https://cua.ai) or check out the [VLM Router docs](https://cua.ai/docs/cua/guide/fundamentals/cua-vlm-router).
