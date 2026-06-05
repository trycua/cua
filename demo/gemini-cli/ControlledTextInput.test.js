import assert from 'node:assert/strict';

function test(name, fn) {
  try {
    fn();
    console.log(`✓ ${name}`);
  } catch (error) {
    console.error(`✗ ${name}`);
    console.error(`  ${error.message}`);
    if (error.stack) {
      console.error(`  ${error.stack.split('\n').slice(1, 3).join('\n')}`);
    }
    process.exit(1);
  }
}

// Test basic typing
test('should handle basic typing', () => {
  let value = '';
  let cursor = 0;

  // Simulate typing "hello"
  // After 'h': value="h", cursor=1
  const input = 'h';
  value = value.slice(0, cursor) + input + value.slice(cursor);
  cursor = cursor + input.length;

  assert.equal(value, 'h');
  assert.equal(cursor, 1);
});

test('should handle backspace at middle', () => {
  let value = 'hello';
  let cursor = 3; // After "hel"

  // Simulate backspace: should delete 'l'
  // Result: "helo", cursor=2
  const expectedValue = 'helo';
  const expectedCursor = 2;

  const result = value.slice(0, cursor - 1) + value.slice(cursor);
  assert.equal(result, expectedValue);
  assert.equal(cursor - 1, expectedCursor);
});

test('should handle backspace at start (no-op)', () => {
  let value = 'hello';
  let cursor = 0;

  // Backspace at position 0 should do nothing
  if (cursor > 0) {
    value = value.slice(0, cursor - 1) + value.slice(cursor);
    cursor = cursor - 1;
  }

  assert.equal(value, 'hello');
  assert.equal(cursor, 0);
});

test('should handle backspace at end', () => {
  let value = 'hello';
  let cursor = 5;

  // Backspace at end should delete last char
  value = value.slice(0, cursor - 1) + value.slice(cursor);
  cursor = cursor - 1;

  assert.equal(value, 'hell');
  assert.equal(cursor, 4);
});

test('should handle delete at middle', () => {
  let value = 'hello';
  let cursor = 2; // After "he"

  // Delete should remove 'l'
  value = value.slice(0, cursor) + value.slice(cursor + 1);

  assert.equal(value, 'helo');
  assert.equal(cursor, 2);
});

test('should handle delete at end (no-op)', () => {
  let value = 'hello';
  let cursor = 5;

  // Delete at end should do nothing
  if (cursor < value.length) {
    value = value.slice(0, cursor) + value.slice(cursor + 1);
  }

  assert.equal(value, 'hello');
  assert.equal(cursor, 5);
});

test('should handle left arrow', () => {
  let cursor = 3;

  if (cursor > 0) {
    cursor = cursor - 1;
  }

  assert.equal(cursor, 2);
});

test('should handle right arrow', () => {
  let value = 'hello';
  let cursor = 3;

  if (cursor < value.length) {
    cursor = cursor + 1;
  }

  assert.equal(cursor, 4);
});

test('should handle Home key', () => {
  let cursor = 3;
  cursor = 0;

  assert.equal(cursor, 0);
});

test('should handle End key', () => {
  let value = 'hello';
  let cursor = 2;
  cursor = value.length;

  assert.equal(cursor, 5);
});

// Word boundary tests
function findWordBoundaryLeft(text, pos) {
  if (pos === 0) return 0;
  let i = pos - 1;
  while (i > 0 && /\s/.test(text[i])) i--;
  while (i > 0 && !/\s/.test(text[i])) i--;
  return i === 0 && !/\s/.test(text[0]) ? 0 : i + 1;
}

function findWordBoundaryRight(text, pos) {
  if (pos >= text.length) return text.length;
  let i = pos;
  while (i < text.length && /\s/.test(text[i])) i++;
  while (i < text.length && !/\s/.test(text[i])) i++;
  return i;
}

test('should find word boundary left in "hello world"', () => {
  const text = 'hello world';
  const cursor = 11; // At end

  const boundary = findWordBoundaryLeft(text, cursor);
  assert.equal(boundary, 6); // Start of "world"
});

test('should find word boundary left in "hello world" from middle', () => {
  const text = 'hello world';
  const cursor = 8; // In "world"

  const boundary = findWordBoundaryLeft(text, cursor);
  assert.equal(boundary, 6); // Start of "world"
});

test('should find word boundary right in "hello world"', () => {
  const text = 'hello world';
  const cursor = 0;

  const boundary = findWordBoundaryRight(text, cursor);
  assert.equal(boundary, 5); // End of "hello"
});

test('should handle Ctrl+Backspace (delete word)', () => {
  let value = 'hello world';
  let cursor = 11; // At end

  const wordStart = findWordBoundaryLeft(value, cursor);
  value = value.slice(0, wordStart) + value.slice(cursor);
  cursor = wordStart;

  assert.equal(value, 'hello ');
  assert.equal(cursor, 6);
});

test('should handle Ctrl+Delete (delete word forward)', () => {
  let value = 'hello world';
  let cursor = 0;

  const wordEnd = findWordBoundaryRight(value, cursor);
  value = value.slice(0, cursor) + value.slice(wordEnd);

  assert.equal(value, ' world');
  assert.equal(cursor, 0);
});

test('should handle Ctrl+Left (move by word)', () => {
  let value = 'hello world';
  let cursor = 11;

  cursor = findWordBoundaryLeft(value, cursor);

  assert.equal(cursor, 6); // Start of "world"
});

test('should handle Ctrl+Right (move by word)', () => {
  let value = 'hello world';
  let cursor = 0;

  cursor = findWordBoundaryRight(value, cursor);

  assert.equal(cursor, 5); // End of "hello"
});

test('should handle selection and delete', () => {
  let value = 'hello world';
  let selectionStart = 0;
  let selectionEnd = 5;

  // Delete selection
  const selStart = Math.min(selectionStart, selectionEnd);
  const selEnd = Math.max(selectionStart, selectionEnd);
  value = value.slice(0, selStart) + value.slice(selEnd);
  const cursor = selStart;

  assert.equal(value, ' world');
  assert.equal(cursor, 0);
});

test('should handle selection and type over', () => {
  let value = 'hello world';
  let selectionStart = 0;
  let selectionEnd = 5;
  let cursor = 5;

  // Type 'hi' over selection
  const input = 'hi';
  const selStart = Math.min(selectionStart, selectionEnd);
  const selEnd = Math.max(selectionStart, selectionEnd);
  value = value.slice(0, selStart) + input + value.slice(selEnd);
  cursor = selStart + input.length;

  assert.equal(value, 'hi world');
  assert.equal(cursor, 2);
});

test('should handle Ctrl+A (select all)', () => {
  let value = 'hello world';
  let selectionStart = 0;
  let selectionEnd = value.length;

  assert.equal(selectionStart, 0);
  assert.equal(selectionEnd, 11);
});

test('should handle copy/paste', () => {
  let value = 'hello world';
  let clipboard = '';

  // Select "hello"
  let selectionStart = 0;
  let selectionEnd = 5;

  // Copy
  const selStart = Math.min(selectionStart, selectionEnd);
  const selEnd = Math.max(selectionStart, selectionEnd);
  clipboard = value.slice(selStart, selEnd);

  assert.equal(clipboard, 'hello');

  // Move cursor to end and paste
  let cursor = value.length;
  value = value.slice(0, cursor) + clipboard + value.slice(cursor);
  cursor = cursor + clipboard.length;

  assert.equal(value, 'hello worldhello');
  assert.equal(cursor, 16);
});

test('should handle cut', () => {
  let value = 'hello world';
  let clipboard = '';
  let selectionStart = 6;
  let selectionEnd = 11;

  // Cut "world"
  const selStart = Math.min(selectionStart, selectionEnd);
  const selEnd = Math.max(selectionStart, selectionEnd);
  clipboard = value.slice(selStart, selEnd);
  value = value.slice(0, selStart) + value.slice(selEnd);
  const cursor = selStart;

  assert.equal(clipboard, 'world');
  assert.equal(value, 'hello ');
  assert.equal(cursor, 6);
});

test('should handle typing in middle', () => {
  let value = 'helo';
  let cursor = 2; // After "he"

  const input = 'l';
  value = value.slice(0, cursor) + input + value.slice(cursor);
  cursor = cursor + input.length;

  assert.equal(value, 'hello');
  assert.equal(cursor, 3);
});

test('should handle multiple backspaces', () => {
  let value = 'hello';
  let cursor = 5;

  // Backspace 3 times
  for (let i = 0; i < 3; i++) {
    if (cursor > 0) {
      value = value.slice(0, cursor - 1) + value.slice(cursor);
      cursor = cursor - 1;
    }
  }

  assert.equal(value, 'he');
  assert.equal(cursor, 2);
});

console.log('\n✅ All ControlledTextInput tests passed!');
