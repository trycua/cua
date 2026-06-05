// Autocomplete logic for @ window mentions
// Extracts @mention at cursor position and provides completions

export function getAutocompleteContext(text, cursorPos, windows) {
  // Find @ before cursor
  let atPos = -1;
  for (let i = cursorPos - 1; i >= 0; i--) {
    if (text[i] === '@') {
      atPos = i;
      break;
    }
    // Stop at whitespace (unless we're inside quotes)
    if (text[i] === ' ' && !isInsideQuotes(text, i)) {
      break;
    }
  }

  if (atPos === -1) {
    return null; // No @ before cursor
  }

  // Extract the query after @
  const afterAt = text.slice(atPos + 1, cursorPos);

  // Check if we're inside quotes
  let query = afterAt;
  let hasQuote = false;
  if (afterAt.startsWith('"')) {
    hasQuote = true;
    query = afterAt.slice(1);
  }

  // Find where the @mention ends (for replacement)
  let endPos = atPos + 1; // Start after @
  if (hasQuote) {
    // Find closing quote from current position
    const closeQuote = text.indexOf('"', atPos + 2);
    if (closeQuote > atPos) {
      endPos = closeQuote + 1;
    } else {
      // No closing quote yet, end at cursor
      endPos = cursorPos;
    }
  } else {
    // Find next space or end, starting from @
    const nextSpace = text.indexOf(' ', atPos + 1);
    endPos = nextSpace > atPos ? nextSpace : text.length;
  }

  // Filter windows by query
  const matches = windows.filter(w =>
    w.title.toLowerCase().includes(query.toLowerCase())
  );

  return {
    atPos,
    endPos,
    query,
    hasQuote,
    matches
  };
}

function isInsideQuotes(text, pos) {
  let quoteCount = 0;
  for (let i = 0; i < pos; i++) {
    if (text[i] === '"') quoteCount++;
  }
  return quoteCount % 2 === 1;
}

export function applyCompletion(text, cursorPos, selectedWindow) {
  // Get the context to find where to replace
  const context = getAutocompleteContext(text, cursorPos, [selectedWindow]);

  if (!context) {
    // No @ before cursor, just insert @window at cursor
    const windowName = selectedWindow.title.includes(' ')
      ? `@"${selectedWindow.title}"`
      : `@${selectedWindow.title}`;

    const newText = text.slice(0, cursorPos) + windowName + text.slice(cursorPos);
    const newCursorPos = cursorPos + windowName.length;

    return { text: newText, cursorPos: newCursorPos };
  }

  // Replace the @mention
  const before = text.slice(0, context.atPos);
  const after = text.slice(context.endPos);
  const windowName = selectedWindow.title.includes(' ')
    ? `"${selectedWindow.title}"`
    : selectedWindow.title;

  const newText = `${before}@${windowName}${after}`;
  const newCursorPos = context.atPos + 1 + windowName.length;

  return { text: newText, cursorPos: newCursorPos };
}

// Extract @window mention from text (for window selection)
export function extractWindowMention(text) {
  const match = text.match(/@"([^"]+)"|@(\S+)/);
  if (!match) return null;

  return {
    query: match[1] || match[2],
    fullMatch: match[0]
  };
}
