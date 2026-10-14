/**
 * Small, deterministic readers for Rust and Go source, for generators whose
 * source of truth is code rather than a dump (the read-only `libs/fleet`
 * mirror cannot grow a `dump-docs` command).
 *
 * They only handle the shapes they are used on and throw when a shape they
 * rely on is missing, so a refactor of the source fails the generator loudly
 * instead of silently dropping entries.
 */

/** Index of the brace that closes the one at `open` (skips strings, chars and comments). */
export function matchBrace(src: string, open: number): number {
  const pairs: Record<string, string> = { '{': '}', '(': ')', '[': ']' };
  const stack: string[] = [];
  for (let i = open; i < src.length; i += 1) {
    const c = src[i];
    if (c === '/' && src[i + 1] === '/') {
      i = src.indexOf('\n', i);
      if (i < 0) break;
      continue;
    }
    if (c === '/' && src[i + 1] === '*') {
      i = src.indexOf('*/', i) + 1;
      continue;
    }
    if (c === '"') {
      for (i += 1; i < src.length && src[i] !== '"'; i += 1) if (src[i] === '\\') i += 1;
      continue;
    }
    if (c === '`') {
      i = src.indexOf('`', i + 1);
      continue;
    }
    if (c === "'" && /^'(\\.|[^\\'])'/.test(src.slice(i, i + 4))) {
      i = src.indexOf("'", i + 2);
      continue;
    }
    if (pairs[c]) stack.push(pairs[c]);
    else if (c === '}' || c === ')' || c === ']') {
      if (stack.pop() !== c) throw new Error(`unbalanced ${c} at ${i}`);
      if (!stack.length) return i;
    }
  }
  throw new Error(`no matching brace for ${open}`);
}

/** The `///` doc comment directly above `index` (the start of an item line). */
export function docAbove(src: string, index: number): string {
  const lines = src.slice(0, index).split('\n');
  lines.pop(); // the partial line the item starts on
  const doc: string[] = [];
  for (let i = lines.length - 1; i >= 0; i -= 1) {
    const t = lines[i].trim();
    if (t.startsWith('///')) doc.unshift(t.replace(/^\/\/\/ ?/, ''));
    else if (t.startsWith('#[') || t === '') {
      if (t === '' && doc.length) break;
      continue;
    } else break;
  }
  return doc.join('\n').trim();
}

/** First sentence of a doc comment, with rustdoc links flattened. */
export function firstSentence(doc: string): string {
  const para = doc.split(/\n\s*\n/)[0].replace(/\s*\n\s*/g, ' ').trim();
  const flat = flattenRustdocLinks(para);
  // End at the first sentence stop that is not an abbreviation (`e.g.`, `i.e.`).
  const re = /[.!?](\s|$)/g;
  for (let m = re.exec(flat); m; m = re.exec(flat)) {
    if (/\b(e\.g|i\.e|etc|vs)$/.test(flat.slice(0, m.index))) continue;
    return flat.slice(0, m.index + 1).trim();
  }
  return flat.trim();
}

/** `[`Foo::bar`]` and `[text](path)` rustdoc links to plain code or text. */
export function flattenRustdocLinks(text: string): string {
  return text.replace(/\[(`[^`]+`)\](?:\([^)]*\))?/g, '$1').replace(/\[([^\]]+)\]\([^)]*\)/g, '$1');
}

export interface RustFn {
  name: string;
  doc: string;
  params: Array<{ name: string; type: string }>;
  returns: string;
  isAsync: boolean;
  body: string;
}

/** Public fns of the `impl <type>` blocks marked `#[uniffi::export]` (or all when `exportedOnly` is false). */
export function implFns(src: string, type: string, exportedOnly = true): RustFn[] {
  const out: RustFn[] = [];
  const re = new RegExp(`(#\\[uniffi::export\\]\\s*)?impl ${type} \\{`, 'g');
  for (let m = re.exec(src); m; m = re.exec(src)) {
    if (exportedOnly && !m[1]) continue;
    const open = m.index + m[0].length - 1;
    const close = matchBrace(src, open);
    const block = src.slice(open + 1, close);
    const fnRe = /\n    pub (async )?fn (\w+)\s*(<[^>]*>)?\s*\(/g;
    for (let f = fnRe.exec(block); f; f = fnRe.exec(block)) {
      const parenOpen = f.index + f[0].length - 1;
      const parenClose = matchBrace(block, parenOpen);
      const paramsText = block.slice(parenOpen + 1, parenClose);
      const bodyOpen = block.indexOf('{', parenClose);
      const bodyClose = matchBrace(block, bodyOpen);
      const retMatch = /->\s*([\s\S]*?)\s*(where\b[\s\S]*)?$/.exec(block.slice(parenClose + 1, bodyOpen));
      out.push({
        name: f[2],
        doc: docAbove(block, f.index + 1),
        params: splitTopLevel(paramsText)
          .map((p) => p.trim())
          .filter((p) => p && !/^(&?(mut )?self|self: .*)$/.test(p))
          .map((p) => {
            const i = p.indexOf(':');
            return { name: p.slice(0, i).trim(), type: p.slice(i + 1).trim() };
          }),
        returns: retMatch ? retMatch[1].trim() : '()',
        isAsync: Boolean(f[1]),
        body: block.slice(bodyOpen, bodyClose + 1),
      });
      fnRe.lastIndex = bodyClose;
    }
  }
  return out;
}

/** Splits on top-level commas (not inside <>, (), [], {}). */
export function splitTopLevel(text: string, sep = ','): string[] {
  const out: string[] = [];
  let depth = 0;
  let cur = '';
  for (const c of text) {
    if ('<([{'.includes(c)) depth += 1;
    else if ('>)]}'.includes(c)) depth -= 1;
    if (c === sep && depth === 0) {
      out.push(cur);
      cur = '';
    } else cur += c;
  }
  if (cur.trim()) out.push(cur);
  return out;
}

export interface RustVariant {
  name: string;
  doc: string;
  /** The `#[error("...")]` format string, or `transparent`. */
  message: string;
}

/** Variants of `pub enum <name>` with their docs and `#[error]` messages. */
export function enumVariants(src: string, name: string): RustVariant[] {
  const m = new RegExp(`pub enum ${name}\\s*\\{`).exec(src);
  if (!m) throw new Error(`enum ${name} not found`);
  const open = m.index + m[0].length - 1;
  const body = src.slice(open + 1, matchBrace(src, open));
  const out: RustVariant[] = [];
  const re = /\n    ([A-Z]\w*)\s*[({,\n]/g;
  let prevEnd = 0;
  for (let v = re.exec(body); v; v = re.exec(body)) {
    if (v.index < prevEnd) continue;
    const start = v.index + 1;
    const attrs = body.slice(prevEnd, start);
    const err = /#\[error\(\s*(transparent|"((?:[^"\\]|\\.)*)")/.exec(attrs);
    out.push({
      name: v[1],
      doc: docAbove(body, start),
      message: err ? (err[1] === 'transparent' ? 'transparent' : JSON.parse(`"${err[2]}"`)) : '',
    });
    const at = v.index + v[0].length - 1;
    prevEnd = body[at] === '{' || body[at] === '(' ? matchBrace(body, at) + 1 : at;
  }
  return out;
}

export interface RustField {
  name: string;
  type: string;
  doc: string;
}

/** Fields of `pub struct <name>`, with serde `rename_all = "camelCase"` applied when present. */
export function structFields(src: string, name: string): RustField[] {
  const m = new RegExp(`pub struct ${name}\\s*\\{`).exec(src);
  if (!m) throw new Error(`struct ${name} not found`);
  const attrs = src.slice(Math.max(0, src.lastIndexOf('\n\n', m.index)), m.index);
  const camel = /rename_all\s*=\s*"camelCase"/.test(attrs);
  const open = m.index + m[0].length - 1;
  const body = src.slice(open + 1, matchBrace(src, open));
  const out: RustField[] = [];
  const re = /\n    pub (\w+):\s*([^\n]+?),?\n/g;
  for (let f = re.exec(body); f; f = re.exec(body)) {
    const field = camel ? f[1].replace(/_([a-z])/g, (_, c: string) => c.toUpperCase()) : f[1];
    out.push({ name: field, type: f[2].replace(/,$/, ''), doc: docAbove(body, f.index + 1) });
    re.lastIndex -= 1;
  }
  return out;
}

/** Rust type to a JSON type expression (for request/response tables). */
export function jsonTypeOfRust(type: string): string {
  const t = type.replace(/\s+/g, '');
  const opt = /^Option<(.+)>$/.exec(t);
  if (opt) return jsonTypeOfRust(opt[1]);
  const vec = /^Vec<(.+)>$/.exec(t);
  if (vec) return `${jsonTypeOfRust(vec[1])}[]`;
  const map = /^(?:Hash|BTree)Map<String,(.+)>$/.exec(t);
  if (map) return `map of ${jsonTypeOfRust(map[1])}`;
  if (/^(String|&str|&'staticstr)$/.test(t)) return 'string';
  if (/^[ui](8|16|32|64|size)$/.test(t)) return 'integer';
  if (/^f(32|64)$/.test(t)) return 'number';
  if (t === 'bool') return 'boolean';
  return t;
}
