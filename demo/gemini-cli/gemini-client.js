// Gemini + cua-driver integration layer
import { GoogleGenAI } from '@google/genai';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import sharp from 'sharp';
import { TrajectoryRecorder } from './trajectory.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

const GEMINI_API_KEY = process.env.GEMINI_API_KEY;
if (!GEMINI_API_KEY) {
  console.error('Error: GEMINI_API_KEY environment variable not set');
  process.exit(1);
}

const client = new GoogleGenAI({ apiKey: GEMINI_API_KEY });
const cuaDriverPath = path.resolve(__dirname, '../../libs/cua-driver/rust/target/release/cua-driver');

let mcpClient = null;
let selectedWindow = null;
let windowScreenshot = null;
const SESSION_KEY = `gemini-${Date.now()}`;

export const trajectory = new TrajectoryRecorder(__dirname);

async function convertToJpeg(base64Png) {
  // Remove data URL prefix if present
  const base64Data = base64Png.split(',')[1] || base64Png;
  const pngBuffer = Buffer.from(base64Data, 'base64');

  // Convert PNG to JPEG with maximum quality settings
  const jpegBuffer = await sharp(pngBuffer)
    .jpeg({
      quality: 95,                    // High quality (95 recommended for best size/quality ratio)
      chromaSubsampling: '4:4:4',     // Preserve full color detail
      progressive: false,             // Sequential for faster decoding
      mozjpeg: false,                 // Standard libjpeg for compatibility
      trellisQuantisation: true,      // Better quality preservation
      overshootDeringing: true,       // Reduce artifacts
      optimiseCoding: true            // Optimize Huffman tables
    })
    .toBuffer();

  return jpegBuffer.toString('base64');
}

export async function initCuaDriver() {
  const transport = new StdioClientTransport({
    command: cuaDriverPath,
    args: ['mcp']
  });

  mcpClient = new Client({
    name: 'gemini-computer-use',
    version: '1.0.0'
  }, {
    capabilities: { tools: {} }
  });

  await mcpClient.connect(transport);

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

  return windows;
}

export async function selectWindow(window) {
  selectedWindow = window;

  // Start trajectory session
  const sessionId = `session_${Date.now()}`;
  await trajectory.startSession(sessionId, {
    pid: window.pid,
    window_id: window.window_id,
    title: window.title
  });

  // Capture initial screenshot
  const result = await mcpClient.callTool({
    name: 'get_window_state',
    arguments: {
      pid: window.pid,
      window_id: window.window_id,
      capture_mode: 'som'
    }
  });

  const screenshotContent = result.content.find(c => c.type === 'image');
  const screenshot = screenshotContent ? screenshotContent.data : null;
  const structured = result.structuredContent || {};

  windowScreenshot = {
    base64: screenshot,
    width: structured.screenshot_width || 1920,
    height: structured.screenshot_height || 1080
  };
}

export function getSelectedWindow() {
  return selectedWindow;
}

function normalizedToPixel(normalizedX, normalizedY) {
  if (!windowScreenshot) {
    throw new Error('No screenshot available');
  }

  const pixelX = Math.round((normalizedX / 999) * windowScreenshot.width);
  const pixelY = Math.round((normalizedY / 999) * windowScreenshot.height);

  return { x: pixelX, y: pixelY };
}

async function executeAction(functionCall) {
  const { name: action, args } = functionCall;

  if (!selectedWindow) {
    return { error: 'No window selected' };
  }

  const { pid, window_id } = selectedWindow;

  try {
    switch (action) {
      case 'take_screenshot': {
        // Capture fresh screenshot
        const result = await mcpClient.callTool({
          name: 'get_window_state',
          arguments: {
            pid,
            window_id,
            capture_mode: 'som'
          }
        });

        const screenshotContent = result.content.find(c => c.type === 'image');
        const screenshot = screenshotContent ? screenshotContent.data : null;
        const structured = result.structuredContent || {};

        // Update global screenshot cache with fresh data
        windowScreenshot = {
          base64: screenshot,
          width: structured.screenshot_width || 1920,
          height: structured.screenshot_height || 1080
        };

        // Return image data directly for function response
        const imageData = await convertToJpeg(screenshot);
        return {
          inlineData: {
            mimeType: 'image/jpeg',
            data: imageData
          }
        };
      }

      case 'click': {
        const { x, y } = normalizedToPixel(args.x, args.y);
        await mcpClient.callTool({
          name: 'click',
          arguments: {
            pid,
            window_id,
            x,
            y,
            button: 'left',
            count: 1,
            session: SESSION_KEY
          }
        });
        return { success: true };
      }

      case 'type': {
        // Append \n if press_enter is true
        const text = args.press_enter ? args.text + '\n' : args.text;
        await mcpClient.callTool({
          name: 'type_text',
          arguments: {
            pid,
            window_id,
            text,
            session: SESSION_KEY
          }
        });
        return { success: true };
      }

      default:
        return { error: `Unsupported action: ${action}` };
    }
  } catch (error) {
    return { error: error.message };
  }
}

async function callWithRetry(params, maxRetries = 10) {
  let attempt = 0;
  while (attempt < maxRetries) {
    try {
      return await client.models.generateContent(params);
    } catch (error) {
      const errorData = error.message ? JSON.parse(error.message) : error;
      const retryInfo = errorData.error?.details?.find(d => d['@type']?.includes('RetryInfo'));

      if (retryInfo?.retryDelay && attempt < maxRetries - 1) {
        const delayMs = parseFloat(retryInfo.retryDelay.replace('s', '')) * 1000;
        const delaySec = Math.ceil(delayMs / 1000);
        console.log(`\n  ⎿  Retrying in ${delaySec}s · attempt ${attempt + 1}/${maxRetries}`);
        await new Promise(resolve => setTimeout(resolve, delayMs));
        attempt++;
        continue;
      }
      throw error;
    }
  }
}

export async function handleComputerAction(userCommand, onToolCall) {
  // Use Computer Use EAP API
  const { Environment } = await import('@google/genai');

  // Send initial screenshot with user command (convert to JPEG to reduce token usage)
  const imageData = await convertToJpeg(windowScreenshot.base64);
  const initialParts = [
    {
      inlineData: {
        mimeType: 'image/jpeg',
        data: imageData
      }
    },
    { text: userCommand }
  ];

  const initialContent = {
    role: 'user',
    parts: initialParts
  };

  const config = {
    tools: [{
      computerUse: {
        environment: Environment.ENVIRONMENT_DESKTOP
      }
    }]
  };

  let response = await callWithRetry({
    model: 'gemini-3.5-flash',
    contents: [initialContent],
    config
  });

  // Record user message
  await trajectory.recordContent(initialContent);

  const allFunctionCalls = [];
  const conversation = [initialContent];

  // Check if response has function calls
  let modelContent = response.candidates?.[0]?.content;

  while (modelContent?.parts?.some(p => p.functionCall)) {
    // Append the model's content directly to conversation
    conversation.push(modelContent);

    // Record model's content
    await trajectory.recordContent(modelContent);

    // Extract function calls for execution
    const functionCalls = modelContent.parts
      .filter(p => p.functionCall)
      .map(p => p.functionCall);

    // Execute function calls and build response parts
    const functionResponseParts = await Promise.all(functionCalls.map(async (call) => {
      // Stream tool call as it happens
      if (onToolCall) {
        onToolCall({ name: call.name, args: call.args });
      }
      allFunctionCalls.push({ name: call.name, args: call.args });

      // Execute and get result
      const actionResult = await executeAction(call);

      // Return the function response Part directly
      return {
        functionResponse: {
          name: call.name,
          response: actionResult
        }
      };
    }));

    // Append user's function response content to conversation
    const functionResponseContent = {
      role: 'user',
      parts: functionResponseParts
    };
    conversation.push(functionResponseContent);

    // Record function response content
    await trajectory.recordContent(functionResponseContent);

    // Get next response
    response = await callWithRetry({
      model: 'gemini-3.5-flash',
      contents: conversation,
      config
    });

    modelContent = response.candidates?.[0]?.content;
  }

  return {
    text: response.text || '',
    functionCalls: allFunctionCalls
  };
}
