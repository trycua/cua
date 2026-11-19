/**
 * House style for generated docs: no em dashes (U+2014).
 *
 * Source strings (tool descriptions, CLI help) may still contain them, so every
 * generator passes its final output through `normalizeEmDashes`. The rewrite is
 * deterministic, so `--check` stays stable across hosts:
 *
 * - an empty table cell `| — |` becomes `| none |`;
 * - two dashes in one sentence (`a — b — c`) are a parenthetical: `a, b, c`;
 * - a single dash (`a — b`) becomes a colon, or a semicolon when the sentence
 *   already has a colon before it;
 * - any remaining em dash (no surrounding spaces) becomes a comma and a space.
 *
 * `scripts/check-no-em-dash.sh` enforces the rule on the committed pages.
 */

const EM = '—';

export function normalizeEmDashes(text: string): string {
  if (!text.includes(EM)) return text;
  return text
    .split('\n')
    .map((line) => normalizeLine(line))
    .join('\n');
}

function normalizeLine(line: string): string {
  if (!line.includes(EM)) return line;
  // Empty table cells (loop: adjacent cells share a pipe).
  let out = line;
  for (let prev = ''; prev !== out; ) {
    prev = out;
    out = out.replace(/\|\s*—\s*\|/g, '| none |');
  }
  // Work sentence by sentence so pairs are detected per sentence.
  out = out
    .split(/(?<=[.!?])(?=\s)/)
    .map((sentence) => normalizeSentence(sentence))
    .join('');
  return out.replace(/\s*—\s*/g, ', ');
}

function normalizeSentence(sentence: string): string {
  const spaced = ` ${EM} `;
  const count = sentence.split(spaced).length - 1;
  if (count === 0) return sentence;
  if (count >= 2) return sentence.split(spaced).join(', ');
  const index = sentence.indexOf(spaced);
  const before = sentence.slice(0, index);
  const separator = before.includes(': ') ? '; ' : ': ';
  return before + separator + sentence.slice(index + spaced.length);
}
