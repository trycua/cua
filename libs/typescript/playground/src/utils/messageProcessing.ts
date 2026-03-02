import type { AgentMessage, ComputerCallOutputMessage } from '../types';

// Process messages to group tool calls and add screenshots
export const processMessagesForRendering = (messages: AgentMessage[]) => {
  const processedMessages: Array<{
    type: 'single';
    message: AgentMessage;
    screenshot?: ComputerCallOutputMessage;
    toolCalls?: {
      message: AgentMessage;
      screenshot?: ComputerCallOutputMessage;
      output?: AgentMessage;
    }[];
  }> = [];

  const createPlaceholderAssistantMessage = (): AgentMessage => ({
    content: [],
    role: 'assistant',
    type: 'message',
  });

  const isToolCallMessage = (message: AgentMessage) => {
    return (
      message.type === 'computer_call' ||
      message.type === 'function_call' ||
      message.type === 'reasoning'
    );
  };

  const isToolCallOutputMessage = (message: AgentMessage) => {
    return message.type === 'computer_call_output' || message.type === 'function_call_output';
  };

  for (let i = 0; i < messages.length; i++) {
    const message = messages[i];

    if (isToolCallMessage(message)) {
      // Start collecting consecutive tool calls
      const toolCalls: {
        message: AgentMessage;
        screenshot?: ComputerCallOutputMessage;
        output?: AgentMessage;
      }[] = [];

      // Process current message and any consecutive tool calls
      let j = i;
      while (
        j < messages.length &&
        (isToolCallMessage(messages[j]) || isToolCallOutputMessage(messages[j]))
      ) {
        const currentMessage = messages[j];

        if (currentMessage.type === 'computer_call') {
          // Look for matching computer_call_output
          const nextMessage = messages[j + 1];
          let screenshot: ComputerCallOutputMessage | undefined;

          if (
            nextMessage &&
            nextMessage.type === 'computer_call_output' &&
            (nextMessage as ComputerCallOutputMessage).call_id === (currentMessage as any).call_id
          ) {
            screenshot = nextMessage as ComputerCallOutputMessage;
            j++; // Skip the output message
          }

          toolCalls.push({ message: currentMessage, screenshot });
        } else if (currentMessage.type === 'function_call') {
          // Look for matching function_call_output and store it with the function_call
          const nextMessage = messages[j + 1];
          let output: AgentMessage | undefined;

          if (
            nextMessage &&
            nextMessage.type === 'function_call_output' &&
            (nextMessage as any).call_id === (currentMessage as any).call_id
          ) {
            output = nextMessage;
            j++; // Skip the output message
          }

          toolCalls.push({ message: currentMessage, output });
        } else if (currentMessage.type === 'computer_call_output') {
          // Skip standalone output messages (should have been grouped)
          const prevMessage = messages[j - 1];
          if (
            !prevMessage ||
            prevMessage.type !== 'computer_call' ||
            (prevMessage as any).call_id !== (currentMessage as any).call_id
          ) {
            toolCalls.push({ message: currentMessage });
          }
        } else if (currentMessage.type === 'function_call_output') {
          // Skip standalone output messages (should have been grouped)
          const prevMessage = messages[j - 1];
          if (
            !prevMessage ||
            prevMessage.type !== 'function_call' ||
            (prevMessage as any).call_id !== (currentMessage as any).call_id
          ) {
            toolCalls.push({ message: currentMessage });
          }
        } else if (currentMessage.type === 'reasoning') {
          toolCalls.push({ message: currentMessage });
        }

        j++;
      }

      // If we have tool calls, attach them to the previous assistant message
      if (toolCalls.length > 0) {
        const lastMessage = processedMessages[processedMessages.length - 1];

        // Attach to previous message if it exists and is NOT a user message; otherwise, insert a placeholder
        const isPreviousUser =
          !!lastMessage &&
          lastMessage.type === 'single' &&
          lastMessage.message.type === 'message' &&
          (lastMessage.message as any).role === 'user';

        if (lastMessage && lastMessage.type === 'single' && !isPreviousUser) {
          lastMessage.toolCalls = toolCalls;
        } else {
          processedMessages.push({
            type: 'single',
            message: createPlaceholderAssistantMessage(),
            toolCalls,
          });
        }
      }

      i = j - 1; // Update main loop index
    } else {
      // Non-tool call message
      processedMessages.push({
        type: 'single',
        message,
      });
    }
  }

  return processedMessages;
};
