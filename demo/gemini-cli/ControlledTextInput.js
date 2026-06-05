// Custom TextInput with full Windows-style keyboard shortcuts
import React, { useState, useEffect, useRef } from 'react';
import { Text, useInput } from 'ink';
import chalk from 'chalk';

// Word boundary detection
function findWordBoundaryLeft(text, pos) {
  if (pos === 0) return 0;
  let i = pos - 1;
  // Skip whitespace
  while (i > 0 && /\s/.test(text[i])) i--;
  // Skip word characters
  while (i > 0 && !/\s/.test(text[i])) i--;
  return i === 0 && !/\s/.test(text[0]) ? 0 : i + 1;
}

function findWordBoundaryRight(text, pos) {
  if (pos >= text.length) return text.length;
  let i = pos;
  // Skip whitespace
  while (i < text.length && /\s/.test(text[i])) i++;
  // Skip word characters
  while (i < text.length && !/\s/.test(text[i])) i++;
  return i;
}

export default function ControlledTextInput({
  value: originalValue = '',
  placeholder = '',
  focus = true,
  showCursor = true,
  cursorOffset: externalCursorOffset,
  onChange,
  onSubmit,
  onCursorMove,
  onUpArrow,
  onDownArrow,
  onTab,
}) {
  const [state, setState] = useState({
    cursorOffset: externalCursorOffset ?? originalValue.length,
    selectionStart: null,
    selectionEnd: null,
  });

  const clipboardRef = useRef('');

  const { cursorOffset, selectionStart, selectionEnd } = state;

  // Sync with external cursor offset when provided
  useEffect(() => {
    if (externalCursorOffset !== undefined && externalCursorOffset !== cursorOffset) {
      setState(prev => ({ ...prev, cursorOffset: externalCursorOffset, selectionStart: null, selectionEnd: null }));
    }
  }, [externalCursorOffset]);

  // Reset cursor to end if value length changes and cursor is beyond end
  useEffect(() => {
    setState(previousState => {
      if (!focus || !showCursor) {
        return previousState;
      }

      const newValue = originalValue || '';
      if (previousState.cursorOffset > newValue.length) {
        const newOffset = newValue.length;
        if (onCursorMove) {
          onCursorMove(newOffset);
        }
        return { ...previousState, cursorOffset: newOffset, selectionStart: null, selectionEnd: null };
      }

      return previousState;
    });
  }, [originalValue, focus, showCursor]);

  const value = originalValue;
  const hasSelection = selectionStart !== null && selectionEnd !== null && selectionStart !== selectionEnd;

  let renderedValue = value;
  let renderedPlaceholder = placeholder ? chalk.grey(placeholder) : undefined;

  if (showCursor && focus) {
    renderedPlaceholder =
      placeholder.length > 0
        ? chalk.inverse(placeholder[0]) + chalk.grey(placeholder.slice(1))
        : chalk.inverse(' ');

    renderedValue = value.length > 0 ? '' : chalk.inverse(' ');

    if (hasSelection) {
      const selStart = Math.min(selectionStart, selectionEnd);
      const selEnd = Math.max(selectionStart, selectionEnd);

      for (let i = 0; i < value.length; i++) {
        if (i >= selStart && i < selEnd) {
          renderedValue += chalk.inverse(value[i]);
        } else {
          renderedValue += value[i];
        }
      }

      if (cursorOffset === value.length && selEnd === value.length) {
        renderedValue += chalk.inverse(' ');
      }
    } else {
      for (let i = 0; i < value.length; i++) {
        renderedValue += i === cursorOffset ? chalk.inverse(value[i]) : value[i];
      }

      if (value.length > 0 && cursorOffset === value.length) {
        renderedValue += chalk.inverse(' ');
      }
    }
  }

  useInput((input, key) => {
    // Allow callbacks to intercept these keys
    if (key.upArrow) {
      if (onUpArrow) {
        onUpArrow();
        return;
      }
    }
    if (key.downArrow) {
      if (onDownArrow) {
        onDownArrow();
        return;
      }
    }
    if (key.tab) {
      if (onTab) {
        onTab();
        return;
      }
    }

    // Skip Ctrl+C without selection (let terminal handle it)
    if (key.ctrl && input === 'c' && !hasSelection) {
      return;
    }

    // Skip Shift+Tab (reverse tab)
    if (key.shift && key.tab) {
      return;
    }

    if (key.return) {
      if (onSubmit) {
        onSubmit(originalValue);
      }
      return;
    }

    let nextCursorOffset = cursorOffset;
    let nextValue = originalValue;
    let nextSelectionStart = null;
    let nextSelectionEnd = null;

    // Ctrl+A: Select all
    if (key.ctrl && input === 'a') {
      nextSelectionStart = 0;
      nextSelectionEnd = originalValue.length;
      nextCursorOffset = originalValue.length;
    }
    // Ctrl+C: Copy
    else if (key.ctrl && input === 'c' && hasSelection) {
      const selStart = Math.min(selectionStart, selectionEnd);
      const selEnd = Math.max(selectionStart, selectionEnd);
      clipboardRef.current = originalValue.slice(selStart, selEnd);
      return;
    }
    // Ctrl+X: Cut
    else if (key.ctrl && input === 'x' && hasSelection) {
      const selStart = Math.min(selectionStart, selectionEnd);
      const selEnd = Math.max(selectionStart, selectionEnd);
      clipboardRef.current = originalValue.slice(selStart, selEnd);
      nextValue = originalValue.slice(0, selStart) + originalValue.slice(selEnd);
      nextCursorOffset = selStart;
      nextSelectionStart = null;
      nextSelectionEnd = null;
    }
    // Ctrl+V: Paste
    else if (key.ctrl && input === 'v') {
      if (hasSelection) {
        const selStart = Math.min(selectionStart, selectionEnd);
        const selEnd = Math.max(selectionStart, selectionEnd);
        nextValue = originalValue.slice(0, selStart) + clipboardRef.current + originalValue.slice(selEnd);
        nextCursorOffset = selStart + clipboardRef.current.length;
      } else {
        nextValue = originalValue.slice(0, cursorOffset) + clipboardRef.current + originalValue.slice(cursorOffset);
        nextCursorOffset = cursorOffset + clipboardRef.current.length;
      }
      nextSelectionStart = null;
      nextSelectionEnd = null;
    }
    // Home: Move to start
    else if (key.home) {
      if (key.shift) {
        nextSelectionStart = selectionStart === null ? cursorOffset : selectionStart;
        nextSelectionEnd = 0;
        nextCursorOffset = 0;
      } else {
        nextCursorOffset = 0;
        nextSelectionStart = null;
        nextSelectionEnd = null;
      }
    }
    // End: Move to end
    else if (key.end) {
      if (key.shift) {
        nextSelectionStart = selectionStart === null ? cursorOffset : selectionStart;
        nextSelectionEnd = originalValue.length;
        nextCursorOffset = originalValue.length;
      } else {
        nextCursorOffset = originalValue.length;
        nextSelectionStart = null;
        nextSelectionEnd = null;
      }
    }
    // Ctrl+Backspace OR Ctrl+Delete: Delete word backward (Windows reports both as ctrl+delete)
    else if (key.ctrl && (key.backspace || key.delete)) {
      if (hasSelection) {
        const selStart = Math.min(selectionStart, selectionEnd);
        const selEnd = Math.max(selectionStart, selectionEnd);
        nextValue = originalValue.slice(0, selStart) + originalValue.slice(selEnd);
        nextCursorOffset = selStart;
      } else {
        const wordStart = findWordBoundaryLeft(originalValue, cursorOffset);
        nextValue = originalValue.slice(0, wordStart) + originalValue.slice(cursorOffset);
        nextCursorOffset = wordStart;
      }
      nextSelectionStart = null;
      nextSelectionEnd = null;
    }
    // Left arrow
    else if (key.leftArrow) {
      if (key.ctrl) {
        // Ctrl+Left: Move by word
        const wordStart = findWordBoundaryLeft(originalValue, cursorOffset);
        if (key.shift) {
          nextSelectionStart = selectionStart === null ? cursorOffset : selectionStart;
          nextSelectionEnd = wordStart;
          nextCursorOffset = wordStart;
        } else {
          nextCursorOffset = wordStart;
          nextSelectionStart = null;
          nextSelectionEnd = null;
        }
      } else if (key.shift) {
        // Shift+Left: Select left
        if (cursorOffset > 0) {
          nextSelectionStart = selectionStart === null ? cursorOffset : selectionStart;
          nextSelectionEnd = cursorOffset - 1;
          nextCursorOffset = cursorOffset - 1;
        }
      } else {
        // Plain left
        if (hasSelection) {
          nextCursorOffset = Math.min(selectionStart, selectionEnd);
          nextSelectionStart = null;
          nextSelectionEnd = null;
        } else if (cursorOffset > 0) {
          nextCursorOffset = cursorOffset - 1;
        }
      }
    }
    // Right arrow
    else if (key.rightArrow) {
      if (key.ctrl) {
        // Ctrl+Right: Move by word
        const wordEnd = findWordBoundaryRight(originalValue, cursorOffset);
        if (key.shift) {
          nextSelectionStart = selectionStart === null ? cursorOffset : selectionStart;
          nextSelectionEnd = wordEnd;
          nextCursorOffset = wordEnd;
        } else {
          nextCursorOffset = wordEnd;
          nextSelectionStart = null;
          nextSelectionEnd = null;
        }
      } else if (key.shift) {
        // Shift+Right: Select right
        if (cursorOffset < originalValue.length) {
          nextSelectionStart = selectionStart === null ? cursorOffset : selectionStart;
          nextSelectionEnd = cursorOffset + 1;
          nextCursorOffset = cursorOffset + 1;
        }
      } else {
        // Plain right
        if (hasSelection) {
          nextCursorOffset = Math.max(selectionStart, selectionEnd);
          nextSelectionStart = null;
          nextSelectionEnd = null;
        } else if (cursorOffset < originalValue.length) {
          nextCursorOffset = cursorOffset + 1;
        }
      }
    }
    // Backspace OR Delete (Windows/Ink reports backspace as delete)
    else if (key.backspace || key.delete) {
      if (hasSelection) {
        // Delete selection (works for both backspace and delete)
        const selStart = Math.min(selectionStart, selectionEnd);
        const selEnd = Math.max(selectionStart, selectionEnd);
        nextValue = originalValue.slice(0, selStart) + originalValue.slice(selEnd);
        nextCursorOffset = selStart;
      } else if (cursorOffset > 0) {
        // Delete backward (backspace behavior)
        nextValue = originalValue.slice(0, cursorOffset - 1) + originalValue.slice(cursorOffset);
        nextCursorOffset = cursorOffset - 1;
      }
      nextSelectionStart = null;
      nextSelectionEnd = null;
    }
    // Regular input
    else if (input) {
      if (hasSelection) {
        const selStart = Math.min(selectionStart, selectionEnd);
        const selEnd = Math.max(selectionStart, selectionEnd);
        nextValue = originalValue.slice(0, selStart) + input + originalValue.slice(selEnd);
        nextCursorOffset = selStart + input.length;
      } else {
        nextValue = originalValue.slice(0, cursorOffset) + input + originalValue.slice(cursorOffset);
        nextCursorOffset = cursorOffset + input.length;
      }
      nextSelectionStart = null;
      nextSelectionEnd = null;
    }

    if (nextCursorOffset < 0) {
      nextCursorOffset = 0;
    }
    if (nextCursorOffset > nextValue.length) {
      nextCursorOffset = nextValue.length;
    }

    // Update local state for immediate visual feedback
    setState({
      cursorOffset: nextCursorOffset,
      selectionStart: nextSelectionStart,
      selectionEnd: nextSelectionEnd,
    });

    // Notify parent AFTER updating local state
    if (nextValue !== originalValue && onChange) {
      onChange(nextValue);
    }

    if (onCursorMove && nextCursorOffset !== cursorOffset) {
      onCursorMove(nextCursorOffset);
    }
  }, { isActive: focus });

  return <Text>{value.length > 0 ? renderedValue : renderedPlaceholder}</Text>;
}
