#!/usr/bin/env node

import React, { useState, useEffect } from 'react';
import { render, Text, Box, useInput, useStdout } from 'ink';
import ControlledTextInput from './ControlledTextInput.js';
import { getAutocompleteContext, applyCompletion, extractWindowMention } from './autocomplete.js';
import { initCuaDriver, handleComputerAction, selectWindow, getSelectedWindow } from './gemini-client.js';

// Clear screen
process.stdout.write('\x1Bc');

function App() {
  const { stdout } = useStdout();
  const terminalWidth = stdout?.columns || 80;

  const [input, setInput] = useState('');
  const [cursorPos, setCursorPos] = useState(0);
  const [messages, setMessages] = useState([]);
  const [ready, setReady] = useState(false);
  const [isProcessing, setIsProcessing] = useState(false);
  const [windows, setWindows] = useState([]);
  const [autocompleteState, setAutocompleteState] = useState(null);
  const [selectedCompletionIndex, setSelectedCompletionIndex] = useState(0);

  useEffect(() => {
    initCuaDriver().then(wins => {
      setWindows(wins);
      setReady(true);
    });
  }, []);

  // Update autocomplete context when input or cursor changes
  useEffect(() => {
    if (windows.length === 0) return;

    const context = getAutocompleteContext(input, cursorPos, windows);
    if (context && context.matches.length > 0) {
      setAutocompleteState(context);
      setSelectedCompletionIndex(0);
    } else {
      setAutocompleteState(null);
    }
  }, [input, cursorPos, windows]);

  const handleInputChange = (value) => {
    setInput(value);
  };

  const handleCursorMove = (newCursorPos) => {
    setCursorPos(newCursorPos);
  };

  const handleUpArrow = () => {
    if (autocompleteState && autocompleteState.matches.length > 0) {
      setSelectedCompletionIndex(prev =>
        prev === 0 ? autocompleteState.matches.length - 1 : prev - 1
      );
    }
  };

  const handleDownArrow = () => {
    if (autocompleteState && autocompleteState.matches.length > 0) {
      setSelectedCompletionIndex(prev =>
        (prev + 1) % autocompleteState.matches.length
      );
    }
  };

  const handleTab = () => {
    if (autocompleteState && autocompleteState.matches.length > 0) {
      const selected = autocompleteState.matches[selectedCompletionIndex];
      const result = applyCompletion(input, cursorPos, selected);
      setInput(result.text);
      setCursorPos(result.cursorPos);
    }
  };

  const handleSubmit = async () => {
    if (!input.trim()) return;

    const fullInput = input;
    setInput(''); // Clear input immediately

    // Check for @window mention
    const mention = extractWindowMention(fullInput);
    if (mention && !getSelectedWindow()) {
      const matches = windows.filter(w =>
        w.title.toLowerCase().includes(mention.query.toLowerCase())
      );

      if (matches.length === 1) {
        await selectWindow(matches[0]);
        setMessages(prev => [...prev, {
          role: 'system',
          text: `📍 Selected: ${matches[0].title}`
        }]);

        // Send the full input including @mention to the agent
        if (fullInput.trim()) {
          await processCommand(fullInput);
          return;
        }
      } else if (matches.length > 1) {
        setMessages(prev => [...prev, {
          role: 'system',
          text: `Multiple matches found. Please be more specific.`
        }]);
      } else {
        setMessages(prev => [...prev, {
          role: 'system',
          text: `No window matching "${mention.query}"`
        }]);
      }

      return;
    }

    if (!getSelectedWindow()) {
      setMessages(prev => [...prev, {
        role: 'system',
        text: 'Select a window first with @window-name'
      }]);
      return;
    }

    await processCommand(fullInput);
  };

  const processCommand = async (command) => {
    setIsProcessing(true);
    setMessages(prev => [...prev, { role: 'user', text: command }]);

    try {
      const result = await handleComputerAction(command, (toolCall) => {
        // Stream tool calls as they happen
        const { intent, ...argsWithoutIntent } = toolCall.args || {};
        const intentText = intent ? `💭 ${intent}\n` : '';
        const argsStr = Object.keys(argsWithoutIntent).length ? ` ${JSON.stringify(argsWithoutIntent)}` : '';
        setMessages(prev => [...prev, {
          role: 'tool',
          text: `${intentText}🔧 ${toolCall.name}${argsStr}`
        }]);
      });

      if (result.text) {
        setMessages(prev => [...prev, {
          role: 'assistant',
          text: result.text
        }]);
      }

      if (result.error) {
        setMessages(prev => [...prev, {
          role: 'error',
          text: result.error
        }]);
      }
    } catch (error) {
      setMessages(prev => [...prev, {
        role: 'error',
        text: error.message
      }]);
    }

    setIsProcessing(false);
  };

  if (!ready) {
    return <Text>Connecting to cua-driver...</Text>;
  }

  const selectedWindow = getSelectedWindow();

  return (
    <Box flexDirection="column" height="100%">
      {/* Chat history */}
      <Box flexDirection="column" flexGrow={1}>
        {messages.slice(-20).map((msg, i) => (
          <Box key={i}>
            {msg.role === 'user' && <Text color="blue">You: {msg.text}</Text>}
            {msg.role === 'assistant' && <Text>Gemini: {msg.text}</Text>}
            {msg.role === 'tool' && <Text color="magenta">{msg.text}</Text>}
            {msg.role === 'system' && <Text color="yellow">{msg.text}</Text>}
            {msg.role === 'error' && <Text color="red">Error: {msg.text}</Text>}
          </Box>
        ))}
        {isProcessing && <Text color="gray">Thinking...</Text>}
      </Box>

      {/* Separator with window title */}
      <Box marginTop={1}>
        {selectedWindow ? (
          <Text color="gray">
            {'─'}
            <Text color="green">{selectedWindow.title}</Text>
            {'─'.repeat(Math.max(0, terminalWidth - selectedWindow.title.length - 2))}
          </Text>
        ) : (
          <Text color="gray">{'─'.repeat(terminalWidth)}</Text>
        )}
      </Box>

      {/* Input */}
      <Box width={terminalWidth}>
        <Text>&gt; </Text>
        <ControlledTextInput
          value={input}
          cursorOffset={cursorPos}
          onChange={handleInputChange}
          onCursorMove={handleCursorMove}
          onSubmit={handleSubmit}
          onUpArrow={handleUpArrow}
          onDownArrow={handleDownArrow}
          onTab={handleTab}
        />
      </Box>

      {/* Bottom separator */}
      <Box>
        <Text color="gray">{'─'.repeat(terminalWidth)}</Text>
      </Box>

      {/* Autocomplete dropdown */}
      {autocompleteState && autocompleteState.matches.length > 0 && (
        <Box flexDirection="column" paddingLeft={2}>
          {autocompleteState.matches.slice(0, 10).map((win, i) => {
            const isSelected = i === selectedCompletionIndex;
            const displayName = win.title.includes(' ') ? `"${win.title}"` : win.title;

            return (
              <Box key={i}>
                <Text color={isSelected ? 'cyan' : 'gray'}>
                  {isSelected ? '▸ ' : '  '}@{displayName}
                </Text>
              </Box>
            );
          })}
          {autocompleteState.matches.length > 10 && (
            <Text color="gray">  ... {autocompleteState.matches.length - 10} more</Text>
          )}
        </Box>
      )}

      {/* Status line */}
      <Box>
        <Text color="gray" dimColor>
          {selectedWindow
            ? '↑↓ navigate • Tab complete • Enter send'
            : `${windows.length} windows available • Type @window-name to select`}
        </Text>
      </Box>
    </Box>
  );
}

render(<App />);
