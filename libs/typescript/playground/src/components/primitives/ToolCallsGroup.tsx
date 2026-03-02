import { AnimatePresence, motion } from 'motion/react';
import { Brain, Settings, TriangleAlert, Wrench } from 'lucide-react';
import type React from 'react';
import { useState } from 'react';
import type {
  AgentMessage,
  ComputerCallMessage,
  ComputerCallOutputMessage,
  FunctionCallMessage,
  ReasoningMessage,
} from '../../types';
import { getActionDescription } from './ChatMessage';

interface ToolCallsGroupProps {
  toolCalls: {
    message: AgentMessage;
    screenshot?: ComputerCallOutputMessage;
  }[];
}

export const ToolCallsGroup: React.FC<ToolCallsGroupProps> = ({ toolCalls }) => {
  const [isExpanded, setIsExpanded] = useState(false);
  const [selectedCallIndex, setSelectedCallIndex] = useState(1);

  const getCallDescription = (message: AgentMessage): React.ReactNode => {
    if (message.type === 'computer_call') {
      const computerCall = message as ComputerCallMessage;
      return getActionDescription(computerCall.action);
    }
    if (message.type === 'function_call') {
      const functionCall = message as FunctionCallMessage;
      return (
        <>
          <p className="mb-2 flex items-center gap-2 py-0.5 text-black text-sm dark:border-white dark:text-white">
            <Wrench className="h-4 w-4" /> Function:
            <span>{functionCall.name}</span>
          </p>
          <p className="rounded-md border bg-neutral-50 p-2 font-mono text-neutral-700 text-xs dark:text-neutral-200">
            {functionCall.arguments}
          </p>
        </>
      );
    }
    if (message.type === 'reasoning') {
      const reasoningCall = message as ReasoningMessage;
      return (
        <>
          <p className="mb-2 flex items-center gap-2 py-0.5 text-black text-xs dark:border-white dark:text-white">
            <Brain className="h-4 w-4" /> Reasoning
          </p>
          <p className="rounded-md border bg-neutral-50 p-2 text-neutral-700 text-sm dark:bg-neutral-700 dark:text-neutral-200">
            {reasoningCall.summary.map((s) => s.text).join(' ')}
          </p>
        </>
      );
    }
    return (
      <div className="inline-flex items-center gap-2 rounded-lg border border-neutral-200 bg-neutral-50 px-3 py-1.5 font-medium text-neutral-700 text-sm dark:border-neutral-600 dark:bg-neutral-800 dark:text-neutral-200">
        <TriangleAlert className="h-4 w-4" />
        <span className="text-sm">Unknown call</span>
      </div>
    );
  };

  const selectedScreenshot = toolCalls[selectedCallIndex]?.screenshot;

  return (
    <div className="flex flex-col items-start gap-2">
      {/* Compact tool button with badge */}
      <button
        onClick={() => setIsExpanded(!isExpanded)}
        className="group relative flex items-center gap-1.5 rounded-lg p-1.5 transition-all hover:bg-neutral-100 dark:hover:bg-neutral-800"
        type="button"
        title={`${toolCalls.length} tool call${toolCalls.length > 1 ? 's' : ''}`}
      >
        <Wrench className="h-4 w-4 text-neutral-500 dark:text-neutral-400" />
        {/* Badge with count */}
        <span className="flex h-4 min-w-4 items-center justify-center rounded-full bg-blue-600 px-1 text-white text-xs dark:bg-blue-500">
          {toolCalls.length}
        </span>
      </button>

      {/* Expanded tool calls panel */}
      <AnimatePresence>
        {isExpanded && (
          <motion.div
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: 'auto', opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={{
              duration: 0.2,
              ease: 'easeInOut',
            }}
            className="w-full overflow-hidden rounded-lg border border-neutral-300 bg-neutral-100 dark:border-neutral-400 dark:bg-neutral-800"
          >
            <div className="flex items-center gap-2 border-neutral-200 border-b px-4 py-2 dark:border-neutral-600">
              <Settings size={16} className="text-blue-600 dark:text-blue-400" />
              <span className="font-medium text-sm dark:text-white">
                Tool Calls ({toolCalls.length})
              </span>
            </div>
            <div className="border-neutral-200 border-t dark:border-neutral-600">
              <div className="flex h-96 p-4">
                {/* Screenshot display area */}
                <div className="mr-4 flex flex-1 items-center justify-center rounded-lg border border-neutral-200 bg-neutral-50 dark:border-neutral-700 dark:bg-neutral-900">
                  {selectedScreenshot?.output?.image_url ? (
                    <img
                      src={selectedScreenshot.output.image_url}
                      alt="Screenshot"
                      className="max-h-full max-w-full rounded object-contain"
                    />
                  ) : (
                    <div className="text-neutral-500 text-sm dark:text-neutral-400">
                      No screenshot available
                    </div>
                  )}
                </div>

                {/* Scrollable calls list */}
                <div className="flex w-72 flex-col">
                  <div className="flex-1 space-y-3 overflow-y-auto px-1">
                    {toolCalls.map(({ message }, index) => (
                      <button
                        key={`call-${index}`}
                        type="button"
                        onClick={() => setSelectedCallIndex(index)}
                        disabled={message.type === 'reasoning'}
                        className={`w-full rounded border p-2 text-left text-neutral-700 transition-colors disabled:cursor-default! dark:text-neutral-300 ${
                          selectedCallIndex === index
                            ? 'border-blue-300 bg-blue-100 dark:border-blue-700 dark:bg-blue-900 '
                            : 'border-neutral-200 bg-white hover:enabled:bg-neutral-50 dark:border-neutral-600 dark:bg-neutral-800 dark:hover:enabled:bg-neutral-700'
                        }`}
                      >
                        <div className="">{getCallDescription(message)}</div>
                      </button>
                    ))}
                  </div>
                </div>
              </div>
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
};
