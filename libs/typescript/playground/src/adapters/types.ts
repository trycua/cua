// Adapter interfaces (contracts) for the playground
// These define WHAT the playground needs, not HOW it's implemented

import type { AgentMessage, Chat, ModelProvider } from '../types';

// =============================================================================
// Main Adapter Bundle
// =============================================================================

/**
 * Main adapter bundle that the Playground component receives.
 * Provides all the infrastructure the playground needs to function.
 */
export interface PlaygroundAdapters {
  persistence: PersistenceAdapter;
  computer: ComputerAdapter;
  inference: InferenceAdapter;
}

// =============================================================================
// Persistence Adapter
// =============================================================================

/**
 * Contract: Where chats are stored
 * - Local implementation: localStorage
 * - Cloud implementation: CUA API
 */
export interface PersistenceAdapter {
  /** Load all chats for the current user/session */
  loadChats(): Promise<Chat[]>;

  /** Save a chat (create or update) */
  saveChat(chat: Chat): Promise<Chat>;

  /** Delete a chat by ID */
  deleteChat(chatId: string): Promise<void>;

  /** Save messages for a specific chat */
  saveMessages(chatId: string, messages: AgentMessage[]): Promise<void>;

  /** Load a settings value by key */
  loadSettings<T>(key: string): Promise<T | null>;

  /** Save a settings value by key */
  saveSettings<T>(key: string, value: T): Promise<void>;
}

// =============================================================================
// Computer Adapter
// =============================================================================

/**
 * Computer info as exposed by the adapter.
 * Normalized representation that works for both VMs and custom computers.
 */
export interface ComputerInfo {
  id: string;
  name: string;
  vncUrl: string;
  agentUrl: string;
  status: 'running' | 'stopped' | 'starting' | 'error';
  isCustom?: boolean;
}

/**
 * Contract: What computers are available
 * - Local implementation: user-provided URLs, stored in localStorage
 * - Cloud implementation: CUA API for VMs
 */
export interface ComputerAdapter {
  /** List all available computers */
  listComputers(): Promise<ComputerInfo[]>;

  /** Get the default computer (first running one, or null) */
  getDefaultComputer(): Promise<ComputerInfo | null>;

  /** Add a custom computer (optional, for local adapter) */
  addCustomComputer?(computer: Omit<ComputerInfo, 'id'>): Promise<ComputerInfo>;

  /** Remove a custom computer (optional, for local adapter) */
  removeCustomComputer?(computerId: string): Promise<void>;

  /** Check if a computer is healthy/reachable (optional) */
  checkHealth?(computerId: string): Promise<boolean>;
}

// =============================================================================
// Inference Adapter
// =============================================================================

/**
 * Configuration for making inference requests.
 * Returned by the inference adapter based on the selected computer.
 */
export interface InferenceConfig {
  /** Base URL for the agent API */
  baseUrl: string;

  /** Model to use (optional, may be selected by user) */
  model?: string;

  /** Environment variables to pass to the agent */
  env?: Record<string, string>;
}

/**
 * Contract: How inference is configured
 * - Local implementation: user-provided API keys
 * - Cloud implementation: CUA-managed keys
 */
export interface InferenceAdapter {
  /** Get inference configuration for a specific computer */
  getConfig(computer: ComputerInfo): Promise<InferenceConfig>;

  /** Get available model providers and their models */
  getAvailableModels(): Promise<ModelProvider[]>;
}

// =============================================================================
// Factory Function Config Types
// =============================================================================

/**
 * Configuration for creating a local adapter.
 * Used when running the playground locally with user-provided resources.
 */
export interface LocalAdapterConfig {
  /** URL of the computer server (e.g., http://localhost:8443) */
  computerServerUrl?: string;

  /** API keys for different providers */
  providerApiKeys?: {
    anthropic?: string;
    openai?: string;
  };
}

/**
 * Configuration for creating a cloud adapter.
 * Used when running the playground with CUA cloud infrastructure.
 */
export interface CloudAdapterConfig {
  /** CUA API key for authentication */
  apiKey: string;

  /** Base URL for the CUA API (defaults to https://api.cua.ai) */
  baseUrl?: string;
}
