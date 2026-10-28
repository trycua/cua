// Turns a parsed recording into the journal model: one panel per turn with the
// action in plain words, its delivery and outcome, and the screenshot worth
// looking at. Pure: no file system access.

import { basename } from 'node:path';

// Arguments that carry what the agent typed or ran. Recordings are made on
// real desktops, so these stay hidden unless the caller opts in.
const TYPED_ARGUMENTS = ['text', 'value', 'javascript'];

// Desktop input tools, whose target is an element, a pixel, the focused
// control or the whole desktop (docs/action-icon-catalog.md).
const INPUT_TOOLS = new Set([
  'click',
  'double_click',
  'right_click',
  'drag',
  'type_text',
  'type_text_chars',
  'press_key',
  'hotkey',
  'scroll',
  'set_value',
]);

const KIND_BY_TOOL = {
  click: 'click',
  double_click: 'click',
  right_click: 'click',
  browser_click: 'click',
  mouse_button_down: 'click',
  mouse_button_up: 'click',
  drag: 'drag',
  mouse_drag: 'drag',
  parallel_mouse_drag: 'drag',
  type_text: 'type',
  type_text_chars: 'type',
  browser_type: 'type',
  press_key: 'key',
  hotkey: 'hotkey',
  set_value: 'value',
  scroll: 'scroll',
  move_cursor: 'move',
  launch_app: 'launch',
  kill_app: 'kill',
  bring_to_front: 'window',
  set_window_frame: 'window',
  browser_navigate: 'navigate',
  browser_dialog: 'dialog',
  browser_set_input_files: 'upload',
  browser_download: 'download',
  clipboard_write: 'clipboard',
  start_recording: 'record',
  stop_recording: 'record',
  replay_trajectory: 'record',
};

// `page` and `browser_pointer` name their gesture in the `action` argument.
const KIND_BY_ACTION = {
  click_element: 'click',
  right_click: 'click',
  double_click: 'click',
  insert_text: 'type',
  type_keystrokes: 'type',
  execute_javascript: 'script',
  hover: 'move',
  scroll: 'scroll',
  drag: 'drag',
};

const PAGE_TITLES = {
  click_element: 'Click element',
  insert_text: 'Insert text',
  type_keystrokes: 'Type keystrokes',
  execute_javascript: 'Run JavaScript',
  get_text: 'Get page text',
  query_dom: 'Query DOM',
};

const KEY_NAMES = {
  ctrl: 'Ctrl',
  control: 'Ctrl',
  cmd: 'Cmd',
  command: 'Cmd',
  alt: 'Alt',
  option: 'Option',
  shift: 'Shift',
  win: 'Win',
  super: 'Super',
  return: 'Return',
  enter: 'Enter',
  escape: 'Esc',
  esc: 'Esc',
  pageup: 'Page Up',
  pagedown: 'Page Down',
};

// Effects that mean input was sent but not read back as intended.
const UNVERIFIED = new Set(['unverifiable', 'partial', 'suspected_noop']);

export function buildJournal(recording, options = {}) {
  const settings = {
    showTypedText: Boolean(options.showTypedText),
    includeDesktop: Boolean(options.includeDesktop),
  };
  const panels = recording.turns.map((turn) => buildPanel(turn, settings));
  const first = recording.turns[0]?.action;
  const startedAt = first
    ? Date.parse(first.timestamp) - (first.t_ms_from_session_start ?? 0)
    : NaN;
  return {
    title: options.title || basename(recording.dir),
    startedAt: Number.isFinite(startedAt) ? startedAt : null,
    durationMs: Math.max(recording.cursorEndMs ?? 0, ...panels.map((panel) => panel.endMs ?? 0)),
    apps: unique(panels.filter((panel) => panel.kind === 'launch').map((panel) => panel.target)),
    windows: unique(recording.turns.map((turn) => turn.windowTitle)),
    stats: countStats(panels),
    privacy: {
      ...settings,
      typedHidden: panels.filter((panel) => panel.typedHidden).length,
      desktopHidden: panels.filter((panel) => panel.frame.hidden === 'desktop').length,
    },
    skipped: recording.skipped,
    panels,
  };
}

// Large screenshots shrink too far to show a click, so the panel zooms to a
// 16:10 window around the point. Window-sized captures stay whole.
export function cropAround(point, { width, height }) {
  if (!point || !width || !height || (width <= 1280 && height <= 800)) return null;
  const cropWidth = Math.min(width, Math.max(640, Math.round(width * 0.45)));
  const cropHeight = Math.min(height, Math.round((cropWidth * 10) / 16));
  return {
    x: clamp(Math.round(point.x - cropWidth / 2), 0, width - cropWidth),
    y: clamp(Math.round(point.y - cropHeight / 2), 0, height - cropHeight),
    width: cropWidth,
    height: cropHeight,
  };
}

function buildPanel(turn, settings) {
  const { action, images } = turn;
  const args = action.arguments ?? {};
  const truth = action.action_truth ?? null;
  const [title, target] = describe(action.tool, args, settings.showTypedText);
  const outcome = outcomeOf(action);
  const screen =
    images.before && images.after
      ? images.before.sha256 === images.after.sha256
        ? 'unchanged'
        : 'changed'
      : 'unknown';
  const scrub = scrubber(args, settings.showTypedText);
  return {
    id: turn.name,
    index: turn.index,
    tool: action.tool,
    kind: kindOf(action.tool, args),
    title,
    target,
    startMs: action.t_start_ms_from_session_start ?? null,
    endMs: action.t_ms_from_session_start ?? null,
    outcome,
    screen,
    attention:
      outcome === 'failed' ||
      outcome === 'refused' ||
      (UNVERIFIED.has(outcome) && screen === 'unchanged'),
    delivery: truth?.actual_delivery ?? null,
    requestedDelivery: truth?.requested_delivery ?? args.delivery_mode ?? null,
    targeting: targetingOf(action.tool, args),
    route: truth?.route ?? null,
    transport: truth?.transport ?? null,
    summary: scrub(cleanSummary(action.result_summary)),
    refusal: truth?.refusal
      ? { code: truth.refusal.code, hint: scrub(truth.refusal.hint ?? '') }
      : null,
    advice: scrub(truth?.escalation?.detail ?? truth?.escalation?.reason ?? ''),
    windowTitle: turn.windowTitle,
    arguments: settings.showTypedText ? args : redact(args),
    typedHidden:
      !settings.showTypedText && TYPED_ARGUMENTS.some((key) => typeof args[key] === 'string'),
    ...frames(turn, settings),
  };
}

// `effect` comes from ActionExecutionRecord (cua-driver-core/src/action_record.rs).
// Tools without one only report result_error, hence `failed` and `done`.
function outcomeOf(action) {
  const truth = action.action_truth;
  if (truth?.effect === 'refused' || truth?.refusal) return 'refused';
  if (action.result_error) return 'failed';
  return truth?.effect ?? 'done';
}

function frames({ action, evidence, images }, settings) {
  const states = [evidence.before?.state, evidence.after?.state];
  // Without a target pid the recorder captures the whole screen, which can
  // show anything the user had open.
  if (
    !settings.includeDesktop &&
    (images.before || images.after) &&
    states.some((state) => state?.classification === 'no_target_pid')
  ) {
    return { frame: { hidden: 'desktop' }, pair: null };
  }
  const pair =
    images.before || images.after
      ? { before: images.before ?? null, after: images.after ?? null }
      : null;
  if (images.click) {
    const crop = cropAround(action.click_point, images.click);
    return { frame: { image: images.click, caption: 'Click target', crop }, pair };
  }
  if (images.after) return { frame: { image: images.after, caption: 'After' }, pair };
  if (images.before) {
    const gone = evidence.after?.screenshot?.classification === 'target_unavailable';
    return {
      frame: { image: images.before, caption: gone ? 'Before; window gone after' : 'Before' },
      pair,
    };
  }
  const shots = [evidence.before?.screenshot, evidence.after?.screenshot];
  const suppressed = shots.some((shot) => shot?.classification === 'privacy_suppressed');
  return { frame: { hidden: suppressed ? 'privacy' : 'missing' }, pair: null };
}

function kindOf(tool, args) {
  if (tool === 'page' || tool === 'browser_pointer')
    return KIND_BY_ACTION[args.action] ?? 'inspect';
  return KIND_BY_TOOL[tool] ?? (/^(get|list)_/.test(tool) ? 'inspect' : 'tool');
}

function describe(tool, args, showTypedText) {
  const typed = (value) => typedText(value, showTypedText);
  switch (tool) {
    case 'click':
    case 'double_click':
    case 'right_click':
      return [clickTitle(tool, args), targetOf(args)];
    case 'browser_click':
      return ['Click', targetOf(args)];
    case 'browser_pointer':
      return [humanize(args.action ?? 'pointer'), targetOf(args)];
    case 'type_text':
    case 'type_text_chars':
    case 'browser_type':
      return ['Type', typed(args.text)];
    case 'set_value':
      return ['Set value', typed(args.value)];
    case 'press_key':
      return ['Press', keyChord([...[].concat(args.modifiers ?? []), args.key])];
    case 'hotkey':
      return ['Hotkey', keyChord([].concat(args.keys ?? []))];
    case 'scroll':
      return [
        'Scroll',
        [args.direction, args.amount > 1 && `× ${args.amount}`, args.by].filter(Boolean).join(' '),
      ];
    case 'drag':
      return [
        'Drag',
        args.from_x == null
          ? ''
          : `(${args.from_x}, ${args.from_y}) → (${args.to_x}, ${args.to_y})`,
      ];
    case 'launch_app':
      return ['Launch', appName(args)];
    case 'kill_app':
      return ['Kill app', args.pid == null ? '' : `pid ${args.pid}`];
    case 'browser_navigate':
      return ['Navigate', showTypedText ? (args.url ?? '') : withoutQuery(args.url ?? '')];
    case 'page':
      return [
        PAGE_TITLES[args.action] ?? humanize(args.action ?? tool),
        typed(args.javascript ?? args.text) || (args.css_selector ?? args.selector ?? ''),
      ];
    case 'clipboard_write':
      return ['Write clipboard', typed(args.text)];
    default:
      return [humanize(tool), ''];
  }
}

function clickTitle(tool, args) {
  if (tool === 'right_click' || args.button === 'right') return 'Right-click';
  if (tool === 'double_click' || args.count === 2) return 'Double-click';
  if (args.count === 3) return 'Triple-click';
  return 'Click';
}

function targetOf(args) {
  if (args.element_token) return `element ${args.element_token}`;
  if (args.element_index != null) return `element #${args.element_index}`;
  if (args.ref) return `ref ${args.ref}`;
  if (args.x != null && args.y != null) {
    return `${args.scope === 'desktop' ? 'desktop ' : ''}at (${args.x}, ${args.y})`;
  }
  return '';
}

function targetingOf(tool, args) {
  if (!INPUT_TOOLS.has(tool)) return null;
  if (args.scope === 'desktop') return 'desktop';
  if (args.element_token || args.element_index != null) return 'element';
  if (args.x != null || args.from_x != null) return 'pixel';
  return 'focused';
}

function keyChord(keys) {
  return keys
    .filter((key) => typeof key === 'string' && key)
    .map(
      (key) =>
        KEY_NAMES[key.toLowerCase()] ??
        (key.length === 1 || /^f\d+$/i.test(key)
          ? key.toUpperCase()
          : key[0].toUpperCase() + key.slice(1))
    )
    .join('+');
}

function appName(args) {
  const path = args.path ?? args.launch_path;
  if (typeof path === 'string') return path.split(/[\\/]/).pop();
  return args.name ?? args.bundle_id ?? args.aumid ?? withoutQuery([].concat(args.urls ?? [])[0]);
}

function typedText(value, show) {
  if (typeof value !== 'string') return '';
  return show ? `“${value}”` : `${characters(value)}, hidden`;
}

function characters(value) {
  const count = [...value].length;
  return `${count} ${count === 1 ? 'character' : 'characters'}`;
}

// Query strings and fragments can carry tokens; the path is enough to follow along.
function withoutQuery(url) {
  return typeof url === 'string' ? url.split(/[?#]/)[0] : '';
}

function redact(args) {
  const out = { ...args };
  for (const key of TYPED_ARGUMENTS) {
    if (typeof out[key] === 'string') out[key] = `‹hidden: ${characters(out[key])}›`;
  }
  if (typeof out.url === 'string') out.url = withoutQuery(out.url);
  if (Array.isArray(out.urls)) out.urls = out.urls.map(withoutQuery);
  return out;
}

// Results can echo arguments back, so hidden values are cut out of them too.
function scrubber(args, showTypedText) {
  if (showTypedText) return (text) => text ?? '';
  const pairs = [
    ...TYPED_ARGUMENTS.map((key) => [args[key], '‹hidden›']),
    ...[args.url, ...[].concat(args.urls ?? [])].map((url) => [url, withoutQuery(url)]),
  ].filter(([value, shown]) => typeof value === 'string' && value.length >= 3 && value !== shown);
  return (text) => pairs.reduce((out, [value, shown]) => out.split(value).join(shown), text ?? '');
}

// Summaries open with a status emoji (✅, 📨); the stamp already says that.
function cleanSummary(text) {
  return typeof text === 'string' ? text.replace(/^[\p{Extended_Pictographic}️\s]+/u, '') : '';
}

function countStats(panels) {
  const count = (test) => panels.filter(test).length;
  return {
    turns: panels.length,
    failed: count((panel) => panel.outcome === 'failed'),
    refused: count((panel) => panel.outcome === 'refused'),
    unverified: count((panel) => UNVERIFIED.has(panel.outcome)),
    unchanged: count((panel) => UNVERIFIED.has(panel.outcome) && panel.screen === 'unchanged'),
    confirmed: count((panel) => panel.outcome === 'confirmed'),
    background: count((panel) => panel.delivery === 'background'),
    attention: count((panel) => panel.attention),
  };
}

function humanize(name) {
  const words = String(name).replaceAll('_', ' ');
  return words.charAt(0).toUpperCase() + words.slice(1);
}

function unique(values) {
  return [...new Set(values.filter(Boolean))];
}

function clamp(value, min, max) {
  return Math.min(Math.max(value, min), max);
}
