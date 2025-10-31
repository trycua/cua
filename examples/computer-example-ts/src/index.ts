import { Computer, OSType } from '@trycua/computer';
import OpenAI from 'openai';
import { executeAction } from './helpers';

import 'dotenv/config';

const openai = new OpenAI({ apiKey: process.env.OPENAI_API_KEY });

const COMPUTER_USE_PROMPT = 'Open firefox and go to cua.ai';

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
        { type: 'input_text', text: COMPUTER_USE_PROMPT },
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
  let ackChecks: OpenAI.Responses.ResponseComputerToolCall.PendingSafetyCheck[] = [];
  if (call.pending_safety_checks.length > 0) {
    console.log('Safety checks pending:', call.pending_safety_checks);
    // In a real implementation, you would want to get user confirmation here
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

process.exit();
