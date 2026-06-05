import assert from 'node:assert/strict';
import { getAutocompleteContext, applyCompletion, extractWindowMention } from './autocomplete.js';

const mockWindows = [
  { pid: 1, title: 'Spotify Premium', window_id: 100 },
  { pid: 2, title: 'Google Chrome', window_id: 200 },
  { pid: 3, title: 'Steam', window_id: 300 },
  { pid: 4, title: 'Visual Studio Code', window_id: 400 },
  { pid: 5, title: 'Settings', window_id: 500 },
];

function test(name, fn) {
  try {
    fn();
    console.log(`✓ ${name}`);
  } catch (error) {
    console.error(`✗ ${name}`);
    console.error(`  ${error.message}`);
    process.exit(1);
  }
}

// Test getAutocompleteContext
test('should find @ at start of line', () => {
  const result = getAutocompleteContext('@S', 2, mockWindows);
  assert(result !== null);
  assert.equal(result.atPos, 0);
  assert.equal(result.query, 'S');
  assert.equal(result.matches.length, 4); // Spotify, Steam, Settings, Visual Studio Code
});

test('should find @ in middle of line', () => {
  const result = getAutocompleteContext('press play on @Spot', 20, mockWindows);
  assert(result !== null);
  assert.equal(result.atPos, 14);
  assert.equal(result.query, 'Spot');
  assert.equal(result.matches.length, 1); // Spotify Premium
  assert.equal(result.matches[0].title, 'Spotify Premium');
});

test('should handle quoted window names', () => {
  const result = getAutocompleteContext('open @"Visual', 13, mockWindows);
  assert(result !== null);
  assert.equal(result.atPos, 5);
  assert.equal(result.query, 'Visual');
  assert.equal(result.hasQuote, true);
  assert.equal(result.matches[0].title, 'Visual Studio Code');
});

test('should handle cursor in middle of @mention', () => {
  const result = getAutocompleteContext('click @Spot here', 11, mockWindows);
  assert(result !== null);
  assert.equal(result.atPos, 6);
  assert.equal(result.query, 'Spot');
});

test('should return null when no @ before cursor', () => {
  const result = getAutocompleteContext('hello world', 5, mockWindows);
  assert.equal(result, null);
});

test('should return null when @ is after cursor', () => {
  const result = getAutocompleteContext('hello @world', 5, mockWindows);
  assert.equal(result, null);
});

test('should handle cursor right after @', () => {
  const result = getAutocompleteContext('press @', 7, mockWindows);
  assert(result !== null);
  assert.equal(result.query, '');
  assert.equal(result.matches.length, 5); // All windows
});

test('should filter case-insensitively', () => {
  const result = getAutocompleteContext('@steam', 6, mockWindows);
  assert.equal(result.matches.length, 1);
  assert.equal(result.matches[0].title, 'Steam');
});

test('should handle partial matches', () => {
  const result = getAutocompleteContext('@Goo', 4, mockWindows);
  assert.equal(result.matches.length, 1);
  assert.equal(result.matches[0].title, 'Google Chrome');
});

// Test applyCompletion
test('should replace simple @mention with cursor at end', () => {
  const { text, cursorPos } = applyCompletion(
    'click @S here',
    8, // cursor after @S
    mockWindows[4] // Settings
  );
  assert.equal(text, 'click @Settings here');
  assert.equal(cursorPos, 15); // After @Settings
});

test('should add quotes for multi-word titles', () => {
  const { text, cursorPos } = applyCompletion(
    'open @Spot',
    10, // cursor at end
    mockWindows[0] // Spotify Premium
  );
  assert.equal(text, 'open @"Spotify Premium"');
  assert.equal(cursorPos, 23); // Position after @"Spotify Premium"
});

test('should replace existing quoted mention', () => {
  const input = 'click @"Visual Studio Code" button';
  const { text, cursorPos } = applyCompletion(
    input,
    20, // cursor in middle of quoted name
    mockWindows[2] // Steam
  );
  assert.equal(text, 'click @Steam button');
  assert.equal(cursorPos, 12);
});

test('should handle @ at start', () => {
  const { text, cursorPos } = applyCompletion(
    '@S',
    2, // cursor after @S
    mockWindows[4] // Settings
  );
  assert.equal(text, '@Settings');
  assert.equal(cursorPos, 9);
});

test('should handle @ at end', () => {
  const { text, cursorPos } = applyCompletion(
    'hello @S',
    8, // cursor at end
    mockWindows[2] // Steam
  );
  assert.equal(text, 'hello @Steam');
  assert.equal(cursorPos, 12);
});

test('should handle cursor in middle of @mention', () => {
  const { text, cursorPos } = applyCompletion(
    'click @Spot here',
    10, // cursor after @Spo
    mockWindows[0] // Spotify Premium
  );
  assert.equal(text, 'click @"Spotify Premium" here');
  assert.equal(cursorPos, 24); // After @"Spotify Premium"
});

test('should place cursor at end of completion with trailing text', () => {
  const { text, cursorPos } = applyCompletion(
    'open @S and close it',
    7, // cursor after @S
    mockWindows[4] // Settings
  );
  assert.equal(text, 'open @Settings and close it');
  assert.equal(cursorPos, 14); // After @Settings, before " and"
});

test('should place cursor at end when completing partial quoted name', () => {
  const { text, cursorPos } = applyCompletion(
    'type in @"Vis',
    13, // cursor at end
    mockWindows[3] // Visual Studio Code
  );
  const expectedText = 'type in @"Visual Studio Code"';
  assert.equal(text, expectedText);
  assert.equal(cursorPos, expectedText.length); // After closing quote (end of string)
});

test('should place cursor correctly when cursor in middle with text after', () => {
  const { text, cursorPos } = applyCompletion(
    'press @Ste play button',
    10, // cursor after @Ste
    mockWindows[2] // Steam
  );
  assert.equal(text, 'press @Steam play button');
  assert.equal(cursorPos, 12); // After @Steam
});

test('should insert @mention when no @ before cursor', () => {
  const { text, cursorPos } = applyCompletion(
    'hello world',
    5, // cursor after "hello"
    mockWindows[2] // Steam
  );
  assert.equal(text, 'hello@Steam world');
  assert.equal(cursorPos, 11);
});

// Test extractWindowMention
test('should extract simple window mention', () => {
  const result = extractWindowMention('@Steam');
  assert.equal(result.query, 'Steam');
  assert.equal(result.fullMatch, '@Steam');
});

test('should extract quoted window mention', () => {
  const result = extractWindowMention('open @"Spotify Premium" now');
  assert.equal(result.query, 'Spotify Premium');
  assert.equal(result.fullMatch, '@"Spotify Premium"');
});

test('should extract first mention if multiple', () => {
  const result = extractWindowMention('@Steam and @Chrome');
  assert.equal(result.query, 'Steam');
});

test('should return null when no mention', () => {
  const result = extractWindowMention('hello world');
  assert.equal(result, null);
});

test('should handle mention at start', () => {
  const result = extractWindowMention('@Chrome is open');
  assert.equal(result.query, 'Chrome');
});

test('should handle mention at end', () => {
  const result = extractWindowMention('click @Steam');
  assert.equal(result.query, 'Steam');
});

console.log('\n✅ All autocomplete tests passed!');
