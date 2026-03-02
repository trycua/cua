// Barrel export for all types

// Message types (re-exported from @trycua/agent + additional types)
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
  FunctionCallMessage,
  FunctionCallOutputMessage,
  ComputerResultContent,
  AgentKwargs,
} from './messages';

// Chat types (copied from cloud repo)
export type {
  Model,
  ApiKeyOption,
  ApiKeySettings,
  ChatError,
  ModelProvider,
  CustomComputer,
  VM,
  Computer,
  Chat,
  ProcessedMessage,
  RetryState,
  ChatState,
  ChatAction,
} from './chat';

export {
  VMStatus,
  isVM,
  isCustomComputer,
  getComputerId,
  getComputerName,
  getComputerStatus,
} from './chat';
