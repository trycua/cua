// Core message types for the playground
// These types are compatible with @trycua/agent but defined locally to avoid
// workspace dependency issues when consuming the package externally

// #region Request
export type ConnectionType = 'http' | 'https' | 'peer';

export interface AgentClientOptions {
  timeout?: number;
  retries?: number;
  /** Optional Cua API key to send as X-API-Key header for HTTP requests */
  apiKey?: string;
}

export interface AgentRequest {
  model: string;
  input: string | AgentMessage[];
  agent_kwargs?: {
    save_trajectory?: boolean;
    verbosity?: number;
    [key: string]: unknown;
  };
  computer_kwargs?: {
    os_type?: string;
    provider_type?: string;
    [key: string]: unknown;
  };
  /** Optional per-request environment variable overrides */
  env?: Record<string, string>;
}
// #endregion

// #region Response
export interface AgentResponse {
  output: AgentMessage[];
  usage: Usage;
  status: 'completed' | 'failed';
  error?: string;
}

export interface Usage {
  prompt_tokens: number;
  completion_tokens: number;
  total_tokens: number;
  response_cost: number;
}
// #endregion

// #region Messages
export type AgentMessage =
  | UserMessage
  | AssistantMessage
  | ReasoningMessage
  | ComputerCallMessage
  | ComputerCallOutputMessage
  | FunctionCallMessage
  | FunctionCallOutputMessage;

export interface UserMessage {
  type?: 'message';
  role: 'user' | 'system' | 'developer';
  content: string | InputContent[];
}

export interface AssistantMessage {
  type: 'message';
  role: 'assistant';
  content: OutputContent[];
}

export interface ReasoningMessage {
  type: 'reasoning';
  summary: SummaryContent[];
}

export interface ComputerCallMessage {
  type: 'computer_call';
  call_id: string;
  status: 'completed' | 'failed' | 'pending';
  action: ComputerAction;
}

export interface ComputerCallOutputMessage {
  type: 'computer_call_output';
  call_id: string;
  output: ComputerResultContent;
}

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
// #endregion

// #region Message Content
export interface InputContent {
  type: 'input_image' | 'input_text';
  text?: string;
  image_url?: string;
}

export interface OutputContent {
  type: 'output_text';
  text: string;
}

export interface SummaryContent {
  type: 'summary_text';
  text: string;
}

export interface ComputerResultContent {
  type: 'computer_screenshot' | 'input_image';
  image_url: string;
}
// #endregion

// #region Actions
export type ComputerAction = ComputerActionOpenAI | ComputerActionAnthropic;

export type ComputerActionOpenAI =
  | ClickAction
  | DoubleClickAction
  | DragAction
  | KeyPressAction
  | MoveAction
  | ScreenshotAction
  | ScrollAction
  | TypeAction
  | WaitAction;

export interface ClickAction {
  type: 'click';
  button: 'left' | 'right' | 'wheel' | 'back' | 'forward';
  x: number;
  y: number;
}

export interface DoubleClickAction {
  type: 'double_click';
  button?: 'left' | 'right' | 'wheel' | 'back' | 'forward';
  x: number;
  y: number;
}

export interface DragAction {
  type: 'drag';
  button?: 'left' | 'right' | 'wheel' | 'back' | 'forward';
  path: Array<[number, number]>;
}

export interface KeyPressAction {
  type: 'keypress';
  keys: string[];
}

export interface MoveAction {
  type: 'move';
  x: number;
  y: number;
}

export interface ScreenshotAction {
  type: 'screenshot';
}

export interface ScrollAction {
  type: 'scroll';
  scroll_x: number;
  scroll_y: number;
  x: number;
  y: number;
}

export interface TypeAction {
  type: 'type';
  text: string;
}

export interface WaitAction {
  type: 'wait';
}

export type ComputerActionAnthropic = LeftMouseDownAction | LeftMouseUpAction;

export interface LeftMouseDownAction {
  type: 'left_mouse_down';
  x: number;
  y: number;
}

export interface LeftMouseUpAction {
  type: 'left_mouse_up';
  x: number;
  y: number;
}
// #endregion

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
