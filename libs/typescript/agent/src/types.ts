// #region Request
export type ConnectionType = 'http' | 'https' | 'peer';
export interface AgentClientOptions {
  timeout?: number;
  retries?: number;
  /** Optional CUA API key to send as X-API-Key header for HTTP requests */
  apiKey?: string;
}
// Request types matching the Python proxy API
export interface AgentRequest {
  model: string;
  input: string | AgentMessage[];
  agent_kwargs?: {
    save_trajectory?: boolean;
    verbosity?: number;
    [key: string]: any;
  };
  computer_kwargs?: {
    os_type?: string;
    provider_type?: string;
    [key: string]: any;
  };
  /**
   * Optional per-request environment variable overrides.
   * Keys and values are strings and will be forwarded to the backend proxy.
   */
  env?: Record<string, string>;
}
// #endregion

// #region Response
// Response types
export interface AgentResponse {
  output: AgentMessage[];
  usage: Usage;
  status: 'completed' | 'failed';
  error?: string;
}
// Usage information
export interface Usage {
  prompt_tokens: number;
  completion_tokens: number;
  total_tokens: number;
  response_cost: number;
}
// #endregion

// #region Messages
// Agent message types - can be one of several different message types
export type AgentMessage =
  | UserMessage
  | AssistantMessage
  | ReasoningMessage
  | ComputerCallMessage
  | ComputerCallOutputMessage
  | FunctionCallMessage
  | FunctionCallOutputMessage;
// Input message
export interface UserMessage {
  type?: 'message';
  role: 'user' | 'system' | 'developer';
  content: string | InputContent[];
}
// Output message
export interface AssistantMessage {
  type: 'message';
  role: 'assistant';
  content: OutputContent[];
}
// Output reasoning/thinking message
export interface ReasoningMessage {
  type: 'reasoning';
  summary: SummaryContent[];
}
// Output computer action call
export interface ComputerCallMessage {
  type: 'computer_call';
  call_id: string;
  status: 'completed' | 'failed' | 'pending';
  action: ComputerAction;
}
// Output computer action result (always a screenshot)
export interface ComputerCallOutputMessage {
  type: 'computer_call_output';
  call_id: string;
  output: ComputerResultContent;
}
// Output function call
export interface FunctionCallMessage {
  type: 'function_call';
  call_id: string;
  status: 'completed' | 'failed' | 'pending';
  name: string;
  arguments: string; // JSON dict of kwargs
}
// Output function call result (always text)
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
// OpenAI Computer Actions
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
// Anthropic Computer Actions
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
