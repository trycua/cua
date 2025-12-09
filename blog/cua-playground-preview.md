# Cua Playground: Agents + Sandboxes in Your Browser

Building computer-use agents means constant iteration—writing code, deploying to a sandbox, testing behavior, debugging issues, then repeating the cycle. Every test requires switching between your code editor, terminal, and VNC viewer. Want to try a different prompt? Edit your code, redeploy, and wait for the agent to restart. It works, but it's slow.

Today we're launching the **Cua Playground**: a browser-based environment for testing computer-use agents without writing code. Send messages to your sandboxes, watch them execute in real-time, and iterate on prompts instantly—all from your dashboard at cua.ai.

![Cua Playground](https://github.com/user-attachments/assets/af1071ba-3df3-4e4b-aafb-df8c3d00b0a5)

**What's new with this release:**

- Instant testing—send messages to any running sandbox directly from your browser
- Real-time execution—watch your agent work with live tool call updates and screenshots
- Multi-model support—test with Claude Sonnet 4.5, Haiku 4.5, and more
- Persistent chat history—conversations save automatically to local storage

The Playground connects to your existing Cua sandboxes—the same ones you use with the Agent SDK. Select a running sandbox and a model, then start chatting. The agent uses computer-use tools (mouse, keyboard, bash, editor) to complete your tasks, and you see every action it takes.

## Getting Started Today

<div align="center">
  <video src="https://github.com/user-attachments/assets/9fef0f30-1024-4833-8b7a-6a2c02d8eb99" width="600" controls></video>
</div>


Sign up at [cua.ai/signin](https://cua.ai/signin) and grab your API key from the dashboard. Then navigate to the Playground:

1. Navigate to Dashboard > Playground
2. Select a sandbox from the dropdown (must be "running" status)
3. Choose a model (we recommend Claude Sonnet 4.5 to start)
4. Send a message: "Take a screenshot and describe what you see"
5. Watch the agent execute computer actions in real-time

Example use cases:

**Prompt Testing**
```
❌ "Check the website"
✅ "Navigate to example.com in Firefox and take a screenshot of the homepage"
```

**Model Comparison**
Run the same task with different models to compare quality, speed, and cost.

**Debugging Agent Behavior**
1. Send: "Find the login button and click it"
2. View tool calls to see each mouse movement
3. Check screenshots to verify the agent found the right element
4. Adjust your prompt based on what you observe

## FAQs

<details>
<summary><strong>Do I need to know how to code?</strong></summary>

No. The Playground is designed for testing agent behavior without writing code. However, for production deployments, you'll need to use the Agent SDK (Python/TypeScript).

</details>

<details>
<summary><strong>Does this replace the Agent SDK?</strong></summary>

No. The Playground is for rapid testing and experimentation. For production deployments, scheduled tasks, or complex workflows, use the Agent SDK.

</details>

<details>
<summary><strong>How much does it cost?</strong></summary>

Playground requests use the same credit system as Agent SDK requests. You're charged for model inference (varies by model) and sandbox runtime (billed per hour while running).

</details>

<details>
<summary><strong>Why is my sandbox not showing up?</strong></summary>

The sandbox must have `status = "running"` to appear in the dropdown. Check Dashboard > Sandboxes to verify status. If stopped, click "Start" and wait ~30 seconds for it to become available.

</details>

## Need help?

If you hit issues getting the Playground working, reach out in [Discord](https://discord.gg/cua-ai). We respond fast and fix based on what people actually use.

---

Get started at [cua.ai](https://cua.ai) or try the Playground at [cua.ai/dashboard/playground](https://cua.ai/dashboard/playground).
