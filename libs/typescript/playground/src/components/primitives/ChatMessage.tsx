import { AnimatePresence, motion } from 'motion/react';
import {
  ArrowDown,
  ArrowUp,
  Brain,
  Camera,
  Check,
  CheckCircle,
  ChevronDown,
  ChevronRight,
  Clock,
  Copy,
  Keyboard,
  Mouse,
  MousePointer,
  Move,
  RotateCcw,
  Wrench,
  XCircle,
} from 'lucide-react';
import React, { useState } from 'react';
import Markdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { ThinkingCompleteAnimation } from './ThinkingCompleteAnimation';
import { cn } from '../../utils/cn';
import type {
  AgentMessage,
  AssistantMessage,
  ComputerAction,
  ComputerCallMessage,
  ComputerCallOutputMessage,
  FunctionCallMessage,
  FunctionCallOutputMessage,
  InputContent,
  OutputContent,
  ReasoningMessage,
  UserMessage,
} from '../../types';

interface MessageChatProps {
  message: AgentMessage;
  screenshot?: ComputerCallOutputMessage;
  toolCalls?: {
    message: AgentMessage;
    screenshot?: ComputerCallOutputMessage;
    output?: AgentMessage;
  }[];
  isLastInGroup?: boolean; // New prop to indicate if this is the last consecutive agent message
  isLastConsecutiveUserMessage?: boolean; // Whether this is the last user message in a consecutive group
  durationSeconds?: number; // Duration of the request in seconds (in-memory only)
}

export const ChatMessage: React.FC<MessageChatProps> = React.memo(
  ({
    message,
    screenshot,
    toolCalls,
    isLastInGroup,
    isLastConsecutiveUserMessage,
    durationSeconds,
  }) => {
    const [isReasoningExpanded, setIsReasoningExpanded] = useState<boolean>(false);

    const renderContent = (content: string | InputContent[] | OutputContent[]): string => {
      if (typeof content === 'string') {
        return content;
      }

      return content
        .map((item) => {
          if ('text' in item && item.text) {
            return item.text;
          }
          if (item.type === 'input_image') {
            //todo: image input
            return '[Image]';
          }
          return '';
        })
        .filter(Boolean)
        .join(' ');
    };

    const UserMessageComponent = ({
      message,
      index,
      showCopyButton,
    }: {
      message: UserMessage;
      index: string;
      showCopyButton?: boolean;
    }) => {
      const [copied, setCopied] = useState(false);

      const handleCopy = () => {
        navigator.clipboard.writeText(renderContent(message.content));
        setCopied(true);
        setTimeout(() => setCopied(false), 2000);
      };

      return (
        <div key={index} className="group flex flex-col items-end gap-2">
          <div className="max-w-2xl rounded-3xl bg-neutral-100 px-5 py-3 text-left dark:bg-neutral-800">
            <p className="text-neutral-900 dark:text-white">{renderContent(message.content)}</p>
          </div>
          {/* Copy button - only visible on hover and only on last consecutive user message */}
          {showCopyButton && (
            <button
              type="button"
              onClick={handleCopy}
              className="rounded-lg p-1.5 opacity-0 transition-all hover:bg-neutral-100 group-hover:opacity-100 dark:hover:bg-neutral-800"
              title="Copy message"
            >
              {copied ? (
                <Check className="h-4 w-4 text-green-600 dark:text-green-400" />
              ) : (
                <Copy className="h-4 w-4 text-neutral-500 dark:text-neutral-400" />
              )}
            </button>
          )}
        </div>
      );
    };

    const renderUserMessage = (message: UserMessage, index: string) => {
      return (
        <UserMessageComponent
          message={message}
          index={index}
          showCopyButton={isLastConsecutiveUserMessage}
        />
      );
    };

    const BaseAssistantMessage = ({
      children,
      index,
      className,
    }: {
      children: React.ReactNode;
      index: string;
      className?: string;
    }) => {
      return (
        <div key={index} className="flex justify-start">
          <div
            className={cn(
              'flex max-w-4xl gap-2 rounded-xl border border-neutral-200 bg-white px-3 py-2 text-left shadow-sm dark:border-neutral-700 dark:bg-neutral-900',
              className
            )}
          >
            {children}
          </div>
        </div>
      );
    };

    const AssistantMessageComponent = ({
      message,
      index,
      toolCalls: messageToolCalls,
      isLastInGroup,
      durationSeconds: msgDurationSeconds,
    }: {
      message: AssistantMessage;
      index: string;
      toolCalls?: {
        message: AgentMessage;
        screenshot?: ComputerCallOutputMessage;
        output?: AgentMessage;
      }[];
      isLastInGroup?: boolean;
      durationSeconds?: number;
    }) => {
      const [copied, setCopied] = useState(false);

      const [expandedCallIndex, setExpandedCallIndex] = useState<number | null>(null);

      const handleCopy = () => {
        // Copy all consecutive agent messages
        let textToCopy = renderContent(message.content);

        // If there are tool calls, include them in the copy
        if (messageToolCalls && messageToolCalls.length > 0) {
          const toolCallsText = messageToolCalls
            .map(({ message: toolMessage }) => {
              if (toolMessage.type === 'computer_call') {
                const action = (toolMessage as ComputerCallMessage).action;
                return `[Action: ${action.type}]`;
              }
              if (toolMessage.type === 'reasoning') {
                const reasoning = toolMessage as ReasoningMessage;
                return `[Reasoning: ${reasoning.summary.map((s) => s.text).join(' ')}]`;
              }
              return '';
            })
            .filter(Boolean)
            .join('\n');

          if (toolCallsText) {
            textToCopy += `\n\n${toolCallsText}`;
          }
        }

        navigator.clipboard.writeText(textToCopy);
        setCopied(true);
        setTimeout(() => setCopied(false), 2000);
      };

      const toggleCallExpanded = (index: number) => {
        // Only allow one expanded at a time
        setExpandedCallIndex(expandedCallIndex === index ? null : index);
      };

      const getCallIcon = (toolMessage: AgentMessage) => {
        if (toolMessage.type === 'computer_call') {
          const action = (toolMessage as ComputerCallMessage).action;
          switch (action.type) {
            case 'click':
              return <Mouse className="h-4 w-4" />;
            case 'type':
              return <Keyboard className="h-4 w-4" />;
            case 'screenshot':
              return <Camera className="h-4 w-4" />;
            default:
              return <Mouse className="h-4 w-4" />;
          }
        }
        if (toolMessage.type === 'reasoning') {
          return <Brain className="h-4 w-4" />;
        }
        return <Wrench className="h-4 w-4" />;
      };

      const getCallLabel = (toolMessage: AgentMessage): string => {
        if (toolMessage.type === 'computer_call') {
          const action = (toolMessage as ComputerCallMessage).action;
          switch (action.type) {
            case 'click':
              return `Clicked ${action.button}`;
            case 'type':
              return 'Typed';
            case 'screenshot':
              return 'Took screenshot';
            case 'scroll':
              return 'Scrolled';
            case 'keypress':
              return 'Pressed keys';
            default:
              return `Used ${action.type}`;
          }
        }
        if (toolMessage.type === 'reasoning') {
          return 'Reasoning';
        }
        if (toolMessage.type === 'function_call') {
          const functionCall = toolMessage as FunctionCallMessage;
          const toolName = functionCall.name;

          // Try to parse arguments and check for action property
          try {
            const args = JSON.parse(functionCall.arguments);
            if (args.action) {
              return `Used ${toolName}::${args.action}`;
            }
          } catch {
            // If parsing fails, just use the tool name
          }

          return `Used ${toolName}`;
        }
        return 'Tool call';
      };

      return (
        <div key={index} className="flex flex-col items-start gap-2">
          <div className="prose prose-neutral dark:prose-invert prose-headings:my-3 prose-li:my-1 prose-p:my-2 prose-ul:my-2 max-w-4xl text-left">
            <Markdown remarkPlugins={[remarkGfm]}>{renderContent(message.content)}</Markdown>
          </div>

          {/* Tool calls section - 2-row layout */}
          {messageToolCalls && messageToolCalls.length > 0 && (
            <div className="my-3 w-full max-w-[1400px] rounded-lg border border-neutral-200/50 dark:border-neutral-700/50">
              {/* Row 1: Dropdown buttons */}
              <div className="flex max-h-48 flex-col overflow-y-auto">
                {messageToolCalls.map(({ message: toolMessage }, toolIndex) => {
                  const isExpanded = expandedCallIndex === toolIndex;
                  const isReasoning = toolMessage.type === 'reasoning';

                  return (
                    <button
                      key={`call-${toolIndex}`}
                      type="button"
                      onClick={() => toggleCallExpanded(toolIndex)}
                      className={`flex w-full items-center justify-between border-neutral-200/50 border-b px-3 py-1.5 text-sm transition-colors last:border-b-0 dark:border-neutral-700/50 ${
                        isExpanded
                          ? 'bg-neutral-100 text-neutral-900 dark:bg-neutral-800 dark:text-white'
                          : 'text-neutral-600 hover:bg-neutral-50 dark:text-neutral-400 dark:hover:bg-neutral-800/50'
                      }`}
                    >
                      <div className="flex items-center gap-2">
                        {getCallIcon(toolMessage)}
                        <span>{getCallLabel(toolMessage)}</span>
                      </div>
                      {!isReasoning && (
                        <ChevronDown
                          className={`h-4 w-4 transition-transform ${isExpanded ? 'rotate-180' : ''}`}
                        />
                      )}
                    </button>
                  );
                })}
              </div>

              {/* Row 2: Content area - show action details when expanded */}
              {expandedCallIndex !== null && (
                <div className="w-full bg-neutral-50 dark:bg-neutral-900">
                  <div className="space-y-4 p-4">
                    {(() => {
                      const {
                        message: toolMessage,
                        screenshot,
                        output,
                      } = messageToolCalls[expandedCallIndex];

                      if (toolMessage.type === 'reasoning') {
                        const reasoningCall = toolMessage as ReasoningMessage;
                        return (
                          <div className="space-y-2">
                            <div className="flex items-center gap-2 font-medium text-sm">
                              <Brain className="h-4 w-4 text-blue-600 dark:text-blue-400" />
                              <span>Reasoning</span>
                            </div>
                            <p className="rounded-md border bg-white p-3 text-neutral-700 text-sm dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-200">
                              {reasoningCall.summary.map((s) => s.text).join(' ')}
                            </p>
                          </div>
                        );
                      }

                      if (toolMessage.type === 'function_call') {
                        const functionCall = toolMessage as FunctionCallMessage;
                        const functionOutput = output as FunctionCallOutputMessage | undefined;

                        return (
                          <div className="space-y-2">
                            {/* Arguments as JSON */}
                            <pre className="overflow-x-auto rounded-md border bg-neutral-100 p-3 font-mono text-neutral-700 text-xs dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-200">
                              {JSON.stringify(JSON.parse(functionCall.arguments), null, 2)}
                            </pre>

                            {/* Function output/result */}
                            {functionOutput && (
                              <div className="space-y-1">
                                <div className="flex items-center gap-2 font-medium text-neutral-600 text-xs dark:text-neutral-400">
                                  Result
                                </div>
                                <pre className="overflow-x-auto whitespace-pre-wrap rounded-md border bg-white p-3 text-neutral-700 text-xs dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-200">
                                  {functionOutput.output}
                                </pre>
                              </div>
                            )}
                          </div>
                        );
                      }

                      // For computer actions, show the action details as JSON
                      const action = (toolMessage as ComputerCallMessage).action;

                      return (
                        <div className="space-y-2">
                          {/* Action details as JSON */}
                          <pre className="overflow-x-auto rounded-md border bg-neutral-100 p-3 font-mono text-neutral-700 text-xs dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-200">
                            {JSON.stringify(action, null, 2)}
                          </pre>

                          {/* Screenshot */}
                          {screenshot?.output?.image_url ? (
                            <img
                              src={screenshot.output.image_url}
                              alt="Screenshot"
                              className="w-full rounded border dark:border-neutral-700"
                            />
                          ) : (
                            <div className="flex items-center justify-center rounded border bg-white p-8 text-neutral-500 text-sm dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-400">
                              No screenshot available
                            </div>
                          )}
                        </div>
                      );
                    })()}
                  </div>
                </div>
              )}
            </div>
          )}

          {/* Copy button - show ONLY if this is the last message in a group of consecutive non-user messages */}
          {isLastInGroup && (
            <button
              type="button"
              onClick={handleCopy}
              className="rounded-lg p-1.5 transition-all hover:bg-neutral-100 dark:hover:bg-neutral-800"
              title="Copy all messages"
            >
              {copied ? (
                <Check className="h-4 w-4 text-green-600 dark:text-green-400" />
              ) : (
                <Copy className="h-4 w-4 text-neutral-500 dark:text-neutral-400" />
              )}
            </button>
          )}

          {/* Duration subtext with animation */}
          {msgDurationSeconds !== undefined && (
            <div className="mt-1 flex items-center gap-2">
              <ThinkingCompleteAnimation />
              <p className="font-mono text-neutral-400 text-xs dark:text-neutral-500">
                cuala thought for {msgDurationSeconds.toFixed(1)} seconds.
              </p>
            </div>
          )}
        </div>
      );
    };

    const renderAssistantMessage = (message: AssistantMessage, index: string) => {
      return (
        <AssistantMessageComponent
          message={message}
          index={index}
          toolCalls={toolCalls}
          isLastInGroup={isLastInGroup}
          durationSeconds={durationSeconds}
        />
      );
    };

    const renderReasoningMessage = (message: ReasoningMessage, index: string) => {
      return (
        <BaseAssistantMessage className="w-full" index={index}>
          <div className="flex w-full flex-col">
            <button
              onClick={() => setIsReasoningExpanded(!isReasoningExpanded)}
              type="button"
              className="flex items-center gap-2 "
            >
              <Brain size={16} className="text-blue-600 dark:text-blue-400" />
              <span className="font-medium text-sm dark:text-white">Agent Reasoning</span>
              <motion.div
                animate={{ rotate: isReasoningExpanded ? 90 : 0 }}
                transition={{ duration: 0.2, ease: 'easeInOut' }}
                className="ml-auto"
              >
                <ChevronRight size={16} className="text-neutral-600 dark:text-neutral-400" />
              </motion.div>
            </button>

            <AnimatePresence>
              {isReasoningExpanded && (
                <motion.div
                  initial={{ height: 0, opacity: 0 }}
                  animate={{ height: 'auto', opacity: 1 }}
                  exit={{ height: 0 }}
                  transition={{
                    duration: 0.2,
                    ease: 'easeInOut',
                    opacity: { duration: 0.2 },
                  }}
                  className="overflow-hidden"
                >
                  <p className="flex-1 pt-2 text-neutral-600 text-sm dark:text-neutral-300">
                    {message.summary.map((s) => s.text).join(' ')}
                  </p>
                </motion.div>
              )}
            </AnimatePresence>
          </div>
        </BaseAssistantMessage>
      );
    };

    const FunctionCallComponent = ({
      message,
      index,
    }: {
      message: FunctionCallMessage;
      index: string;
    }) => {
      const [isExpanded, setIsExpanded] = useState(false);

      return (
        <BaseAssistantMessage index={index}>
          <div className="my-1 flex w-full flex-col gap-2">
            <button
              onClick={() => setIsExpanded(!isExpanded)}
              type="button"
              className="flex w-full items-center justify-between gap-2"
            >
              <div className="flex items-center gap-2">
                <Wrench size={16} className="text-blue-600 dark:text-blue-400" />
                <span className="font-medium text-sm dark:text-white">
                  Used tool {message.name}
                </span>
                <span
                  className={`flex items-center gap-1 rounded-md px-1 py-1 text-xs ${
                    message.status === 'completed'
                      ? 'bg-green-100 text-green-800 dark:bg-green-800 dark:text-green-100'
                      : message.status === 'failed'
                        ? 'bg-red-100 text-red-800 dark:bg-red-800 dark:text-red-100'
                        : 'bg-yellow-100 text-yellow-800 dark:bg-yellow-800 dark:text-yellow-100'
                  }`}
                >
                  {message.status === 'completed' && <CheckCircle size={12} />}
                  {message.status === 'failed' && <XCircle size={12} />}
                  {message.status === 'pending' && <Clock size={12} />}
                </span>
              </div>
              <motion.div
                animate={{ rotate: isExpanded ? 90 : 0 }}
                transition={{ duration: 0.2, ease: 'easeInOut' }}
              >
                <ChevronRight size={16} className="text-neutral-600 dark:text-neutral-400" />
              </motion.div>
            </button>

            <AnimatePresence>
              {isExpanded && (
                <motion.div
                  initial={{ height: 0, opacity: 0 }}
                  animate={{ height: 'auto', opacity: 1 }}
                  exit={{ height: 0, opacity: 0 }}
                  transition={{
                    duration: 0.2,
                    ease: 'easeInOut',
                    opacity: { duration: 0.2 },
                  }}
                  className="overflow-hidden"
                >
                  <pre className="overflow-x-auto rounded-md border bg-neutral-50 p-3 text-neutral-700 text-xs dark:bg-neutral-900 dark:text-neutral-300">
                    {JSON.stringify(JSON.parse(message.arguments), null, 2)}
                  </pre>
                </motion.div>
              )}
            </AnimatePresence>
          </div>
        </BaseAssistantMessage>
      );
    };

    const renderFunctionCall = (message: FunctionCallMessage, index: string) => (
      <FunctionCallComponent message={message} index={index} />
    );

    const renderFunctionOutput = (message: FunctionCallOutputMessage, index: string) => (
      <BaseAssistantMessage index={index}>
        <div className="my-1 flex flex-col gap-2">
          <div className="flex items-center gap-2">
            <Wrench size={16} className="text-blue-600 dark:text-blue-400" />
            <span className="font-medium text-sm dark:text-white">Function Result</span>
          </div>
          <pre className="whitespace-pre-wrap rounded-md border bg-neutral-50 p-3 text-neutral-700 text-sm dark:bg-neutral-900 dark:text-neutral-300">
            {message.output}
          </pre>
        </div>
      </BaseAssistantMessage>
    );

    const renderMessage = (message: AgentMessage, index: string) => {
      switch (message.type) {
        case 'message':
          return message.role === 'user' ||
            message.role === 'system' ||
            message.role === 'developer'
            ? renderUserMessage(message as UserMessage, index)
            : renderAssistantMessage(message as AssistantMessage, index);
        case 'reasoning':
          return renderReasoningMessage(message, index);
        case 'computer_call':
          // Computer calls are now rendered via ToolCallsGroup, not individually
          return null;
        case 'computer_call_output':
          // Don't render computer_call_output messages independently anymore
          return null;
        case 'function_call':
          return renderFunctionCall(message, index);
        case 'function_call_output':
          return renderFunctionOutput(message, index);
        default:
          return null;
      }
    };

    return <div>{renderMessage(message, `${message.type}-${Math.random()}`)}</div>;
  },
  // Custom comparison function - only re-render if props actually change
  (prevProps, nextProps) => {
    // Compare message by reference (messages are immutable in the chat)
    if (prevProps.message !== nextProps.message) return false;

    // Compare screenshot by reference
    if (prevProps.screenshot !== nextProps.screenshot) return false;

    // Compare toolCalls array by reference
    if (prevProps.toolCalls !== nextProps.toolCalls) return false;

    // Compare isLastInGroup
    if (prevProps.isLastInGroup !== nextProps.isLastInGroup) return false;

    // Compare isLastConsecutiveUserMessage
    if (prevProps.isLastConsecutiveUserMessage !== nextProps.isLastConsecutiveUserMessage)
      return false;

    // Compare durationSeconds
    if (prevProps.durationSeconds !== nextProps.durationSeconds) return false;

    // If all props are the same, don't re-render
    return true;
  }
);

ChatMessage.displayName = 'ChatMessage';

export const getActionDescription = (action: ComputerAction): React.ReactNode => {
  const baseClasses = 'inline-flex items-center gap-2 px-3 py-2 rounded-lg text-sm';
  const colorClasses =
    'bg-white text-neutral-800 border border-neutral-200 shadow-sm dark:bg-neutral-900 dark:text-neutral-100 dark:border-neutral-700';

  switch (action.type) {
    case 'click':
      return (
        <div className={`${baseClasses} ${colorClasses}`}>
          <Mouse className="h-4 w-4" />
          <span>Click {action.button}</span>
          <span className="rounded bg-neutral-100 px-2 py-0.5 font-mono text-xs dark:bg-neutral-700 dark:text-neutral-200">
            ({action.x}, {action.y})
          </span>
        </div>
      );

    case 'type':
      return (
        <div className={`${baseClasses} ${colorClasses}`}>
          <Keyboard className="h-4 w-4" />
          <span>Type:</span>
          <span className="rounded bg-neutral-100 px-2 py-0.5 font-mono text-xs dark:bg-neutral-700 dark:text-neutral-200">
            "{action.text}"
          </span>
        </div>
      );

    case 'scroll':
      return (
        <div className={`${baseClasses} ${colorClasses}`}>
          <Mouse className="h-4 w-4" />
          <span>Scroll</span>
          <span className="rounded bg-neutral-100 px-2 py-0.5 font-mono text-xs dark:bg-neutral-700 dark:text-neutral-200">
            ({action.scroll_x}, {action.scroll_y})
          </span>
          <span className="text-neutral-500 dark:text-neutral-400">at</span>
          <span className="rounded bg-neutral-100 px-2 py-0.5 font-mono text-xs dark:bg-neutral-700 dark:text-neutral-200">
            ({action.x}, {action.y})
          </span>
        </div>
      );

    case 'keypress':
      return (
        <div className={`${baseClasses} ${colorClasses}`}>
          <Keyboard className="h-4 w-4" />
          <span>Press keys:</span>
          <div className="flex gap-1">
            {action.keys?.map((key, index) => (
              <React.Fragment key={index}>
                <kbd className="rounded border border-neutral-300 bg-neutral-100 px-2 py-0.5 font-mono text-xs dark:border-neutral-600 dark:bg-neutral-700 dark:text-neutral-200">
                  {key}
                </kbd>
                {index < action.keys!.length - 1 && (
                  <span className="text-neutral-500 dark:text-neutral-400">+</span>
                )}
              </React.Fragment>
            ))}
          </div>
        </div>
      );

    case 'screenshot':
      return (
        <div className={`${baseClasses} ${colorClasses}`}>
          <Camera className="h-4 w-4" />
          <span>Take screenshot</span>
        </div>
      );

    case 'double_click':
      return (
        <div className={`${baseClasses} ${colorClasses}`}>
          <MousePointer className="h-4 w-4" />
          <span>Double click</span>
          <span className="rounded bg-neutral-100 px-2 py-0.5 font-mono text-xs dark:bg-neutral-700 dark:text-neutral-200">
            ({action.x}, {action.y})
          </span>
        </div>
      );

    case 'drag':
      return (
        <div className={`${baseClasses} ${colorClasses}`}>
          <Move className="h-4 w-4" />
          <span>Drag path with</span>
          <span className="rounded bg-neutral-100 px-2 py-0.5 text-xs dark:bg-neutral-700 dark:text-neutral-200">
            {action.path?.length} points
          </span>
        </div>
      );

    case 'move':
      return (
        <div className={`${baseClasses} ${colorClasses}`}>
          <Move className="h-4 w-4" />
          <span>Move to</span>
          <span className="rounded bg-neutral-100 px-2 py-0.5 font-mono text-xs dark:bg-neutral-700 dark:text-neutral-200">
            ({action.x}, {action.y})
          </span>
        </div>
      );

    case 'wait':
      return (
        <div className={`${baseClasses} ${colorClasses}`}>
          <Clock className="h-4 w-4" />
          <span>Wait</span>
        </div>
      );

    case 'left_mouse_down':
      return (
        <div className={`${baseClasses} ${colorClasses}`}>
          <ArrowDown className="h-4 w-4" />
          <span>Mouse down</span>
          <span className="rounded bg-neutral-100 px-2 py-0.5 font-mono text-xs dark:bg-neutral-700 dark:text-neutral-200">
            ({action.x}, {action.y})
          </span>
        </div>
      );

    case 'left_mouse_up':
      return (
        <div className={`${baseClasses} ${colorClasses}`}>
          <ArrowUp className="h-4 w-4" />
          <span>Mouse up</span>
          <span className="rounded bg-neutral-100 px-2 py-0.5 font-mono text-xs dark:bg-neutral-700 dark:text-neutral-200">
            ({action.x}, {action.y})
          </span>
        </div>
      );

    default:
      return (
        <div className={`${baseClasses} ${colorClasses}`}>
          <RotateCcw className="h-4 w-4" />
          <span>Computer action</span>
        </div>
      );
  }
};
