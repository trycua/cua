/**
 * Native accessibility observations for jev-use (RFC #4268, Phase 1).
 * Mirrors python/native.py; see that module for the full element rules.
 *
 * One observation is exactly one get_window_state call with both the tree and
 * the screenshot, so its snapshot_id (element tokens) and capture_id (visual
 * regions) describe the same moment. Stable IDs never use element_index.
 */
import { createHash } from 'node:crypto';

import {
  ROLE_CLASS_ACTION,
  isWindowChrome,
  normalizedRole,
  roleClass,
  type ActionKind,
  type Platform,
  type RoleClass,
} from './native_roles.js';

export const ID_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,63}$/;
export const PARAMETER_NAME_PATTERN = /^[a-z][a-z0-9_]{0,7}$/;
const MAX_LABEL_CHARS = 120;
const SLUG_CHARS = 32;

export type RiskCategory = 'destructive' | 'send' | 'purchase' | 'close_unsaved';

/** Whole-word, case-insensitive phrases; a guard, not an authorization boundary. */
export const RISK_PHRASES: Readonly<Record<RiskCategory, readonly string[]>> = {
  destructive: ['delete', 'remove', 'erase', 'trash', 'discard', 'clear all', 'format', 'reset'],
  send: ['send', 'submit', 'post', 'publish', 'share', 'reply'],
  purchase: ['buy', 'purchase', 'pay', 'checkout', 'order', 'subscribe'],
  close_unsaved: ['close', 'quit', 'exit', "don't save", 'discard changes'],
};

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

const RISK_PATTERNS = Object.entries(RISK_PHRASES).map(
  ([category, phrases]) =>
    [
      category,
      phrases.map((phrase) => new RegExp(`(?<![a-z0-9])${escapeRegExp(phrase)}(?![a-z0-9])`)),
    ] as const
);

export function riskCategories(label: string): Set<string> {
  const text = label.toLowerCase().replaceAll('’', "'");
  return new Set(
    RISK_PATTERNS.filter(([, patterns]) => patterns.some((pattern) => pattern.test(text))).map(
      ([category]) => category
    )
  );
}

function sha256Hex(value: string): string {
  return createHash('sha256').update(value, 'utf8').digest('hex');
}

/** Lowercase ASCII, collapse other runs to '-', trim to 32 characters. */
export function slug(label: string): string {
  const lowered = [...label]
    .map((char) => (/^[A-Za-z0-9]$/.test(char) ? char.toLowerCase() : '-'))
    .join('');
  const value = lowered
    .replace(/-+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, SLUG_CHARS)
    .replace(/^-+|-+$/g, '');
  return value || sha256Hex(label).slice(0, 8);
}

/** Python json.dumps(value, ensure_ascii=False, separators=(",", ":")) for arrays of strings/numbers. */
function compactJson(value: unknown): string {
  return JSON.stringify(value);
}

function hex4(value: unknown): string {
  return sha256Hex(compactJson(value)).slice(0, 4);
}

type Rect = readonly [number, number, number, number];

function num(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
}

function rect(value: unknown): Rect | undefined {
  if (!value || typeof value !== 'object') return undefined;
  const item = value as Record<string, unknown>;
  const x = num(item.x);
  const y = num(item.y);
  const w = num(item.w ?? item.width);
  const h = num(item.h ?? item.height);
  if (x === undefined || y === undefined || w === undefined || h === undefined) return undefined;
  return [x, y, w, h];
}

function intersects(a: Rect, b: Rect): boolean {
  return (
    Math.min(a[0] + a[2], b[0] + b[2]) > Math.max(a[0], b[0]) &&
    Math.min(a[1] + a[3], b[1] + b[3]) > Math.max(a[1], b[1])
  );
}

function str(value: unknown): string | undefined {
  return typeof value === 'string' ? value : undefined;
}

export class NativeObservationError extends Error {
  constructor(
    message: string,
    readonly code = 'invalid_window_state'
  ) {
    super(message);
    this.name = 'NativeObservationError';
  }
}

export type WindowElement = Readonly<Record<string, unknown>>;

export type NativeObservation = Readonly<{
  pid: number;
  windowId: number;
  snapshotId?: string;
  captureId?: string;
  windowBounds?: Rect;
  elements: readonly WindowElement[];
  complete: boolean;
  truncated: boolean;
  degradedReason?: string;
  treeMarkdown: string;
}>;

export function isPartial(observation: NativeObservation): boolean {
  return !observation.complete;
}

export function isTreeEmpty(observation: NativeObservation): boolean {
  return Boolean(observation.degradedReason?.startsWith('ax_tree_empty'));
}

export const WINDOW_ROOT_ROLES: ReadonlySet<string> = new Set(['window', 'application', 'frame']);
const MACOS_MENU_BAR_ROLE = 'menubar';
const normalizedOf = (item: WindowElement | undefined) =>
  item && typeof item.role === 'string' ? normalizedRole(item.role) : '';

function inMacosMenuBar(item: WindowElement, byIndex: Map<number, WindowElement>): boolean {
  const seen = new Set<number>();
  let current: WindowElement | undefined = item;
  while (current) {
    if (normalizedOf(current) === MACOS_MENU_BAR_ROLE) return true;
    const parent = current.parent_index;
    if (!Number.isInteger(parent) || seen.has(parent as number)) return false;
    seen.add(parent as number);
    current = byIndex.get(parent as number);
  }
  return false;
}

/** An unlabeled, valueless button directly under the window root (close, minimize, zoom). */
function isMacosWindowButton(item: WindowElement, byIndex: Map<number, WindowElement>): boolean {
  const parent = Number.isInteger(item.parent_index) ? byIndex.get(item.parent_index as number) : undefined;
  return (
    normalizedOf(item) === 'button' &&
    Boolean(parent) &&
    WINDOW_ROOT_ROLES.has(normalizedOf(parent)) &&
    !str(item.label) &&
    !str(item.value)
  );
}

function indexElements(observation: NativeObservation): Map<number, WindowElement> {
  const byIndex = new Map<number, WindowElement>();
  for (const item of observation.elements) {
    if (Number.isInteger(item.element_index)) byIndex.set(item.element_index as number, item);
  }
  return byIndex;
}

/** Whether an ancestor of `item` is a window-chrome container. */
function inWindowChrome(item: WindowElement, byIndex: Map<number, WindowElement>, platform: Platform): boolean {
  const seen = new Set<number>();
  let parent = item.parent_index;
  while (Number.isInteger(parent) && byIndex.has(parent as number) && !seen.has(parent as number)) {
    seen.add(parent as number);
    const ancestor = byIndex.get(parent as number)!;
    if (isWindowChrome(ancestor.role, platform)) return true;
    parent = ancestor.parent_index;
  }
  return false;
}

/** Whether any element is window content (mirrors native.py has_application_elements). */
export function hasApplicationElements(observation: NativeObservation, platform: Platform): boolean {
  const byIndex = indexElements(observation);
  return observation.elements.some((item) => {
    if (WINDOW_ROOT_ROLES.has(normalizedOf(item))) return false;
    if (isWindowChrome(item.role, platform) || inWindowChrome(item, byIndex, platform)) return false;
    if (platform === 'macos' && (inMacosMenuBar(item, byIndex) || isMacosWindowButton(item, byIndex))) return false;
    return true;
  });
}

export function parseWindowState(
  payload: Readonly<Record<string, unknown>>,
  expectedPid: number,
  expectedWindowId: number
): NativeObservation {
  if (payload.pid !== expectedPid || payload.window_id !== expectedWindowId) {
    throw new NativeObservationError('window state belongs to a different window', 'window_mismatch');
  }
  const raw = payload.elements ?? [];
  if (!Array.isArray(raw) || !raw.every((item) => item && typeof item === 'object')) {
    throw new NativeObservationError('window state has malformed elements');
  }
  const index = (item: WindowElement) =>
    Number.isInteger(item.element_index) ? (item.element_index as number) : -1;
  const elements = [...(raw as WindowElement[])].sort((a, b) => index(a) - index(b));
  return {
    pid: expectedPid,
    windowId: expectedWindowId,
    snapshotId: str(payload.snapshot_id),
    captureId: str(payload.capture_id),
    windowBounds: rect(payload.window_bounds),
    elements,
    complete: payload.elements_complete === true,
    truncated: payload.truncated === true || Boolean(payload.truncation_reason),
    degradedReason: payload.degraded ? str(payload.degraded_reason) : undefined,
    treeMarkdown: str(payload.tree_markdown) ?? '',
  };
}

export type NativeControl = Readonly<{
  id: string;
  roleClass: RoleClass;
  action: ActionKind;
  label: string;
  value?: string;
  selected?: boolean;
  elementIndex: number;
  elementToken: string;
  risk: ReadonlySet<string>;
}>;

export type NativeControls = Readonly<{
  controls: readonly NativeControl[];
  excluded: Readonly<Record<string, number>>;
}>;

const collapse = (value: string) => value.split(/\s+/).filter(Boolean).join(' ');

/** Apply the element rules and assign stable IDs, in element_index order. */
export function eligibleControls(
  observation: NativeObservation,
  platform: Platform,
  redact: (value: string) => string = (value) => value
): NativeControls {
  const byIndex = indexElements(observation);
  const excluded: Record<string, number> = {};
  const exclude = (reason: string) => {
    excluded[reason] = (excluded[reason] ?? 0) + 1;
  };
  const labelOf = (item: WindowElement) => collapse(redact(str(item.label) ?? ''));
  const pathOf = (item: WindowElement): [string, string][] => {
    const path: [string, string][] = [];
    const seen = new Set<number>();
    let parent = item.parent_index;
    while (Number.isInteger(parent) && byIndex.has(parent as number) && !seen.has(parent as number)) {
      seen.add(parent as number);
      const ancestor = byIndex.get(parent as number)!;
      const rawRole = str(ancestor.role) ?? '';
      path.push([roleClass(rawRole, platform) ?? normalizedRole(rawRole), labelOf(ancestor)]);
      parent = ancestor.parent_index;
    }
    return path.reverse();
  };

  const pending: { item: WindowElement; klass: RoleClass; label: string; path: [string, string][] }[] = [];
  for (const item of observation.elements) {
    const klass = roleClass(item.role, platform);
    if (!klass) {
      exclude('unknown_role');
      continue;
    }
    if (inWindowChrome(item, byIndex, platform)) {
      exclude('window_chrome');
      continue;
    }
    if (item.enabled === false) {
      exclude('disabled');
      continue;
    }
    const frame = rect(item.frame);
    if (
      !frame ||
      frame[2] <= 0 ||
      frame[3] <= 0 ||
      !observation.windowBounds ||
      !intersects(frame, observation.windowBounds)
    ) {
      exclude('off_screen');
      continue;
    }
    const label = labelOf(item);
    const value = str(item.value);
    const redactedValue = value === undefined ? undefined : collapse(redact(value));
    if (item.unlabelled === true || !label || label === redactedValue) {
      exclude('unlabeled');
      continue;
    }
    if (item.in_web_content === true) {
      exclude('web_content');
      continue;
    }
    if (typeof item.element_token !== 'string' || !item.element_token) {
      exclude('no_token');
      continue;
    }
    pending.push({ item, klass, label, path: pathOf(item) });
  }

  const bases = pending.map(({ klass, label }) => `ax:${klass}:${slug(label)}`);
  const counts = new Map<string, number>();
  for (const base of bases) counts.set(base, (counts.get(base) ?? 0) + 1);
  const ordinals = new Map<string, number>();
  const controls = pending.map(({ item, klass, label, path }, position) => {
    const base = bases[position];
    const key = JSON.stringify([klass, label, path]);
    const ordinal = ordinals.get(key) ?? 0;
    ordinals.set(key, ordinal + 1);
    const id = counts.get(base) === 1 ? base : `${base}:${hex4([path, ordinal])}`;
    if (!ID_PATTERN.test(id)) throw new NativeObservationError('derived candidate ID is invalid');
    const control: NativeControl = {
      id,
      roleClass: klass,
      action: ROLE_CLASS_ACTION[klass],
      label: label.slice(0, MAX_LABEL_CHARS),
      value: str(item.value),
      selected: typeof item.selected === 'boolean' ? item.selected : undefined,
      elementIndex: item.element_index as number,
      elementToken: item.element_token as string,
      risk: riskCategories(label),
    };
    return control;
  });
  return { controls, excluded };
}

export type ElementState =
  | 'enabled'
  | 'checked'
  | 'unchecked'
  | 'selected'
  | 'not_selected'
  | 'empty'
  | 'has_text';

export function elementState(control: NativeControl): ElementState {
  if (control.roleClass === 'checkbox' || control.roleClass === 'toggle') {
    return control.selected ? 'checked' : 'unchecked';
  }
  if (control.roleClass === 'radio') return control.selected ? 'selected' : 'not_selected';
  if (control.roleClass === 'text_input') return control.value ? 'has_text' : 'empty';
  return 'enabled';
}

export function fieldState(value: string | undefined, required: string): string {
  if (!value) return 'empty';
  if (value === required) return 'contains_required_value';
  return 'contains_other_value';
}
