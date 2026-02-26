// Types copied from cloud/src/website/app/types.ts and cloud/src/website/app/contexts/ChatContext.tsx
// IMPORTANT: Keep these in sync with the source files

import type { AgentMessage, ComputerCallOutputMessage } from './messages';

// Model type
export interface Model {
  id: string;
  name: string;
}

// API Key Settings Types (used by playground)
export type ApiKeyOption = 'cua' | 'custom' | 'none';

export interface ApiKeySettings {
  option: ApiKeyOption;
  selectedCuaKey: string;
  customKey: string;
}

// Chat error type
export interface ChatError {
  message: string;
  timestamp: Date;
}

// Model provider type
export interface ModelProvider {
  name: string;
  models: Model[];
}

// Custom computer type (user-provided URLs)
export interface CustomComputer {
  id: string;
  url: string;
  name: string;
  status?: string;
}

// VM Status enum
export enum VMStatus {
  Provisioning = 'provisioning',
  Failed = 'failed',
  Starting = 'starting',
  Running = 'running',
  Restarting = 'restarting',
  Stopping = 'stopping',
  Stopped = 'stopped',
  Suspending = 'suspending',
  Suspended = 'suspended',
  Preempted = 'preempted',
  Error = 'error',
  Deleting = 'deleting',
  Deleted = 'deleted',
}

// VM type (from cloud)
export interface VM {
  id: string;
  vmId: string;
  name: string;
  status: string;
  vncUrl: string | null;
  customName?: string;
  size?: string;
  region?: string;
  os?: string;
  workspaceName?: string | null;
}

// Unified computer type that is a union of VM and CustomComputer
export type Computer = VM | CustomComputer;

// Type guards for Computer union type
export function isVM(computer: Computer): computer is VM {
  return 'vmId' in computer;
}

export function isCustomComputer(computer: Computer): computer is CustomComputer {
  return 'url' in computer && !('vmId' in computer);
}

// Helper functions to get common properties
export function getComputerId(computer: Computer): string {
  return isVM(computer) ? computer.vmId.toString() : computer.id;
}

export function getComputerName(computer: Computer): string {
  if (isVM(computer)) {
    return computer.customName || computer.name;
  }
  return computer.name;
}

export function getComputerStatus(computer: Computer): string | undefined {
  return computer.status;
}

// Chat interface
export interface Chat {
  id: string;
  name: string;
  model?: Model;
  computer?: Computer;
  updated: Date;
  created: Date;
  messages: AgentMessage[];
}

// Processed message type for rendering (from ChatContext.tsx)
export interface ProcessedMessage {
  type: 'single';
  message: AgentMessage;
  screenshot?: ComputerCallOutputMessage;
  toolCalls?: {
    message: AgentMessage;
    screenshot?: ComputerCallOutputMessage;
    output?: AgentMessage;
  }[];
}

// Retry state for displaying retry progress
export interface RetryState {
  attempt: number;
  maxRetries: number;
}

// Chat state type (from ChatContext.tsx)
export interface ChatState {
  id: string;
  name: string;
  messages: AgentMessage[];
  model?: Model;
  computer?: Computer;
  currentInput: string;
  waitingForResponse: boolean;
  chatError: ChatError | null;
  processedMessages: ProcessedMessage[];
  retryState: RetryState | null;
}

// Chat action types (from ChatContext.tsx)
export type ChatAction =
  | { type: 'SET_INPUT'; payload: string }
  | { type: 'SET_MODEL'; payload: Model }
  | { type: 'SET_COMPUTER'; payload: Computer }
  | { type: 'ADD_MESSAGE'; payload: AgentMessage }
  | { type: 'SET_MESSAGES'; payload: AgentMessage[] }
  | { type: 'APPEND_MESSAGES'; payload: AgentMessage[] }
  | { type: 'SET_WAITING'; payload: boolean }
  | { type: 'SET_ERROR'; payload: ChatError | null }
  | { type: 'SET_RETRY_STATE'; payload: RetryState | null }
  | { type: 'UPDATE_NAME'; payload: string }
  | { type: 'CLEAR_INPUT' }
  | { type: 'RESET_CHAT'; payload: Chat };
