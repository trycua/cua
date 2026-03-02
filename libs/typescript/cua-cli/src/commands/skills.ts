import type { Argv } from 'yargs';
import { ensureApiKeyInteractive } from '../auth';
import { http } from '../http';
import { clearApiKey } from '../storage';
import type { SandboxItem } from '../util';
import { openInBrowser } from '../util';
import * as readline from 'readline';

const SKILLS_DIR = `${process.env.HOME || process.env.USERPROFILE}/.cua/skills`;

// Helper to ensure skills directory exists
async function ensureSkillsDir(): Promise<void> {
  const fs = await import('fs/promises');
  try {
    await fs.mkdir(SKILLS_DIR, { recursive: true });
  } catch {
    // Directory might already exist
  }
}

// Helper to prompt for user input
function prompt(question: string): Promise<string> {
  const rl = readline.createInterface({
    input: process.stdin,
    output: process.stdout,
  });

  return new Promise((resolve) => {
    rl.question(question, (answer) => {
      rl.close();
      resolve(answer.trim());
    });
  });
}

// Helper to get sandbox VNC URL
async function getSandboxVncUrl(
  name: string,
  token: string
): Promise<{ url: string; host: string; password: string }> {
  const listRes = await http('/v1/vms', { token });
  if (listRes.status === 401) {
    clearApiKey();
    console.error("Unauthorized. Try 'cua login' again.");
    process.exit(1);
  }
  if (!listRes.ok) {
    console.error(`Request failed: ${listRes.status}`);
    process.exit(1);
  }

  const sandboxes = (await listRes.json()) as SandboxItem[];
  const sandbox = sandboxes.find((s) => s.name === name);

  if (!sandbox) {
    console.error('Sandbox not found');
    process.exit(1);
  }

  if (sandbox.status !== 'running') {
    console.error(`Sandbox is not running (status: ${sandbox.status})`);
    process.exit(1);
  }

  const host = sandbox.host || `${sandbox.name}.sandbox.cua.ai`;
  const url = `https://${host}/vnc.html?autoconnect=true&password=${encodeURIComponent(sandbox.password)}&show_dot=true`;

  return { url, host, password: sandbox.password };
}

// Recording format: JSON with events + optional MP4 video
// Format sent over WebSocket:
//   [4 bytes JSON length (big-endian)][JSON data][optional MP4 data]
// JSON structure: { events: InputEvent[], metadata: {...} }

interface InputEvent {
  timestamp: number;
  type: string; // 'click', 'type', 'key', 'drag', 'scroll', 'paste'
  data: Record<string, unknown>;
}

interface RecordingData {
  events: InputEvent[];
  metadata?: {
    width?: number;
    height?: number;
    duration?: number;
    startTime?: number;
  };
}

interface TrajectoryStep {
  step_idx: number;
  caption: {
    observation: string;
    think: string;
    action: string;
    expectation: string;
  };
  raw_event: {
    timestamp: number;
    type: string;
    data: Record<string, unknown>;
  };
  screenshot_full?: string;
  screenshot_crop?: string;
}

interface Skill {
  name: string;
  description: string;
  trajectory: TrajectoryStep[];
  skill_prompt?: string;
  video_path?: string;
  metadata?: {
    recording_file?: string;
    task_description?: string;
    total_steps?: number;
    width?: number;
    height?: number;
    duration?: number;
    created_at?: string;
  };
}

// Default prompt for trajectory captioning (adapted from Aloha_Learn)
const DEFAULT_PROMPT = {
  basePrompt: `Objective:
You will describe a single GUI action using two images per step:
1) a ZOOMED-IN CROP (primary focus), and
2) a FULL-SCREEN image providing context.

Crop semantics:
• Click / Select → the crop has a red "X" marker centered on the target control. This marker is an annotation overlay—ignore it visually and describe only the UI element underneath.
• Scroll/Hotkey/Type → no marker; the crop frames the entire screen.
• Drag → the crop has a red line/path showing the drag route. This is an annotation—describe only the UI elements at the start and end points.

High-Priority Rules:
• NEVER mention the red X, red marker, red path, or any annotation overlay in your output. These are visual guides only—describe the actual UI elements they point to.
• Read the crop first, then the full screen.
• Never include coordinates, file paths, or timestamps.
• Observation must describe what is visually present—icons, text, glyphs, color, shape—and must be complete.
• Infer the actual name of the icon if possible. Describe the icon such that an operator with no prior knowledge can identify and select it correctly.
• For clicks: identify the UI element at the center of the crop (where the marker points).
• For drags: describe the start element, direction, and destination element.
• Respond strictly in JSON; no extra prose.

Output Fields:
{
  "Observation": "Describe the UI element being targeted, then relevant full-screen context. Do NOT mention any red markers or overlays.",
  "Think": "Explain the user's likely intention based on Observation.",
  "Action": "Concrete operation: click / type / drag, etc.",
  "Expectation": "What should happen immediately after the action."
}`,

  actionDeltas: {
    click:
      'Identify the UI control at the center of the crop (toolbar button, menu item, canvas object, text field, etc.). Do not mention any visual markers.',
    type: 'Describe the visible text entry area and what is being typed. The operation is typing, not clicking.',
    drag: 'Describe the start element, direction, and destination element. Action must be: "Click-and-hold on <start>, drag to <destination>, then release."',
    scroll:
      'Determine direction and purpose. Up usually moves view up; down moves view down.',
    key: 'Describe the special key being pressed and its likely purpose.',
  },
};

// Parse recording data (JSON header + optional MP4)
function parseRecordingData(data: Uint8Array): {
  recording: RecordingData;
  mp4Data?: Uint8Array;
} {
  if (data.length < 4) {
    throw new Error('Recording data too short');
  }

  // Read JSON length (4 bytes, big-endian)
  const jsonLength =
    ((data[0] ?? 0) << 24) |
    ((data[1] ?? 0) << 16) |
    ((data[2] ?? 0) << 8) |
    (data[3] ?? 0);

  if (data.length < 4 + jsonLength) {
    throw new Error(
      `Invalid recording: expected ${jsonLength} bytes of JSON, got ${data.length - 4}`
    );
  }

  // Parse JSON
  const jsonBytes = data.slice(4, 4 + jsonLength);
  const jsonStr = new TextDecoder().decode(jsonBytes);
  const recording = JSON.parse(jsonStr) as RecordingData;

  // Extract MP4 data if present
  let mp4Data: Uint8Array | undefined;
  if (data.length > 4 + jsonLength) {
    mp4Data = data.slice(4 + jsonLength);
  }

  return { recording, mp4Data };
}

// Extract a frame from MP4 video at a specific timestamp using ffmpeg
async function extractFrame(
  videoPath: string,
  timestampMs: number,
  outputPath: string
): Promise<boolean> {
  const timestampSec = Math.max(0, timestampMs / 1000 - 0.1); // Slightly before the action
  const proc = Bun.spawn({
    cmd: [
      'ffmpeg',
      '-y',
      '-ss',
      timestampSec.toFixed(3),
      '-i',
      videoPath,
      '-frames:v',
      '1',
      '-q:v',
      '2',
      outputPath,
    ],
    stdout: 'ignore',
    stderr: 'ignore',
  });
  const exitCode = await proc.exited;
  return exitCode === 0;
}

// Create a cropped screenshot with optional red X marker
async function createCropWithMarker(
  fullImagePath: string,
  cropPath: string,
  x: number | undefined,
  y: number | undefined,
  cropSize: number = 256,
  actionType: string = 'click'
): Promise<boolean> {
  // For types without coordinates, just copy the full image
  if (
    x === undefined ||
    y === undefined ||
    actionType === 'type' ||
    actionType === 'key' ||
    actionType === 'scroll'
  ) {
    const proc = Bun.spawn({
      cmd: ['cp', fullImagePath, cropPath],
      stdout: 'ignore',
      stderr: 'ignore',
    });
    return (await proc.exited) === 0;
  }

  // Use ImageMagick to create crop with red X marker
  const halfSize = Math.floor(cropSize / 2);
  const xSize = 15; // Half-length of X arms
  const thickness = 3;

  // Create crop centered on x,y with red X overlay
  const proc = Bun.spawn({
    cmd: [
      'convert',
      fullImagePath,
      // Crop region
      '-crop',
      `${cropSize}x${cropSize}+${Math.max(0, x - halfSize)}+${Math.max(0, y - halfSize)}`,
      '+repage',
      // Draw red X at center
      '-fill',
      'none',
      '-stroke',
      'red',
      '-strokewidth',
      String(thickness),
      '-draw',
      `line ${halfSize - xSize},${halfSize - xSize} ${halfSize + xSize},${halfSize + xSize}`,
      '-draw',
      `line ${halfSize - xSize},${halfSize + xSize} ${halfSize + xSize},${halfSize - xSize}`,
      // Add semi-transparency to the X
      cropPath,
    ],
    stdout: 'ignore',
    stderr: 'ignore',
  });

  return (await proc.exited) === 0;
}

// Progress bar helper
function renderProgressBar(
  currentStep: number,
  totalSteps: number,
  tokens: number,
  barWidth: number = 20
): string {
  const progress = currentStep / totalSteps;
  const filled = Math.round(progress * barWidth);
  const empty = barWidth - filled;
  const bar = '█'.repeat(filled) + '░'.repeat(empty);
  const tokenStr = tokens.toLocaleString();
  return `[${bar}] ${currentStep}/${totalSteps} │ ${tokenStr} tokens`;
}

// Streaming callback type
type TokenCallback = (tokens: number) => void;

// Call Anthropic API for captioning with streaming
async function callAnthropic(
  prompt: string,
  cropImagePath: string | null,
  fullImagePath: string | null,
  model: string,
  apiKey: string,
  onToken?: TokenCallback
): Promise<string> {
  const content: Array<{
    type: string;
    text?: string;
    source?: { type: string; media_type: string; data: string };
  }> = [{ type: 'text', text: prompt }];

  // Add cropped image first (primary focus)
  if (cropImagePath) {
    try {
      const cropData = await Bun.file(cropImagePath).arrayBuffer();
      content.push({
        type: 'image',
        source: {
          type: 'base64',
          media_type: 'image/jpeg',
          data: Buffer.from(cropData).toString('base64'),
        },
      });
    } catch {
      // Skip if image doesn't exist
    }
  }

  // Add full screenshot for context
  if (fullImagePath) {
    try {
      const fullData = await Bun.file(fullImagePath).arrayBuffer();
      content.push({
        type: 'image',
        source: {
          type: 'base64',
          media_type: 'image/jpeg',
          data: Buffer.from(fullData).toString('base64'),
        },
      });
    } catch {
      // Skip if image doesn't exist
    }
  }

  const response = await fetch('https://api.anthropic.com/v1/messages', {
    method: 'POST',
    headers: {
      'x-api-key': apiKey,
      'anthropic-version': '2023-06-01',
      'content-type': 'application/json',
    },
    body: JSON.stringify({
      model,
      max_tokens: 1200,
      stream: true,
      messages: [{ role: 'user', content }],
    }),
  });

  if (!response.ok) {
    const error = await response.text();
    throw new Error(`Anthropic API error: ${response.status} - ${error}`);
  }

  // Process SSE stream
  const reader = response.body?.getReader();
  if (!reader) {
    throw new Error('No response body');
  }

  const decoder = new TextDecoder();
  let fullText = '';
  let tokenCount = 0;
  let buffer = '';

  while (true) {
    const { done, value } = await reader.read();
    if (done) break;

    buffer += decoder.decode(value, { stream: true });
    const lines = buffer.split('\n');
    buffer = lines.pop() || '';

    for (const line of lines) {
      if (!line.startsWith('data: ')) continue;
      const data = line.slice(6);
      if (data === '[DONE]') continue;

      try {
        const event = JSON.parse(data);
        if (event.type === 'content_block_delta' && event.delta?.text) {
          fullText += event.delta.text;
          // Estimate tokens (roughly 4 chars per token)
          const newTokens = Math.ceil(event.delta.text.length / 4);
          tokenCount += newTokens;
          onToken?.(tokenCount);
        } else if (
          event.type === 'message_delta' &&
          event.usage?.output_tokens
        ) {
          // Use actual token count if available
          tokenCount = event.usage.output_tokens;
          onToken?.(tokenCount);
        }
      } catch {
        // Ignore parse errors
      }
    }
  }

  return fullText;
}

// Call OpenAI API for captioning with streaming
async function callOpenAI(
  prompt: string,
  cropImagePath: string | null,
  fullImagePath: string | null,
  model: string,
  apiKey: string,
  onToken?: TokenCallback
): Promise<string> {
  const content: Array<{
    type: string;
    text?: string;
    image_url?: { url: string };
  }> = [{ type: 'text', text: prompt }];

  // Add cropped image first (primary focus)
  if (cropImagePath) {
    try {
      const cropData = await Bun.file(cropImagePath).arrayBuffer();
      content.push({
        type: 'image_url',
        image_url: {
          url: `data:image/jpeg;base64,${Buffer.from(cropData).toString('base64')}`,
        },
      });
    } catch {
      // Skip if image doesn't exist
    }
  }

  // Add full screenshot for context
  if (fullImagePath) {
    try {
      const fullData = await Bun.file(fullImagePath).arrayBuffer();
      content.push({
        type: 'image_url',
        image_url: {
          url: `data:image/jpeg;base64,${Buffer.from(fullData).toString('base64')}`,
        },
      });
    } catch {
      // Skip if image doesn't exist
    }
  }

  const response = await fetch('https://api.openai.com/v1/chat/completions', {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${apiKey}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({
      model,
      messages: [{ role: 'user', content }],
      temperature: 0.2,
      stream: true,
    }),
  });

  if (!response.ok) {
    const error = await response.text();
    throw new Error(`OpenAI API error: ${response.status} - ${error}`);
  }

  // Process SSE stream
  const reader = response.body?.getReader();
  if (!reader) {
    throw new Error('No response body');
  }

  const decoder = new TextDecoder();
  let fullText = '';
  let tokenCount = 0;
  let buffer = '';

  while (true) {
    const { done, value } = await reader.read();
    if (done) break;

    buffer += decoder.decode(value, { stream: true });
    const lines = buffer.split('\n');
    buffer = lines.pop() || '';

    for (const line of lines) {
      if (!line.startsWith('data: ')) continue;
      const data = line.slice(6);
      if (data === '[DONE]') continue;

      try {
        const event = JSON.parse(data);
        const delta = event.choices?.[0]?.delta?.content;
        if (delta) {
          fullText += delta;
          // Estimate tokens (roughly 4 chars per token)
          const newTokens = Math.ceil(delta.length / 4);
          tokenCount += newTokens;
          onToken?.(tokenCount);
        }
      } catch {
        // Ignore parse errors
      }
    }
  }

  return fullText;
}

// Extract JSON from LLM response
function extractJson(text: string): Record<string, string> {
  if (!text) return {};

  // Try to find JSON block
  const jsonMatch = text.match(/\{[\s\S]*\}/);
  if (jsonMatch) {
    try {
      return JSON.parse(jsonMatch[0]);
    } catch {
      // Continue to fallback
    }
  }

  // Fallback: try parsing the whole text
  try {
    return JSON.parse(text);
  } catch {
    return {};
  }
}

// Sanitize caption output
function sanitizeCaption(cap: Record<string, string>): {
  observation: string;
  think: string;
  action: string;
  expectation: string;
} {
  // Remove coordinates from output
  const scrub = (s: string): string => {
    if (!s) return '';
    return s
      .replace(/\bcoordinates?\b[^.]*\[[^\]]*\]/gi, '')
      .replace(/\[\s*\d+\s*,\s*\d+\s*\]/g, '')
      .replace(/\b(x\s*[:=]?\s*\d+|y\s*[:=]?\s*\d+)/gi, '')
      .replace(/\s{2,}/g, ' ')
      .trim();
  };

  let observation = scrub(cap.Observation || cap.observation || '');
  // Ensure observation starts with "Cropped image shows"
  if (
    observation &&
    !observation.toLowerCase().startsWith('cropped image shows')
  ) {
    observation = 'Cropped image shows ' + observation;
  }

  return {
    observation,
    think: scrub(cap.Think || cap.think || ''),
    action: scrub(cap.Action || cap.action || ''),
    expectation: scrub(cap.Expectation || cap.expectation || ''),
  };
}

// Build prompt for a step
function buildStepPrompt(
  event: InputEvent,
  stepIdx: number,
  recentSteps: Array<{ step_idx: number; caption: Record<string, string> }>,
  taskDescription: string
): string {
  const actionType = event.type.toLowerCase();
  const delta =
    DEFAULT_PROMPT.actionDeltas[
      actionType as keyof typeof DEFAULT_PROMPT.actionDeltas
    ] || '';

  const recentContext = recentSteps.slice(-3).map((s) => ({
    step_idx: s.step_idx,
    Observation: s.caption.observation || '',
    Think: s.caption.think || '',
    Action: s.caption.action || '',
    Expectation: s.caption.expectation || '',
  }));

  return `${DEFAULT_PROMPT.basePrompt}

Recent Steps (most recent first, up to 3):
${JSON.stringify(recentContext, null, 2)}

Step Index: ${stepIdx}
Action Type: ${event.type}
Action Data: ${JSON.stringify(event.data)}
Timestamp: ${event.timestamp}ms
Overall Task: ${taskDescription}

${delta ? `ActionTypeDelta: ${delta}` : ''}

Respond with JSON only.`;
}

// Process recording with LLM captioning
async function processRecordingWithLLM(
  data: Uint8Array,
  taskDescription: string,
  skillName: string,
  options: {
    provider: 'anthropic' | 'openai';
    model: string;
    apiKey: string;
  }
): Promise<{
  trajectory: TrajectoryStep[];
  skillPrompt: string;
  trajectoryDir: string;
  metadata: Skill['metadata'];
}> {
  const { recording, mp4Data } = parseRecordingData(data);
  const inputEvents = recording.events || [];

  // Create timestamp for trajectory folder (format: YYYYMMDD-HHMMSS)
  const now = new Date();
  const timestamp = now
    .toISOString()
    .replace(/[-:]/g, '')
    .replace('T', '-')
    .slice(0, 15);

  // Save MP4 video and other assets to {skillName}/trajectory/
  const trajectoryDir = `${SKILLS_DIR}/${skillName}/trajectory`;
  const tempDir = `${SKILLS_DIR}/.temp/${skillName}`;
  const fs = await import('fs/promises');
  await fs.mkdir(trajectoryDir, { recursive: true });
  await fs.mkdir(tempDir, { recursive: true });

  if (!mp4Data || mp4Data.length === 0) {
    throw new Error(
      'No video data in recording. Video is required for trajectory captioning.'
    );
  }

  await ensureSkillsDir();
  const videoPath = `${trajectoryDir}/${skillName}.mp4`;
  await Bun.write(videoPath, mp4Data);

  // Save raw events JSON
  const eventsPath = `${trajectoryDir}/events.json`;
  await Bun.write(
    eventsPath,
    JSON.stringify(
      {
        events: inputEvents,
        metadata: recording.metadata,
      },
      null,
      2
    )
  );

  const trajectory: TrajectoryStep[] = [];
  const recentSteps: Array<{
    step_idx: number;
    caption: Record<string, string>;
  }> = [];

  // Process each event
  const totalEvents = inputEvents.length;
  let totalTokens = 0;

  // Helper to update progress display
  const updateProgress = (currentStep: number, tokens: number) => {
    const progressBar = renderProgressBar(currentStep, totalEvents, tokens);
    process.stdout.write(`\r${progressBar}`);
  };

  for (let idx = 0; idx < inputEvents.length; idx++) {
    const event = inputEvents[idx];
    if (!event) continue;
    const stepIdx = idx + 1;

    // Show initial progress for this step
    updateProgress(stepIdx, totalTokens);

    let caption = {
      observation: '',
      think: '',
      action: '',
      expectation: '',
    };

    let screenshotFull: string | undefined;
    let screenshotCrop: string | undefined;

    const fullPath = `${tempDir}/step_${stepIdx}_full.jpg`;
    const cropPath = `${tempDir}/step_${stepIdx}_crop.jpg`;

    // Extract frame at event timestamp
    const extracted = await extractFrame(videoPath, event.timestamp, fullPath);

    if (!extracted) {
      throw new Error(
        `Failed to extract frame for step ${stepIdx} at timestamp ${event.timestamp}ms`
      );
    }

    // Get coordinates if available
    const eventData = event.data as { x?: number; y?: number };
    const x = eventData.x;
    const y = eventData.y;

    // Create cropped version with marker
    await createCropWithMarker(fullPath, cropPath, x, y, 256, event.type);

    // Build prompt and call LLM
    const stepPrompt = buildStepPrompt(
      event,
      stepIdx,
      recentSteps,
      taskDescription
    );

    // Token callback to update progress in real-time
    let stepTokens = 0;
    const onToken: TokenCallback = (tokens: number) => {
      stepTokens = tokens;
      updateProgress(stepIdx, totalTokens + stepTokens);
    };

    let response: string;
    if (options.provider === 'openai') {
      response = await callOpenAI(
        stepPrompt,
        cropPath,
        fullPath,
        options.model,
        options.apiKey,
        onToken
      );
    } else {
      response = await callAnthropic(
        stepPrompt,
        cropPath,
        fullPath,
        options.model,
        options.apiKey,
        onToken
      );
    }

    // Add this step's tokens to total
    totalTokens += stepTokens;

    const parsed = extractJson(response);
    caption = sanitizeCaption(parsed);

    // Copy screenshots to trajectory dir
    try {
      screenshotFull = `${trajectoryDir}/step_${stepIdx}_full.jpg`;
      screenshotCrop = `${trajectoryDir}/step_${stepIdx}_crop.jpg`;
      await fs.copyFile(fullPath, screenshotFull);
      await fs.copyFile(cropPath, screenshotCrop);
    } catch {
      // Ignore screenshot copy errors
    }

    const step: TrajectoryStep = {
      step_idx: stepIdx,
      caption,
      raw_event: {
        timestamp: event.timestamp,
        type: event.type,
        data: event.data,
      },
      screenshot_full: screenshotFull,
      screenshot_crop: screenshotCrop,
    };

    trajectory.push(step);
    recentSteps.push({ step_idx: stepIdx, caption });

    // Small delay to avoid rate limiting
    await new Promise((r) => setTimeout(r, 100));
  }

  // Show completed progress bar
  updateProgress(totalEvents, totalTokens);
  process.stdout.write('\n');

  // Clean up temp directory
  try {
    await fs.rm(tempDir, { recursive: true });
  } catch {
    // Ignore cleanup errors
  }

  // Generate skill prompt
  const stepsText = trajectory
    .map(
      (step) =>
        `Step ${step.step_idx}: ${step.caption.action || step.raw_event.type}`
    )
    .join('\n');

  const actionTypes = [...new Set(trajectory.map((s) => s.raw_event.type))];

  const skillPrompt = `You have been shown a demonstration of how to perform this task:
${taskDescription}

The demonstration consisted of the following steps:
${stepsText}

Follow this workflow pattern, adapting as needed for the current screen state.
Key observations from the demonstration:
- Total steps: ${trajectory.length}
- Main actions: ${actionTypes.join(', ')}

Execute each step while monitoring for expected outcomes before proceeding.`;

  // Save trajectory JSON to trajectory dir
  const trajectoryJsonPath = `${trajectoryDir}/trajectory.json`;
  await Bun.write(
    trajectoryJsonPath,
    JSON.stringify(
      {
        events: inputEvents,
        trajectory,
        metadata: {
          task_description: taskDescription,
          total_steps: trajectory.length,
          width: recording.metadata?.width,
          height: recording.metadata?.height,
          duration: recording.metadata?.duration,
          created_at: new Date().toISOString(),
        },
      },
      null,
      2
    )
  );

  return {
    trajectory,
    skillPrompt,
    trajectoryDir,
    metadata: {
      task_description: taskDescription,
      total_steps: trajectory.length,
      width: recording.metadata?.width,
      height: recording.metadata?.height,
      duration: recording.metadata?.duration,
      created_at: new Date().toISOString(),
    },
  };
}

// Create WebSocket server to receive recording
async function startRecordingServer(): Promise<{
  port: number;
  waitForRecording: () => Promise<Uint8Array>;
  stop: () => void;
}> {
  let resolveRecording: (data: Uint8Array) => void;
  const recordingPromise = new Promise<Uint8Array>((resolve) => {
    resolveRecording = resolve;
  });

  const chunks: Uint8Array[] = [];

  const server = Bun.serve({
    port: 0, // Let OS pick an available port
    fetch(req, server) {
      // Upgrade to WebSocket
      if (server.upgrade(req)) {
        return;
      }
      return new Response('WebSocket upgrade required', { status: 426 });
    },
    websocket: {
      message(_ws, message) {
        if (message instanceof ArrayBuffer) {
          chunks.push(new Uint8Array(message));
        } else if (message instanceof Uint8Array) {
          chunks.push(message);
        } else if (Buffer.isBuffer(message)) {
          chunks.push(new Uint8Array(message));
        } else if (typeof message === 'string') {
          const encoder = new TextEncoder();
          chunks.push(encoder.encode(message));
        } else {
          try {
            chunks.push(new Uint8Array(message as unknown as ArrayBuffer));
          } catch {
            // Ignore conversion errors
          }
        }
      },
      close() {
        const totalLength = chunks.reduce(
          (sum, chunk) => sum + chunk.length,
          0
        );
        const combined = new Uint8Array(totalLength);
        let offset = 0;
        for (const chunk of chunks) {
          combined.set(chunk, offset);
          offset += chunk.length;
        }
        resolveRecording(combined);
      },
      open() {},
    },
  });

  return {
    port: server.port as number,
    waitForRecording: () => recordingPromise,
    stop: () => server.stop(),
  };
}

// Check if a command is available
async function checkCommand(cmd: string): Promise<boolean> {
  try {
    const proc = Bun.spawn({
      cmd: ['which', cmd],
      stdout: 'ignore',
      stderr: 'ignore',
    });
    return (await proc.exited) === 0;
  } catch {
    return false;
  }
}

// Check required dependencies
async function checkDependencies(): Promise<void> {
  const missing: string[] = [];

  if (!(await checkCommand('ffmpeg'))) {
    missing.push('ffmpeg');
  }
  if (!(await checkCommand('convert'))) {
    missing.push('convert (ImageMagick)');
  }

  if (missing.length > 0) {
    console.error('Error: Missing required dependencies:');
    for (const dep of missing) {
      console.error(`  - ${dep}`);
    }
    console.error('\nInstall with:');
    if (process.platform === 'darwin') {
      console.error('  brew install ffmpeg imagemagick');
    } else {
      console.error('  apt-get install ffmpeg imagemagick  # Debian/Ubuntu');
      console.error('  # or your package manager equivalent');
    }
    process.exit(1);
  }
}

// Command handlers
const recordHandler = async (argv: Record<string, unknown>) => {
  // Check dependencies first
  await checkDependencies();

  const sandbox = argv.sandbox as string | undefined;
  const vncUrl = argv['vnc-url'] as string | undefined;
  const provider = ((argv.provider as string) || 'anthropic').toLowerCase() as
    | 'anthropic'
    | 'openai';
  const model =
    (argv.model as string) ||
    (provider === 'openai' ? 'gpt-5-mini' : 'claude-haiku-4-5');
  const apiKeyArg = argv['api-key'] as string | undefined;

  if (!sandbox && !vncUrl) {
    console.error('Either --sandbox or --vnc-url is required');
    process.exit(1);
  }

  // Get API key
  let apiKey = apiKeyArg;
  if (!apiKey) {
    if (provider === 'openai') {
      apiKey = process.env.OPENAI_API_KEY;
    } else {
      apiKey = process.env.ANTHROPIC_API_KEY;
    }
  }

  if (!apiKey) {
    const envVar =
      provider === 'openai' ? 'OPENAI_API_KEY' : 'ANTHROPIC_API_KEY';
    console.error(`Error: No ${provider.toUpperCase()} API key found.`);
    console.error(`Set ${envVar} environment variable or use --api-key flag.`);
    process.exit(1);
  }

  // Start recording server
  console.log('Starting recording server...');
  const { port, waitForRecording, stop } = await startRecordingServer();
  const recordUrl = `ws://localhost:${port}`;

  let recordingUrl: string;

  if (sandbox) {
    const token = await ensureApiKeyInteractive();
    const { url } = await getSandboxVncUrl(sandbox, token);
    const urlObj = new URL(url);
    urlObj.searchParams.set('autorecord', 'true');
    urlObj.searchParams.set('record_format', 'mp4');
    urlObj.searchParams.set('record_url', recordUrl);
    recordingUrl = urlObj.toString();
  } else {
    const urlObj = new URL(vncUrl!);
    urlObj.searchParams.set('autorecord', 'true');
    urlObj.searchParams.set('record_format', 'mp4');
    urlObj.searchParams.set('record_url', recordUrl);
    recordingUrl = urlObj.toString();
  }

  console.log('\nRecording will start automatically when you connect.');
  console.log('When finished, click "Stop Recording" in the VNC panel.\n');

  openInBrowser(recordingUrl);

  const timeoutMs = 30 * 60 * 1000;
  let recordingData: Uint8Array;

  try {
    const timeout = new Promise<never>((_, reject) =>
      setTimeout(
        () => reject(new Error('Recording timeout (30 minutes)')),
        timeoutMs
      )
    );
    recordingData = await Promise.race([waitForRecording(), timeout]);
  } catch (err) {
    stop();
    console.error(`Recording failed: ${err}`);
    process.exit(1);
  }

  stop();

  if (recordingData.length === 0) {
    console.error('No recording data received');
    process.exit(1);
  }

  // Get skill name (from flag or prompt)
  let skillName = (argv.name as string) || '';
  while (!skillName) {
    skillName = await prompt('Enter skill name: ');
    if (!skillName) {
      console.log('Skill name is required.');
      continue;
    }
    if (!/^[a-zA-Z0-9_-]+$/.test(skillName)) {
      console.log('Use only letters, numbers, hyphens, and underscores.');
      skillName = '';
    }
  }

  // Check for existing skill and deduplicate name
  await ensureSkillsDir();
  const fs = await import('fs/promises');
  let finalSkillName = skillName;
  let counter = 1;
  while (true) {
    try {
      await fs.access(`${SKILLS_DIR}/${finalSkillName}`);
      finalSkillName = `${skillName}-${counter}`;
      counter++;
    } catch {
      break; // Directory doesn't exist, we can use this name
    }
  }
  if (finalSkillName !== skillName) {
    console.log(`Skill "${skillName}" exists, using "${finalSkillName}"`);
  }
  skillName = finalSkillName;

  // Get skill description (from flag or prompt)
  let description = (argv.description as string) || '';
  while (!description) {
    description = await prompt('Describe what this skill demonstrates: ');
    if (!description) {
      console.log('Description is required.');
    }
  }

  console.log('\nProcessing recording...');

  const { trajectory, skillPrompt, trajectoryDir } =
    await processRecordingWithLLM(recordingData, description, skillName, {
      provider,
      model,
      apiKey,
    });

  if (trajectory.length === 0) {
    console.log('Warning: No input events captured.');
  }

  // Generate step descriptions from captions
  const stepsMarkdown = trajectory
    .map((step, idx) => {
      const caption = step.caption;
      const event = step.raw_event;
      const lines: string[] = [];

      lines.push(`### Step ${idx + 1}: ${caption.action || event.type}`);
      lines.push('');

      if (caption.observation) {
        // Trim "Cropped image shows" prefix if present
        const obs = caption.observation.replace(/^Cropped image shows\s*/i, '');
        lines.push(`**Context:** ${obs}`);
        lines.push('');
      }

      if (caption.think) {
        lines.push(`**Intent:** ${caption.think}`);
        lines.push('');
      }

      if (caption.expectation) {
        lines.push(`**Expected Result:** ${caption.expectation}`);
        lines.push('');
      }

      return lines.join('\n');
    })
    .join('\n');

  // Create skill markdown file with frontmatter
  const skillDir = `${SKILLS_DIR}/${skillName}`;
  const skillPath = `${skillDir}/SKILL.md`;
  const skillContent = `---
name: ${skillName}
description: ${description}
---

# ${skillName}

${description}

## Steps

${stepsMarkdown}

## Agent Prompt

${skillPrompt}
`;
  await Bun.write(skillPath, skillContent);

  console.log(`\nSkill saved: ${skillPath}`);
  console.log(`  Steps: ${trajectory.length}`);

  // Explicitly exit
  process.exit(0);
};

// Parse skill markdown frontmatter
function parseSkillFrontmatter(
  content: string
): { name: string; description: string; body: string } | null {
  const match = content.match(/^---\n([\s\S]*?)\n---\n([\s\S]*)$/);
  if (!match || !match[1] || match[2] === undefined) return null;

  const frontmatter = match[1];
  const body = match[2].trim();

  const nameMatch = frontmatter.match(/^name:\s*(.+)$/m);
  const descMatch = frontmatter.match(/^description:\s*(.+)$/m);

  if (!nameMatch?.[1] || !descMatch?.[1]) return null;

  return {
    name: nameMatch[1].trim(),
    description: descMatch[1].trim(),
    body,
  };
}

const listHandler = async () => {
  await ensureSkillsDir();

  const glob = new Bun.Glob('*/SKILL.md');
  const files: string[] = [];

  for await (const file of glob.scan({ cwd: SKILLS_DIR })) {
    files.push(file);
  }

  if (files.length === 0) {
    console.log('No skills found');
    console.log(`\nRecord a skill with: cua skills record --sandbox <name>`);
    return;
  }

  // Print skills table
  const headers = ['NAME', 'DESCRIPTION', 'STEPS', 'CREATED'];
  const rows: string[][] = [headers];

  for (const file of files.sort()) {
    try {
      const skillDir = file.replace('/SKILL.md', '');
      const content = await Bun.file(`${SKILLS_DIR}/${file}`).text();
      const parsed = parseSkillFrontmatter(content);
      if (!parsed) continue;

      // Try to read trajectory.json for steps count and created date
      let steps = 0;
      let created = '-';
      try {
        const trajContent = await Bun.file(
          `${SKILLS_DIR}/${skillDir}/trajectory/trajectory.json`
        ).text();
        const trajData = JSON.parse(trajContent);
        steps = trajData.trajectory?.length || 0;
        if (trajData.metadata?.created_at) {
          created = new Date(trajData.metadata.created_at).toLocaleDateString();
        }
      } catch {
        // Trajectory file might not exist
      }

      rows.push([
        parsed.name,
        parsed.description.substring(0, 40) +
          (parsed.description.length > 40 ? '...' : ''),
        String(steps),
        created,
      ]);
    } catch {
      // Skip malformed files
    }
  }

  // Calculate column widths
  const widths = headers.map((_, i) =>
    Math.max(...rows.map((r) => (r[i] || '').length))
  );

  // Print table
  for (const row of rows) {
    console.log(row.map((c, i) => c.padEnd(widths[i] || 0)).join('  '));
  }
};

const readHandler = async (argv: Record<string, unknown>) => {
  const name = String(argv.name);
  const format = (argv.format as string) || 'md';

  await ensureSkillsDir();

  const skillDir = `${SKILLS_DIR}/${name}`;
  const skillPath = `${skillDir}/SKILL.md`;
  const trajectoryDir = `${skillDir}/trajectory`;
  const file = Bun.file(skillPath);

  if (!(await file.exists())) {
    console.error(`Skill not found: ${name}`);
    console.log(`\nAvailable skills:`);
    await listHandler();
    process.exit(1);
  }

  const content = await file.text();
  const parsed = parseSkillFrontmatter(content);

  if (!parsed) {
    console.error(`Invalid skill file format: ${name}`);
    process.exit(1);
  }

  if (format === 'md') {
    // Just output the raw markdown file
    console.log(content);
  } else if (format === 'json') {
    // Read trajectory.json and combine with frontmatter
    try {
      const trajContent = await Bun.file(
        `${trajectoryDir}/trajectory.json`
      ).text();
      const trajData = JSON.parse(trajContent);
      const skill = {
        name: parsed.name,
        description: parsed.description,
        trajectory: trajData.trajectory || [],
        skill_prompt: parsed.body,
        trajectory_dir: trajectoryDir,
        metadata: trajData.metadata,
      };
      console.log(JSON.stringify(skill, null, 2));
    } catch {
      console.error(`Failed to read trajectory data from: ${trajectoryDir}`);
      process.exit(1);
    }
  } else {
    console.error(`Unknown format: ${format}`);
    process.exit(1);
  }
};

const deleteHandler = async (argv: Record<string, unknown>) => {
  const name = String(argv.name);

  await ensureSkillsDir();

  const skillDir = `${SKILLS_DIR}/${name}`;
  const skillPath = `${skillDir}/SKILL.md`;
  const file = Bun.file(skillPath);

  if (!(await file.exists())) {
    console.error(`Skill not found: ${name}`);
    process.exit(1);
  }

  const fs = await import('fs/promises');

  // Delete entire skill directory (contains SKILL.md and trajectory/)
  await fs.rm(skillDir, { recursive: true });

  console.log(`Deleted skill: ${name}`);
};

const replayHandler = async (argv: Record<string, unknown>) => {
  const name = String(argv.name);

  await ensureSkillsDir();

  const skillDir = `${SKILLS_DIR}/${name}`;
  const skillPath = `${skillDir}/SKILL.md`;
  const trajectoryDir = `${skillDir}/trajectory`;
  const file = Bun.file(skillPath);

  if (!(await file.exists())) {
    console.error(`Skill not found: ${name}`);
    process.exit(1);
  }

  // Find the .mp4 file in the trajectory directory
  const glob = new Bun.Glob('*.mp4');
  let mp4File: string | null = null;

  for await (const f of glob.scan({ cwd: trajectoryDir })) {
    mp4File = `${trajectoryDir}/${f}`;
    break;
  }

  if (!mp4File) {
    console.error(`No video found in: ${trajectoryDir}`);
    process.exit(1);
  }

  openInBrowser(`file://${mp4File}`);
};

const cleanHandler = async () => {
  await ensureSkillsDir();

  const glob = new Bun.Glob('*/SKILL.md');
  const files: string[] = [];

  for await (const file of glob.scan({ cwd: SKILLS_DIR })) {
    files.push(file);
  }

  if (files.length === 0) {
    console.log('No skills to clean.');
    return;
  }

  // List skills that will be deleted
  console.log(`\nSkills to delete:`);
  const skillDirs = files.map((f) => f.replace('/SKILL.md', '')).sort();
  for (const dir of skillDirs) {
    console.log(`  - ${dir}`);
  }

  const answer = await prompt(
    `\nDelete ${skillDirs.length} skill${skillDirs.length > 1 ? 's' : ''}? y/[n]: `
  );

  if (answer.toLowerCase() !== 'y') {
    console.log('Cancelled.');
    return;
  }

  const fs = await import('fs/promises');

  for (const dir of skillDirs) {
    // Delete entire skill directory
    await fs.rm(`${SKILLS_DIR}/${dir}`, { recursive: true });
  }

  console.log(
    `Deleted ${skillDirs.length} skill${skillDirs.length > 1 ? 's' : ''}.`
  );
};

// Register commands
export function registerSkillsCommands(argv: Argv): Argv {
  return argv.command('skills', 'Manage demonstration skills', (yargs) =>
    yargs
      .command(
        'record',
        'Record a demonstration and create a skill',
        (y) =>
          y
            .option('sandbox', {
              alias: 's',
              type: 'string',
              description: 'Sandbox name to connect to',
            })
            .option('vnc-url', {
              alias: 'u',
              type: 'string',
              description: 'Direct VNC URL to connect to',
            })
            .option('provider', {
              alias: 'p',
              type: 'string',
              description: 'LLM provider for captioning (anthropic or openai)',
              default: 'anthropic',
              choices: ['anthropic', 'openai'],
            })
            .option('model', {
              alias: 'm',
              type: 'string',
              description: 'Model to use for captioning',
            })
            .option('api-key', {
              alias: 'k',
              type: 'string',
              description: 'API key for the LLM provider',
            })
            .option('name', {
              alias: 'n',
              type: 'string',
              description: 'Skill name (skips interactive prompt)',
            })
            .option('description', {
              alias: 'd',
              type: 'string',
              description: 'Skill description (skips interactive prompt)',
            }),
        recordHandler
      )
      .command('list', 'List all saved skills', {}, listHandler)
      .command('ls', false, {}, listHandler)
      .command(
        'read <name>',
        'Read a skill as JSON or Markdown',
        (y) =>
          y
            .positional('name', {
              type: 'string',
              description: 'Skill name',
              demandOption: true,
            })
            .option('format', {
              alias: 'f',
              type: 'string',
              description: 'Output format (json or md)',
              default: 'md',
              choices: ['json', 'md'],
            }),
        readHandler
      )
      .command(
        'delete <name>',
        'Delete a skill',
        (y) =>
          y.positional('name', {
            type: 'string',
            description: 'Skill name',
            demandOption: true,
          }),
        deleteHandler
      )
      .command(
        'replay <name>',
        'Open the video recording for a skill',
        (y) =>
          y.positional('name', {
            type: 'string',
            description: 'Skill name',
            demandOption: true,
          }),
        replayHandler
      )
      .command(
        'clean',
        'Delete all skills (with confirmation)',
        {},
        cleanHandler
      )
      .demandCommand(1)
  );
}
