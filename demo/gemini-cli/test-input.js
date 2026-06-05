#!/usr/bin/env node
import React, { useState } from 'react';
import { render, Box, Text } from 'ink';
import ControlledTextInput from './ControlledTextInput.js';

function TestApp() {
  const [value, setValue] = useState('hello world');
  const [cursor, setCursor] = useState(11);

  return (
    <Box flexDirection="column">
      <Text>Test ControlledTextInput - press backspace to test</Text>
      <Text>Value: "{value}" (length: {value.length})</Text>
      <Text>Cursor: {cursor}</Text>
      <Box marginTop={1}>
        <Text>&gt; </Text>
        <ControlledTextInput
          value={value}
          cursorOffset={cursor}
          onChange={(v) => {
            console.log('onChange called:', v);
            setValue(v);
          }}
          onCursorMove={(c) => {
            console.log('onCursorMove called:', c);
            setCursor(c);
          }}
        />
      </Box>
    </Box>
  );
}

render(<TestApp />);
