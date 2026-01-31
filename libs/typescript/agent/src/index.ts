// Export the main AgentClient class as default
export { AgentClient as default } from './client.js';

// Also export as named export for flexibility
export { AgentClient } from './client.js';

// Export types for TypeScript users
export type {
  AgentRequest,
  AgentResponse,
  AgentMessage,
  UserMessage,
  AssistantMessage,
  ReasoningMessage,
  ComputerCallMessage,
  ComputerCallOutputMessage,
  OutputContent,
  SummaryContent,
  InputContent,
  ComputerAction,
  ClickAction,
  TypeAction,
  KeyPressAction,
  ScrollAction,
  WaitAction,
  Usage,
  ConnectionType,
  AgentClientOptions,
  StreamEvent,
  StreamCallback,
} from './types';
