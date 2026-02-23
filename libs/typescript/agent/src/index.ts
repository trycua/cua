import { Telemetry } from '@trycua/core';

// Emit module_init event when the agent package is imported
const telemetry = new Telemetry();
telemetry.recordEvent('module_init', {
  module: 'agent',
  version: process.env.npm_package_version || '0.1.0',
  node_version: process.version,
});

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
} from './types';
