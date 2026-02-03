// Re-export core message types from @trycua/agent
// Note: @trycua/agent package provides the canonical definitions for agent messages
export type {
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
  AgentRequest,
  AgentResponse,
  ConnectionType,
  AgentClientOptions,
} from '@trycua/agent';

// Additional message types used by the playground that may not be in @trycua/agent
// These are copied from cloud/src/website/app/types.ts for completeness

export interface FunctionCallMessage {
  type: 'function_call';
  call_id: string;
  status: 'completed' | 'failed' | 'pending';
  name: string;
  arguments: string; // JSON dict of kwargs
}

export interface FunctionCallOutputMessage {
  type: 'function_call_output';
  call_id: string;
  output: string;
}

export interface ComputerResultContent {
  type: 'computer_screenshot' | 'input_image';
  image_url: string;
}

// Agent kwargs matching ComputerAgent constructor parameters
export interface AgentKwargs {
  tools?: unknown[];
  custom_loop?: unknown;
  only_n_most_recent_images?: number;
  callbacks?: unknown[];
  instructions?: string;
  verbosity?: number;
  trajectory_dir?: string | Record<string, unknown>;
  max_retries?: number;
  screenshot_delay?: number;
  use_prompt_caching?: boolean;
  max_trajectory_budget?: number | Record<string, unknown>;
  telemetry_enabled?: boolean | { log_trajectory?: boolean };
  trust_remote_code?: boolean;
  api_key?: string;
  api_base?: string;
  [key: string]: unknown;
}
