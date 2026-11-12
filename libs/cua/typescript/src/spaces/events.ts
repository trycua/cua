/**
 * What an agent sends back.
 *
 * The honest situation, as of this release: the agent runtime inside a Space is
 * a terminal. `agent_status` returns the tail of the run's output (the agent
 * runs as a detached, tagged cua-spacesd process). There is no structured
 * result channel yet — no file manifest, no image parts, no approval
 * protocol. The supported agents are launched auto-approved and the sandbox is
 * the safety boundary.
 *
 * So this module does two things and is careful about the difference:
 *
 *  1. It defines the *result model* a chat app with agent coworkers needs — text, files,
 *     images, links, approval requests — as a stable discriminated union, so an
 *     app can be written against it today and keep working when the runtime
 *     starts emitting the real thing.
 *  2. It derives what it can from the terminal transcript, and marks every
 *     derived event `derived: true`. A consumer can therefore tell a fact
 *     ("the agent emitted this file") from an inference ("a path-shaped string
 *     appeared in the scrollback"). Nothing here pretends to be ground truth.
 *
 * When the agent runtime gains a structured channel (see `docs/`), the adapter
 * gains a branch that emits the same events with `derived: false`. The union is
 * the stable contract; the adapter is not.
 */

export type ThreadEventKind =
  | 'text'
  | 'file'
  | 'image'
  | 'link'
  | 'approval_request'
  | 'state'
  | 'error';

interface ThreadEventBase {
  kind: ThreadEventKind;
  /** Monotonic within a thread, assigned by the SDK. */
  seq: number;
  /** Wall clock at the moment the SDK observed it, not when the agent produced it. */
  observedAt: string;
  /**
   * `true` when this event was inferred from unstructured terminal output
   * rather than reported by the agent runtime. Always check it before treating
   * an event as authoritative.
   */
  derived: boolean;
}

export interface TextEvent extends ThreadEventBase {
  kind: 'text';
  text: string;
}

/** A file the agent appears to have produced, by absolute path inside the Space.
 *  Fetch it with `space.download(path, destDir)` — the SDK does not pre-fetch. */
export interface FileEvent extends ThreadEventBase {
  kind: 'file';
  path: string;
}

/** An image the agent produced. `data` is base64; `path` is set instead when the
 *  image is only known by its in-Space location. */
export interface ImageEvent extends ThreadEventBase {
  kind: 'image';
  mimeType: string;
  data?: string;
  path?: string;
}

export interface LinkEvent extends ThreadEventBase {
  kind: 'link';
  url: string;
}

/**
 * The agent is blocked on a human decision.
 *
 * No agent runtime in a Space emits this today (they run auto-approved). It is
 * defined because an app that puts an agent in front of a user needs the shape,
 * and because a derived detector can spot a REPL prompt that is waiting. Answer
 * it with `thread.send(...)` — there is no separate approval RPC yet, and this
 * SDK will not invent one that silently does nothing.
 */
export interface ApprovalRequestEvent extends ThreadEventBase {
  kind: 'approval_request';
  prompt: string;
  /** Choices parsed from the prompt, when they were parseable. */
  options: string[];
}

export interface StateEvent extends ThreadEventBase {
  kind: 'state';
  state: AgentStatus;
  /** The ladder's reason for this status. */
  reason: string;
}

export interface ErrorEvent extends ThreadEventBase {
  kind: 'error';
  message: string;
}

export type ThreadEvent =
  | TextEvent
  | FileEvent
  | ImageEvent
  | LinkEvent
  | ApprovalRequestEvent
  | StateEvent
  | ErrorEvent;

/**
 * The agent status ladder — one closed vocabulary for every harness and every
 * provider. A UI can rely on this being closed.
 *
 * `idle` and `finished` are deliberately different: both mean "no turn is
 * running", but `idle` means the session can still be continued and `finished`
 * means it cannot.
 *
 * `unknown` is a real, reachable state and is **not** a synonym for "done". A
 * probe that could not run, an unreachable Space, and an interactive REPL whose
 * working-vs-waiting we cannot tell apart all land here. Reporting any of them
 * as `idle` is how a caller concludes an agent finished when the Space actually
 * fell over — and `crashed` exists so a dead process that never recorded an
 * exit status is never read as a clean finish.
 */
export type AgentStatus =
  | 'running'
  | 'awaiting_input'
  | 'idle'
  | 'finished'
  | 'failed'
  | 'crashed'
  | 'unknown';

/** Everything the SDK knows about a run's current moment. */
export interface ThreadStatus {
  runId: string;
  status: AgentStatus;
  /** Why the ladder landed there, in words. Always populated. */
  reason: string;
  /** A follow-up sent now starts the next turn: the server's published
   *  rule (idle, or a crashed or failed run it restarts; not mid-turn). */
  acceptsMessage: boolean;
  /** The raw transcript tail exactly as the Space reported it. Always present,
   *  because every derived event above is an interpretation of this. */
  transcript: string;
  /** The agent's terminal window on the Space desktop, when it was found.
   *  `null` means "not found", which is not the same as "does not exist". */
  desktopWindow: string | null;
}

/**
 * Turns a growing transcript into events, without re-emitting what it already
 * emitted.
 *
 * The runtime gives us a *tail*, not a stream, so the adapter tracks what it has
 * consumed by content, not by offset: a tail that scrolled past our last known
 * position is detected, and only the portion it can prove is new is emitted.
 * When it cannot prove overlap at all — the tail
 * scrolled entirely past — it emits the whole tail and an `error` event saying
 * output was lost, rather than silently dropping it.
 */
export class TranscriptAdapter {
  private consumed = '';
  private seq = 0;

  /** Feed the newest transcript tail; get back only the newly observed events. */
  ingest(tail: string, now: () => Date = () => new Date()): ThreadEvent[] {
    const events: ThreadEvent[] = [];
    const fresh = this.diff(tail, events, now);
    if (fresh === '') return events;
    this.consumed = tail;
    for (const event of this.extract(fresh, now)) events.push(event);
    return events;
  }

  /** Reset for a new turn that restarts the session (codex `resume` does this). */
  reset(): void {
    this.consumed = '';
  }

  private diff(tail: string, events: ThreadEvent[], now: () => Date): string {
    if (this.consumed === '') return tail;
    if (tail === this.consumed) return '';
    if (tail.startsWith(this.consumed)) return tail.slice(this.consumed.length);
    // The window scrolled. Find the longest suffix of `consumed` that prefixes
    // `tail`; everything after it is new.
    const max = Math.min(this.consumed.length, tail.length);
    for (let overlap = max; overlap > 0; overlap--) {
      if (tail.startsWith(this.consumed.slice(this.consumed.length - overlap))) {
        return tail.slice(overlap);
      }
    }
    events.push(
      this.make<ErrorEvent>(
        {
          kind: 'error',
          message:
            'transcript scrolled past the polled window; output between the last poll and this one was lost',
        },
        now,
      ),
    );
    return tail;
  }

  private *extract(fresh: string, now: () => Date): Generator<ThreadEvent> {
    const text = fresh.trim();
    if (text !== '') yield this.make<TextEvent>({ kind: 'text', text }, now);

    for (const url of matchAll(fresh, URL_PATTERN)) {
      yield this.make<LinkEvent>({ kind: 'link', url: trimTrailingPunctuation(url) }, now);
    }
    for (const path of matchAll(fresh, PATH_PATTERN)) {
      const clean = trimTrailingPunctuation(path);
      if (IMAGE_SUFFIX.test(clean)) {
        yield this.make<ImageEvent>(
          { kind: 'image', mimeType: mimeForPath(clean), path: clean },
          now,
        );
      } else {
        yield this.make<FileEvent>({ kind: 'file', path: clean }, now);
      }
    }
    const approval = detectApproval(fresh);
    if (approval) {
      yield this.make<ApprovalRequestEvent>(
        { kind: 'approval_request', prompt: approval.prompt, options: approval.options },
        now,
      );
    }
  }

  private make<T extends ThreadEvent>(
    partial: Omit<T, 'seq' | 'observedAt' | 'derived'>,
    now: () => Date,
  ): T {
    return {
      ...partial,
      seq: this.seq++,
      observedAt: now().toISOString(),
      derived: true,
    } as T;
  }
}

// -- derivation heuristics, deliberately conservative -----------------------

const URL_PATTERN = /\bhttps?:\/\/[^\s<>"')\]]+/g;
/** Absolute POSIX paths only, with a file extension. A bare `/root` is a
 *  directory mention, not a produced artefact, and is not reported. */
const PATH_PATTERN = /(?<![\w:/])\/(?:[\w.@+-]+\/)+[\w.@+-]+\.[A-Za-z0-9]{1,8}\b/g;
const IMAGE_SUFFIX = /\.(png|jpe?g|gif|webp|bmp|svg)$/i;

function mimeForPath(path: string): string {
  const suffix = path.toLowerCase().split('.').pop() ?? '';
  if (suffix === 'jpg' || suffix === 'jpeg') return 'image/jpeg';
  if (suffix === 'svg') return 'image/svg+xml';
  if (suffix === 'gif') return 'image/gif';
  if (suffix === 'webp') return 'image/webp';
  if (suffix === 'bmp') return 'image/bmp';
  return 'image/png';
}

function matchAll(text: string, pattern: RegExp): string[] {
  const out: string[] = [];
  const seen = new Set<string>();
  for (const match of text.matchAll(pattern)) {
    const value = match[0];
    if (!seen.has(value)) {
      seen.add(value);
      out.push(value);
    }
  }
  return out;
}

function trimTrailingPunctuation(value: string): string {
  return value.replace(/[).,;:'"\]]+$/, '');
}

/**
 * Spot a terminal prompt that is waiting on a human.
 *
 * Only fires on a prompt at the very end of the fresh output — a question in
 * the middle of the scrollback has already been answered. Recognizes the two
 * common shapes: a `[y/N]`-style suffix and a numbered choice list followed by
 * a prompt line.
 */
export function detectApproval(
  fresh: string,
): { prompt: string; options: string[] } | null {
  const lines = fresh.split(/\r?\n/);
  let lastIndex = lines.length - 1;
  while (lastIndex >= 0 && (lines[lastIndex] ?? '').trim() === '') lastIndex--;
  if (lastIndex < 0) return null;
  const tail = (lines[lastIndex] ?? '').trim();

  const yesNo = tail.match(/(.*?)\s*[[(]\s*(y(?:es)?)\s*\/\s*(n(?:o)?)\s*[\])]\s*[?:]?\s*$/i);
  if (yesNo) {
    return {
      prompt: (yesNo[1] ?? '').trim() || tail,
      options: [yesNo[2] ?? 'y', yesNo[3] ?? 'n'],
    };
  }

  // A numbered menu: collect contiguous "1) …" / "2. …" lines above the prompt.
  const options: string[] = [];
  let cursor = lastIndex;
  const promptLooksLikeQuestion = /[?:]\s*$/.test(tail) || /^[>❯]/.test(tail);
  if (!promptLooksLikeQuestion) return null;
  cursor--;
  while (cursor >= 0) {
    const candidate = (lines[cursor] ?? '').trim();
    const numbered = candidate.match(/^(\d+)\s*[).]\s+(.*)$/);
    if (!numbered) break;
    options.unshift(numbered[2] ?? '');
    cursor--;
  }
  if (options.length < 2) return null;
  return { prompt: tail, options };
}
