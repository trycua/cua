import type { AgentMessage, UserMessage } from '../types';

/**
 * Represents a single agent run (trajectory) starting from a user message
 */
export interface TrajectoryRun {
  /** 0-based index of this run in the chat */
  index: number;
  /** Preview of the user's prompt (truncated for display) */
  userPromptPreview: string;
  /** Full user prompt text */
  userPrompt: string;
  /** All messages in this run (including the starting user message) */
  messages: AgentMessage[];
  /** Number of screenshots in this run */
  screenshotCount: number;
  /** Number of agent actions (computer_call + function_call) */
  actionCount: number;
  /** Whether this run has any content beyond the user message */
  isEmpty: boolean;
}

const MAX_PREVIEW_LENGTH = 60;

/**
 * Get preview text from a user message
 */
function getUserPromptText(message: UserMessage): string {
  if (typeof message.content === 'string') {
    return message.content;
  }
  // Extract text from InputContent array
  const textParts = message.content
    .filter((c) => c.type === 'input_text' && c.text)
    .map((c) => c.text)
    .filter(Boolean);
  return textParts.join(' ') || '[Image input]';
}

/**
 * Truncate text to a maximum length with ellipsis
 */
function truncate(text: string, maxLength: number): string {
  const cleaned = text.replace(/\n/g, ' ').trim();
  if (cleaned.length <= maxLength) {
    return cleaned;
  }
  return `${cleaned.slice(0, maxLength - 3)}...`;
}

/**
 * Check if a message is a user message (starts a new run)
 */
function isUserMessage(message: AgentMessage): message is UserMessage {
  return (
    message.type === 'message' ||
    (message.type === undefined && 'role' in message && message.role === 'user')
  );
}

/**
 * Infer agent runs from a flat array of messages.
 * Each UserMessage starts a new run. Consecutive UserMessages result in
 * empty runs (no agent response) which are still valid for export.
 *
 * @param messages - The flat array of messages from a chat session
 * @returns Array of TrajectoryRun objects
 */
export function inferRuns(messages: AgentMessage[]): TrajectoryRun[] {
  const runs: TrajectoryRun[] = [];
  let currentRunMessages: AgentMessage[] = [];
  let currentRunIndex = 0;
  let currentUserPrompt = '';

  for (const message of messages) {
    // Check if this is a user message (role === 'user')
    const isUser = isUserMessage(message) && 'role' in message && message.role === 'user';

    if (isUser) {
      // If we have accumulated messages, save the previous run
      if (currentRunMessages.length > 0) {
        runs.push(createRun(currentRunIndex, currentUserPrompt, currentRunMessages));
        currentRunIndex++;
      }

      // Start new run with this user message
      currentUserPrompt = getUserPromptText(message as UserMessage);
      currentRunMessages = [message];
    } else {
      // Non-user message: add to current run
      currentRunMessages.push(message);
    }
  }

  // Don't forget the last run
  if (currentRunMessages.length > 0) {
    runs.push(createRun(currentRunIndex, currentUserPrompt, currentRunMessages));
  }

  return runs;
}

/**
 * Create a TrajectoryRun object from collected messages
 */
function createRun(index: number, userPrompt: string, messages: AgentMessage[]): TrajectoryRun {
  let screenshotCount = 0;
  let actionCount = 0;

  for (const msg of messages) {
    if (msg.type === 'computer_call_output') {
      screenshotCount++;
    }
    if (msg.type === 'computer_call' || msg.type === 'function_call') {
      actionCount++;
    }
  }

  // A run is empty if it only contains the user message (no agent response)
  const firstMsg = messages[0];
  const isEmpty = messages.length === 1 && 'role' in firstMsg && firstMsg.role === 'user';

  return {
    index,
    userPromptPreview: truncate(userPrompt, MAX_PREVIEW_LENGTH),
    userPrompt,
    messages,
    screenshotCount,
    actionCount,
    isEmpty,
  };
}
