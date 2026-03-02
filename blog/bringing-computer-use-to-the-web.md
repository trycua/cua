# Bringing Computer-Use to the Web

_Published on August 5, 2025 by Morgan Dean_

In one of our original posts, we explored building Computer-Use Operators on macOS - first with a [manual implementation](build-your-own-operator-on-macos-1.md) using OpenAI's `computer-use-preview` model, then with our [cua-agent framework](build-your-own-operator-on-macos-2.md) for Python developers. While these tutorials have been incredibly popular, we've received consistent feedback from our community: **"Can we use Cua with JavaScript and TypeScript?"**

Today, we're excited to announce the release of the **`@trycua/computer` Web SDK** - a new library that allows you to control your Cua cloud containers from any JavaScript or TypeScript project. With this library, you can click, type, and grab screenshots from your cloud containers - no extra servers required.

With this new SDK, you can easily develop Cua experiences like the one below, which we will release soon as open source.

<div align="center">
  <video src="https://github.com/user-attachments/assets/e213d6c3-73b6-48dd-a7d9-ed761ed74f89" width="600" controls></video>
</div>

Let’s see how it works.

## What You'll Learn

By the end of this tutorial, you'll be able to:

- Set up the `@trycua/computer` npm library in any JavaScript/TypeScript project
- Connect OpenAI's computer-use model to Cua cloud containers from web applications
- Build computer-use agents that work in Node.js, React, Vue, or any web framework
- Handle different types of computer actions (clicking, typing, scrolling) from web code
- Implement the complete computer-use loop in JavaScript/TypeScript
- Integrate AI automation into existing web applications and workflows

**Prerequisites:**

- Node.js 16+ and npm/yarn/pnpm
- Basic JavaScript or TypeScript knowledge
- OpenAI API access (Tier 3+ for computer-use-preview)
- Cua cloud container credits ([get started here](https://cua.ai/pricing))

**Estimated Time:** 45-60 minutes

## Access Requirements

### OpenAI Model Availability

At the time of writing, the **computer-use-preview** model has limited availability:

- Only accessible to OpenAI tier 3+ users
- Additional application process may be required even for eligible users
- Cannot be used in the OpenAI Playground
- Outside of ChatGPT Operator, usage is restricted to the new **Responses API**

Luckily, the `@trycua/computer` library can be used in conjunction with other models, like [Anthropic’s Computer Use](https://platform.claude.com/docs/en/agents-and-tools/tool-use/computer-use-tool) or [UI-TARS](https://huggingface.co/ByteDance-Seed/UI-TARS-1.5-7B). You’ll just have to write your own handler to parse the model output for interfacing with the container.

### Cua Cloud Containers

To follow this guide, you’ll need access to a Cua cloud container.

Getting access is simple: purchase credits from our [pricing page](https://cua.ai/pricing), then create and provision a new container instance from the [dashboard](https://cua.ai/dashboard/containers). With your container running, you'll be ready to leverage the web SDK and bring automation to your JavaScript or TypeScript applications.

## Understanding the Flow

### OpenAI API Overview

Let's start with the basics. In our case, we'll use OpenAI's API to communicate with their computer-use model.

Think of it like this:

1. We send the model a screenshot of our container and tell it what we want it to do
2. The model looks at the screenshot and decides what actions to take
3. It sends back instructions (like "click here" or "type this")
4. We execute those instructions in our container.

### Model Setup

Here's how we set up the computer-use model for web development:

```javascript
const res = await openai.responses.create({
  model: 'computer-use-preview',
  tools: [
    {
      type: 'computer_use_preview',
      display_width: 1024,
      display_height: 768,
      environment: 'linux', // we're using a linux container
    },
  ],
  input: [
    {
      role: 'user',
      content: [
        // what we want the ai to do
        { type: 'input_text', text: 'Open firefox and go to cua.ai' },
        // first screenshot of the vm
        {
          type: 'input_image',
          image_url: `data:image/png;base64,${screenshotBase64}`,
          detail: 'auto',
        },
      ],
    },
  ],
  truncation: 'auto',
});
```

### Understanding the Response

When we send a request, the API sends back a response that looks like this:

```json
"output": [
    {
        "type": "reasoning",           // The AI explains what it's thinking
        "id": "rs_67cc...",
        "summary": [
            {
                "type": "summary_text",
                "text": "Clicking on the browser address bar."
            }
        ]
    },
    {
        "type": "computer_call",       // The actual action to perform
        "id": "cu_67cc...",
        "call_id": "call_zw3...",      // Used to track previous calls
        "action": {
            "type": "click",           // What kind of action (click, type, etc.)
            "button": "left",          // Which mouse button to use
            "x": 156,                  // Where to click (coordinates)
            "y": 50
        },
        "pending_safety_checks": [],   // Any safety warnings to consider
        "status": "completed"          // Whether the action was successful
    }
]

```

Each response contains:

1. **Reasoning**: The AI's explanation of what it's doing
2. **Action**: The specific computer action to perform
3. **Safety Checks**: Any potential risks to review
4. **Status**: Whether everything worked as planned

## Implementation Guide

### Provision a Cua Cloud Container

1. Visit [cua.ai](https://cua.ai), sign up, purchase [credits](https://cua.ai/pricing), and create a new container instance from the [dashboard](https://cua.ai/dashboard).
2. Create an API key from the dashboard — be sure to save it in a secure location before continuing.
3. Start the cloud container from the dashboard.

### Environment Setup

1. Install required packages with your preferred package manager:

   ```bash
   npm install --save @trycua/computer # or yarn, pnpm, bun
   npm install --save openai # or yarn, pnpm, bun
   ```

   Works with any JavaScript/TypeScript project setup - whether you're using Create React App, Next.js, Vue, Angular, or plain JavaScript.

2. Save your OpenAI API key, Cua API key, and container name to a `.env` file:

   ```bash
   OPENAI_API_KEY=openai-api-key
   CUA_API_KEY=cua-api-key
   CUA_CONTAINER_NAME=cua-cloud-container-name
   ```

   These environment variables work the same whether you're using vanilla JavaScript, TypeScript, or any web framework.

## Building the Agent

### Mapping API Actions to `@trycua/computer` Interface Methods

This helper function handles a `computer_call` action from the OpenAI API — converting the action into an equivalent action from the `@trycua/computer` interface. These actions will execute on the initialized `Computer` instance. For example, `await computer.interface.leftClick()` sends a mouse left click to the current cursor position.

Whether you're using JavaScript or TypeScript, the interface remains the same:

```javascript
export async function executeAction(
  computer: Computer,
  action: OpenAI.Responses.ResponseComputerToolCall['action']
) {
  switch (action.type) {
    case 'click':
      const { x, y, button } = action;
      console.log(`Executing click at (${x}, ${y}) with button '${button}'.`);
      await computer.interface.moveCursor(x, y);
      if (button === 'right') await computer.interface.rightClick();
      else await computer.interface.leftClick();
      break;
    case 'type':
      const { text } = action;
      console.log(`Typing text: ${text}`);
      await computer.interface.typeText(text);
      break;
    case 'scroll':
      const { x: locX, y: locY, scroll_x, scroll_y } = action;
      console.log(
        `Scrolling at (${locX}, ${locY}) with offsets (scroll_x=${scroll_x}, scroll_y=${scroll_y}).`
      );
      await computer.interface.moveCursor(locX, locY);
      await computer.interface.scroll(scroll_x, scroll_y);
      break;
    case 'keypress':
      const { keys } = action;
      for (const key of keys) {
        console.log(`Pressing key: ${key}.`);
        // Map common key names to Cua equivalents
        if (key.toLowerCase() === 'enter') {
          await computer.interface.pressKey('return');
        } else if (key.toLowerCase() === 'space') {
          await computer.interface.pressKey('space');
        } else {
          await computer.interface.pressKey(key);
        }
      }
      break;
    case 'wait':
      console.log(`Waiting for 3 seconds.`);
      await new Promise((resolve) => setTimeout(resolve, 3 * 1000));
      break;
    case 'screenshot':
      console.log('Taking screenshot.');
      // This is handled automatically in the main loop, but we can take an extra one if requested
      const screenshot = await computer.interface.screenshot();
      return screenshot;
    default:
      console.log(`Unrecognized action: ${action.type}`);
      break;
  }
}
```

### Implementing the Computer-Use Loop

This section defines a loop that:

1. Initializes the `Computer` instance (connecting to a Linux cloud container).
2. Captures a screenshot of the current state.
3. Sends the screenshot (with a user prompt) to the OpenAI Responses API using the `computer-use-preview` model.
4. Processes the returned `computer_call` action and executes it using our helper function.
5. Captures an updated screenshot after the action.
6. Send the updated screenshot and loops until no more actions are returned.

```javascript
const openai = new OpenAI({ apiKey: process.env.OPENAI_API_KEY });

// Initialize the Computer Connection
const computer = new Computer({
  apiKey: process.env.CUA_API_KEY!,
  name: process.env.CUA_CONTAINER_NAME!,
  osType: OSType.LINUX,
});

await computer.run();
// Take the initial screenshot
const screenshot = await computer.interface.screenshot();
const screenshotBase64 = screenshot.toString('base64');

// Setup openai config for computer use
const computerUseConfig: OpenAI.Responses.ResponseCreateParamsNonStreaming = {
  model: 'computer-use-preview',
  tools: [
    {
      type: 'computer_use_preview',
      display_width: 1024,
      display_height: 768,
      environment: 'linux', // we're using a linux vm
    },
  ],
  truncation: 'auto',
};

// Send initial screenshot to the openai computer use model
let res = await openai.responses.create({
  ...computerUseConfig,
  input: [
    {
      role: 'user',
      content: [
        // what we want the ai to do
        { type: 'input_text', text: 'open firefox and go to cua.ai' },
        // current screenshot of the vm
        {
          type: 'input_image',
          image_url: `data:image/png;base64,${screenshotBase64}`,
          detail: 'auto',
        },
      ],
    },
  ],
});

// Loop until there are no more computer use actions.
while (true) {
  const computerCalls = res.output.filter((o) => o.type === 'computer_call');
  if (computerCalls.length < 1) {
    console.log('No more computer calls. Loop complete.');
    break;
  }
  // Get the first call
  const call = computerCalls[0];
  const action = call.action;
  console.log('Received action from OpenAI Responses API:', action);
  let ackChecks: OpenAI.Responses.ResponseComputerToolCall.PendingSafetyCheck[] =
    [];
  if (call.pending_safety_checks.length > 0) {
    console.log('Safety checks pending:', call.pending_safety_checks);
    // In a real implementation, you would want to get user confirmation here.
    ackChecks = call.pending_safety_checks;
  }

  // Execute the action in the container
  await executeAction(computer, action);
  // Wait for changes to process within the container (1sec)
  await new Promise((resolve) => setTimeout(resolve, 1000));

  // Capture new screenshot
  const newScreenshot = await computer.interface.screenshot();
  const newScreenshotBase64 = newScreenshot.toString('base64');

  // Screenshot back as computer_call_output

  res = await openai.responses.create({
    ...computerUseConfig,
    previous_response_id: res.id,
    input: [
      {
        type: 'computer_call_output',
        call_id: call.call_id,
        acknowledged_safety_checks: ackChecks,
        output: {
          type: 'computer_screenshot',
          image_url: `data:image/png;base64,${newScreenshotBase64}`,
        },
      },
    ],
  });
}
```

You can find the full example on [GitHub](https://github.com/trycua/cua/tree/main/examples/computer-example-ts).

## What's Next?

The `@trycua/computer` Web SDK opens up some interesting possibilities. You could build browser-based testing tools, create interactive demos for your products, or automate repetitive workflows directly from your web apps.

We're working on more examples and better documentation - if you build something cool with this SDK, we'd love to see it. Drop by our [Discord](https://discord.gg/cua-ai) and share what you're working on.

Happy automating on the web!
