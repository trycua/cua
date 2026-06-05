#!/usr/bin/env node

import { GoogleGenerativeAI } from '@google/generative-ai';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import * as readline from 'node:readline/promises';
import { stdin as input, stdout as output } from 'node:process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

const GEMINI_API_KEY = process.env.GEMINI_API_KEY;
if (!GEMINI_API_KEY) {
  console.error('Error: GEMINI_API_KEY environment variable not set');
  process.exit(1);
}

const genAI = new GoogleGenerativeAI(GEMINI_API_KEY);
const cuaDriverPath = path.resolve(__dirname, '../../libs/cua-driver/rust/target/release/cua-driver');

console.log('Connecting to cua-driver...');

const transport = new StdioClientTransport({
  command: cuaDriverPath,
  args: ['mcp']
});

const mcpClient = new Client({
  name: 'gemini-computer-use',
  version: '1.0.0'
}, {
  capabilities: { tools: {} }
});

await mcpClient.connect(transport);
console.log('✓ Connected to cua-driver-rs\n');

// Get list of windows
const result = await mcpClient.callTool({
  name: 'list_windows',
  arguments: {}
});

const text = result.content[0]?.text || '';
const windows = [];

for (const line of text.split('\n')) {
  const match = line.match(/\(pid\s+(\d+)\)\s+"([^"]+)"\s+\[window_id:\s+(\d+)\]/);
  if (match) {
    windows.push({
      pid: parseInt(match[1]),
      title: match[2],
      window_id: parseInt(match[3])
    });
  }
}

console.log(`Found ${windows.length} windows\n`);

let selectedWindow = null;
let windowScreenshot = null;

// Get fresh window state and screenshot
async function refreshWindowState() {
  if (!selectedWindow) return null;

  const result = await mcpClient.callTool({
    name: 'get_window_state',
    arguments: {
      pid: selectedWindow.pid,
      window_id: selectedWindow.window_id,
      capture_mode: 'som'
    }
  });

  // Screenshot is in content array as image type
  const screenshotContent = result.content.find(c => c.type === 'image');
  const screenshot = screenshotContent ? screenshotContent.data : null;

  // Get dimensions from structured content
  const structured = result.structuredContent || {};
  windowScreenshot = {
    base64: screenshot,
    width: structured.screenshot_width || 1920,
    height: structured.screenshot_height || 1080
  };

  return windowScreenshot;
}

// Convert Gemini normalized coords (0-999) to window-relative pixels
function normalizedToPixel(normalizedX, normalizedY) {
  if (!windowScreenshot) {
    throw new Error('No window screenshot available');
  }

  const pixelX = Math.round((normalizedX / 999) * windowScreenshot.width);
  const pixelY = Math.round((normalizedY / 999) * windowScreenshot.height);

  return { x: pixelX, y: pixelY };
}

// Handle Gemini Computer Use function calls
async function handleComputerAction(functionCall) {
  const { name: action, args } = functionCall;

  if (!selectedWindow) {
    return { error: 'No window selected' };
  }

  const { pid, window_id } = selectedWindow;

  try {
    switch (action) {
      case 'take_screenshot': {
        await refreshWindowState();
        return {
          screenshot: windowScreenshot.base64,
          width: windowScreenshot.width,
          height: windowScreenshot.height
        };
      }

      case 'click':
      case 'double_click':
      case 'triple_click':
      case 'middle_click':
      case 'right_click': {
        const { x, y } = normalizedToPixel(args.x, args.y);
        const button = action === 'right_click' ? 'right' :
                      action === 'middle_click' ? 'middle' : 'left';
        const count = action === 'double_click' ? 2 :
                     action === 'triple_click' ? 3 : 1;

        await mcpClient.callTool({
          name: 'click',
          arguments: { pid, window_id, x, y, button, count }
        });

        return { success: true, action, pixel: { x, y } };
      }

      case 'type': {
        await mcpClient.callTool({
          name: 'type_text',
          arguments: { pid, window_id, text: args.text }
        });

        return { success: true, typed: args.text };
      }

      case 'press_key':
      case 'key_down':
      case 'key_up': {
        await mcpClient.callTool({
          name: 'press_key',
          arguments: { pid, window_id, key: args.key }
        });

        return { success: true, key: args.key };
      }

      case 'hotkey': {
        const modifiers = args.keys.slice(0, -1);
        const key = args.keys[args.keys.length - 1];

        await mcpClient.callTool({
          name: 'hotkey',
          arguments: { pid, window_id, key, modifiers }
        });

        return { success: true, keys: args.keys };
      }

      case 'scroll': {
        const { x, y } = normalizedToPixel(args.x, args.y);
        const magnitude = args.magnitude_in_pixels || 300;

        await mcpClient.callTool({
          name: 'scroll',
          arguments: {
            pid, window_id, x, y,
            dy: args.direction === 'up' ? -magnitude :
                args.direction === 'down' ? magnitude : 0,
            dx: args.direction === 'left' ? -magnitude :
                args.direction === 'right' ? magnitude : 0
          }
        });

        return { success: true, direction: args.direction };
      }

      case 'drag_and_drop': {
        const start = normalizedToPixel(args.start_x, args.start_y);
        const end = normalizedToPixel(args.end_x, args.end_y);

        await mcpClient.callTool({
          name: 'drag',
          arguments: {
            pid, window_id,
            from_x: start.x, from_y: start.y,
            to_x: end.x, to_y: end.y
          }
        });

        return { success: true, from: start, to: end };
      }

      case 'wait': {
        const ms = (args.seconds || 1) * 1000;
        await new Promise(resolve => setTimeout(resolve, ms));
        return { success: true, waited_ms: ms };
      }

      default:
        return { error: `Unsupported action: ${action}` };
    }
  } catch (error) {
    return { error: error.message, action };
  }
}

// Autocomplete function for @ window selection (works anywhere in line)
function completer(line) {
  const atIndex = line.lastIndexOf('@');
  if (atIndex === -1) {
    return [[], line];
  }

  const beforeAt = line.slice(0, atIndex);
  let afterAt = line.slice(atIndex + 1);

  // If there's a quote, extract content between quotes
  let query = '';
  let hasQuote = false;
  if (afterAt.startsWith('"')) {
    hasQuote = true;
    const closeQuote = afterAt.indexOf('"', 1);
    query = closeQuote > 0 ? afterAt.slice(1, closeQuote) : afterAt.slice(1);
  } else {
    // No quote, take until space or end
    const spaceIndex = afterAt.indexOf(' ');
    query = spaceIndex > 0 ? afterAt.slice(0, spaceIndex) : afterAt;
  }

  query = query.toLowerCase();

  // Build completions
  const completions = windows
    .filter(w => w.title.toLowerCase().includes(query))
    .map(w => {
      const title = w.title.includes(' ') ? `"${w.title}"` : w.title;
      return `${beforeAt}@${title}`;
    });

  // If only one match, return it; otherwise return all matches
  if (completions.length === 1) {
    return [completions, line];
  } else if (completions.length > 1) {
    return [completions, line];
  } else {
    // No matches, show all windows
    const allCompletions = windows.map(w => {
      const title = w.title.includes(' ') ? `"${w.title}"` : w.title;
      return `${beforeAt}@${title}`;
    });
    return [allCompletions, line];
  }
}

// Interactive loop
const rl = readline.createInterface({
  input,
  output,
  completer,
  prompt: ''
});

console.log('🤖 Gemini 3.5 Flash - Desktop Computer Use');
console.log('Type @window-name to select a window (Tab to autocomplete), then ask Gemini to control it');
console.log('Commands: /export - export current trajectory\n');

while (true) {
  const prompt = selectedWindow
    ? `[${selectedWindow.title}] > `
    : '> ';

  let userInput = await rl.question(prompt);

  if (userInput.toLowerCase() === 'exit') {
    break;
  }

  // Handle /export command
  if (userInput.toLowerCase() === '/export') {
    if (!selectedWindow) {
      console.log('No session to export. Select a window first.\n');
      continue;
    }
    try {
      const { trajectory } = await import('./gemini-client.js');
      await trajectory.exportMinimalRepro();
    } catch (error) {
      console.error(`Export failed: ${error.message}\n`);
    }
    continue;
  }

  // Handle @window selection (can be anywhere in the line)
  const atMatch = userInput.match(/@"([^"]+)"|@(\S+)/);
  if (atMatch && !selectedWindow) {
    // Only do window selection if we don't have a window selected yet
    const query = (atMatch[1] || atMatch[2]).toLowerCase();
    const matches = windows.filter(w =>
      w.title.toLowerCase().includes(query)
    );

    if (matches.length === 1) {
      selectedWindow = matches[0];
      console.log(`✓ Selected: ${selectedWindow.title}`);

      // Capture initial screenshot
      try {
        await refreshWindowState();
        console.log(`📸 Screenshot captured (${windowScreenshot.width}×${windowScreenshot.height})\n`);
      } catch (error) {
        console.error(`Failed to capture screenshot: ${error.message}\n`);
      }

      // If there's text before/after the @window, process it as a command
      const commandText = userInput.replace(/@"[^"]+"|@\S+/, '').trim();
      if (commandText) {
        userInput = commandText; // Continue processing the command
      } else {
        continue; // Just window selection, get next input
      }
    } else if (matches.length > 1) {
      console.log('Multiple matches:');
      matches.forEach(w => console.log(`  @${w.title.includes(' ') ? `"${w.title}"` : w.title}`));
      console.log();
      continue;
    } else {
      console.log('No matches. Available windows:');
      windows.forEach(w => console.log(`  @${w.title.includes(' ') ? `"${w.title}"` : w.title}`));
      console.log();
      continue;
    }
  }

  if (!selectedWindow) {
    console.log('Please select a window first using @window-name\n');
    continue;
  }

  // Process with Gemini
  try {
    const model = genAI.getGenerativeModel({
      model: 'gemini-3.5-flash',
      tools: [{
        functionDeclarations: [
          {
            name: 'take_screenshot',
            description: 'Capture a screenshot of the selected window',
            parameters: {
              type: 'object',
              properties: {
                intent: { type: 'string', description: 'Reason for taking screenshot' }
              },
              required: ['intent']
            }
          },
          {
            name: 'click',
            description: 'Click at normalized coordinates (0-999)',
            parameters: {
              type: 'object',
              properties: {
                x: { type: 'integer', minimum: 0, maximum: 999 },
                y: { type: 'integer', minimum: 0, maximum: 999 },
                intent: { type: 'string' }
              },
              required: ['x', 'y', 'intent']
            }
          },
          {
            name: 'double_click',
            description: 'Double click at normalized coordinates',
            parameters: {
              type: 'object',
              properties: {
                x: { type: 'integer', minimum: 0, maximum: 999 },
                y: { type: 'integer', minimum: 0, maximum: 999 },
                intent: { type: 'string' }
              },
              required: ['x', 'y', 'intent']
            }
          },
          {
            name: 'type',
            description: 'Type text',
            parameters: {
              type: 'object',
              properties: {
                text: { type: 'string' },
                intent: { type: 'string' }
              },
              required: ['text', 'intent']
            }
          },
          {
            name: 'press_key',
            description: 'Press a key',
            parameters: {
              type: 'object',
              properties: {
                key: { type: 'string' },
                intent: { type: 'string' }
              },
              required: ['key', 'intent']
            }
          },
          {
            name: 'scroll',
            description: 'Scroll in a direction',
            parameters: {
              type: 'object',
              properties: {
                x: { type: 'integer', minimum: 0, maximum: 999 },
                y: { type: 'integer', minimum: 0, maximum: 999 },
                direction: { type: 'string', enum: ['up', 'down', 'left', 'right'] },
                magnitude_in_pixels: { type: 'integer' },
                intent: { type: 'string' }
              },
              required: ['x', 'y', 'direction', 'intent']
            }
          }
        ]
      }]
    });

    const chat = model.startChat();
    let result = await chat.sendMessage(userInput);
    let response = result.response;

    // Handle function calls
    let functionCalls = response.functionCalls?.() || [];
    while (functionCalls.length > 0) {
      console.log(`\n🔧 Calling: ${functionCalls.map(fc => fc.name).join(', ')}`);

      const functionResponses = [];

      for (const call of functionCalls) {
        const actionResult = await handleComputerAction(call);

        functionResponses.push({
          functionResponse: {
            name: call.name,
            response: actionResult
          }
        });
      }

      result = await chat.sendMessage(functionResponses);
      response = result.response;
      functionCalls = response.functionCalls?.() || [];
    }

    const text = response.text();
    console.log(`\nGemini: ${text}\n`);

  } catch (error) {
    console.error('Error:', error.message, '\n');
  }
}

console.log('\nGoodbye!');
await mcpClient.close();
rl.close();
