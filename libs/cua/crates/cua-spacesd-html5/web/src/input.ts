// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Keyboard, pointer and touch input for the viewer. Everything becomes
 * wire v2 `InteractiveInputEvent`s on the media socket; spacesd hands them
 * to cua-driver, which performs the input in the guest.
 *
 * Keyboard. A hidden textarea owns focus, so the platform's text machinery
 * (keyboard layouts, dead keys, IMEs, mobile soft keyboards) produces the
 * text, and the viewer sends the committed characters (`text_commit`). Keys
 * that are not text (Enter, arrows, F-keys, ...) and shortcuts (anything
 * with Ctrl, Cmd or Alt) are sent as key events. Shortcut letters and
 * digits come from the physical key (`KeyboardEvent.code`), so Ctrl+C is
 * Ctrl+C on a Cyrillic or AZERTY layout too. AltGr is text, not a shortcut.
 *
 * Pointer. Mouse and pen map 1:1. Touch: tap clicks, a long press is a
 * right click, two fingers scroll, one-finger drag drags.
 */

import {
  modifiersOf,
  normalizePoint,
  pointerButton,
  type InputModifier,
  type InteractiveInputEvent,
} from "./core/mediaWire";

export type Emit = (event: InteractiveInputEvent) => void;

export interface KeyLike {
  key: string;
  code: string;
  shiftKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  metaKey: boolean;
  repeat: boolean;
  isComposing?: boolean;
  keyCode?: number;
  getModifierState?: (key: string) => boolean;
}

/** Keys that are never text. DOM names, lower-cased on the wire. */
const NAMED = new Set([
  "Enter",
  "Tab",
  "Backspace",
  "Delete",
  "Escape",
  "ArrowUp",
  "ArrowDown",
  "ArrowLeft",
  "ArrowRight",
  "Home",
  "End",
  "PageUp",
  "PageDown",
  "Insert",
  "CapsLock",
  "Shift",
  "Control",
  "Alt",
  "Meta",
  "ContextMenu",
  "PrintScreen",
  "Pause",
  "ScrollLock",
  "NumLock",
]);

const MODIFIER_KEYS = new Set(["Shift", "Control", "Alt", "Meta", "AltGraph"]);

/** Options for mapping one key event. */
export interface KeyMapOptions {
  /** Send the host's Cmd as Ctrl (a Mac viewing a Linux or Windows guest). */
  metaAsControl: boolean;
}

function isAltGraph(event: KeyLike): boolean {
  return Boolean(event.getModifierState?.("AltGraph"));
}

/** The wire key name for a shortcut: the physical key's letter or digit
 * when there is one, else the produced character. */
export function shortcutKey(event: KeyLike): string {
  const letter = /^Key([A-Z])$/.exec(event.code);
  if (letter) return letter[1]!.toLowerCase();
  const digit = /^Digit([0-9])$/.exec(event.code);
  if (digit) return digit[1]!;
  if (event.key === " ") return "space";
  return event.key.length === 1 ? event.key.toLowerCase() : event.key.toLowerCase();
}

function wireModifiers(event: KeyLike, options: KeyMapOptions): InputModifier[] {
  const out = modifiersOf(event);
  if (!options.metaAsControl) return out;
  return [...new Set(out.map((m) => (m === "command" ? "control" : m)))];
}

/**
 * Maps a keydown/keyup to an input event, or null when the key is text that
 * the textarea's `input` event will deliver (or nothing at all).
 */
export function mapKey(event: KeyLike, state: "down" | "up", options: KeyMapOptions): InteractiveInputEvent | null {
  if (event.isComposing || event.keyCode === 229) return null; // IME owns it
  if (event.key === "Dead" || event.key === "Unidentified" || event.key === "Process") return null;
  if (event.key === "AltGraph") return null;
  const altGraph = isAltGraph(event);
  const shortcut = !altGraph && (event.ctrlKey || event.metaKey || event.altKey);
  if (MODIFIER_KEYS.has(event.key)) {
    let key = event.key.toLowerCase();
    if (options.metaAsControl && key === "meta") key = "control";
    return { kind: "key", key, state, modifiers: [], repeat: state === "down" && event.repeat };
  }
  if (NAMED.has(event.key) || /^F([1-9]|1[0-9]|2[0-4])$/.test(event.key)) {
    return {
      kind: "key",
      key: event.key.toLowerCase(),
      state,
      modifiers: wireModifiers(event, options),
      repeat: state === "down" && event.repeat,
    };
  }
  if (shortcut) {
    return {
      kind: "key",
      key: shortcutKey(event),
      state,
      modifiers: wireModifiers(event, options),
      repeat: state === "down" && event.repeat,
    };
  }
  return null; // text: the textarea's input event commits it
}

/** Maps a `beforeinput` (soft keyboards, IMEs) to input events. */
export function mapBeforeInput(inputType: string, data: string | null): InteractiveInputEvent[] {
  const key = (name: string): InteractiveInputEvent[] => [
    { kind: "key", key: name, state: "down", modifiers: [], repeat: false },
    { kind: "key", key: name, state: "up", modifiers: [], repeat: false },
  ];
  switch (inputType) {
    case "insertText":
    case "insertReplacementText":
    case "insertFromComposition":
      return data ? [{ kind: "text_commit", text: data }] : [];
    case "insertLineBreak":
    case "insertParagraph":
      return key("enter");
    case "deleteContentBackward":
      return key("backspace");
    case "deleteContentForward":
      return key("delete");
    default:
      return [];
  }
}

/** A key combination sent from the keys menu (`["control","alt","delete"]`). */
export function chord(keys: string[]): InteractiveInputEvent[] {
  const downs: InteractiveInputEvent[] = keys.map((key) => ({
    kind: "key",
    key,
    state: "down",
    modifiers: [],
    repeat: false,
  }));
  const ups: InteractiveInputEvent[] = [...keys].reverse().map((key) => ({
    kind: "key",
    key,
    state: "up",
    modifiers: [],
    repeat: false,
  }));
  return [...downs, ...ups];
}

const LONG_PRESS_MS = 550;
const TAP_SLOP_PX = 10;
const MOVE_INTERVAL_MS = 16;

interface Touch {
  id: number;
  x: number;
  y: number;
  startX: number;
  startY: number;
}

export interface InputControllerOptions {
  surface: HTMLElement;
  /** The element whose box is the remote screen (pointer mapping). */
  screen: () => HTMLElement;
  emit: Emit;
  keyOptions: () => KeyMapOptions;
  /** Ctrl/Cmd+V: the host clipboard is pushed first, then the keys sent. */
  onPaste?: (event: ClipboardEvent) => Promise<void> | void;
  /** After Ctrl/Cmd+C or X: the guest clipboard is about to change. */
  onCopyShortcut?: () => void;
  /** Every emitted user action (the skill recorder listens). */
  onAction?: (event: InteractiveInputEvent) => void;
}

/** Wires DOM events to input events. */
export class InputController {
  readonly textarea: HTMLTextAreaElement;
  private readonly off: Array<() => void> = [];
  private lastMove = 0;
  private readonly touches = new Map<number, Touch>();
  private longPress: number | null = null;
  private touchMode: "none" | "pointer" | "scroll" | "longpress" = "none";
  private pendingPaste: { resolve: () => void; timer: number } | null = null;
  private readonly heldKeys = new Map<string, InteractiveInputEvent>();
  enabled = true;

  constructor(private readonly options: InputControllerOptions) {
    const textarea = document.createElement("textarea");
    textarea.className = "cua-keys";
    textarea.setAttribute("aria-label", "Remote keyboard");
    textarea.autocapitalize = "off";
    textarea.autocomplete = "off";
    textarea.spellcheck = false;
    textarea.setAttribute("autocorrect", "off");
    options.surface.appendChild(textarea);
    this.textarea = textarea;
    this.wire();
  }

  focus(): void {
    this.textarea.focus({ preventScroll: true });
  }

  dispose(): void {
    this.releaseAll();
    for (const off of this.off.splice(0)) off();
    this.textarea.remove();
  }

  /** Releases every key still held down (focus loss). */
  releaseAll(): void {
    for (const event of this.heldKeys.values()) {
      if (event.kind === "key") this.send({ ...event, state: "up", repeat: false });
    }
    this.heldKeys.clear();
  }

  private send(event: InteractiveInputEvent): void {
    if (!this.enabled) return;
    this.options.emit(event);
    this.options.onAction?.(event);
  }

  private on<K extends keyof HTMLElementEventMap>(
    target: HTMLElement | Window | Document,
    type: K,
    handler: (event: HTMLElementEventMap[K]) => void,
    options?: AddEventListenerOptions,
  ): void {
    target.addEventListener(type, handler as EventListener, options);
    this.off.push(() => target.removeEventListener(type, handler as EventListener, options));
  }

  private point(event: { clientX: number; clientY: number }) {
    return normalizePoint(event.clientX, event.clientY, this.options.screen().getBoundingClientRect());
  }

  private wire(): void {
    const surface = this.options.surface;
    const textarea = this.textarea;

    this.on(textarea, "keydown", (event) => {
      const mapped = mapKey(event, "down", this.options.keyOptions());
      if (!mapped) return; // text arrives through beforeinput
      const isPaste = mapped.kind === "key" && mapped.key === "v" && mapped.modifiers.length > 0 && this.options.onPaste;
      if (isPaste) {
        // Let the browser fire `paste` (it carries the host clipboard without
        // a permission prompt); the keys follow once the guest has it.
        this.holdForPaste(mapped);
        return;
      }
      event.preventDefault();
      if (mapped.kind === "key") this.heldKeys.set(event.code || mapped.key, mapped);
      this.send(mapped);
      if (mapped.kind === "key" && (mapped.key === "c" || mapped.key === "x") && mapped.modifiers.length > 0) {
        this.options.onCopyShortcut?.();
      }
    });
    this.on(textarea, "keyup", (event) => {
      // Release exactly what went down for this physical key, even if the
      // modifiers changed in between (Ctrl released before C).
      const held = this.heldKeys.get(event.code || event.key);
      if (held && held.kind === "key") {
        event.preventDefault();
        this.heldKeys.delete(event.code || event.key);
        this.send({ ...held, state: "up", repeat: false });
        return;
      }
      const mapped = mapKey(event, "up", this.options.keyOptions());
      if (!mapped || !MODIFIER_KEYS.has(event.key)) return;
      event.preventDefault();
      this.send(mapped);
    });
    this.on(textarea, "beforeinput", (event) => {
      const input = event as InputEvent;
      if (input.isComposing || input.inputType === "insertCompositionText") return;
      event.preventDefault();
      for (const mapped of mapBeforeInput(input.inputType, input.data)) this.send(mapped);
    });
    this.on(textarea, "compositionend", (event) => {
      const data = (event as CompositionEvent).data;
      if (data) this.send({ kind: "text_commit", text: data });
      textarea.value = "";
    });
    this.on(textarea, "input", () => {
      // Anything that slipped past beforeinput (old engines).
      if (!textarea.value || (textarea as unknown as { isComposing?: boolean }).isComposing) return;
      const text = textarea.value;
      textarea.value = "";
      this.send({ kind: "text_commit", text });
    });
    this.on(textarea, "paste", (event) => {
      event.preventDefault();
      void this.pasted(event as ClipboardEvent);
    });
    this.on(textarea, "blur", () => this.releaseAll());

    // Pointer (mouse, pen, touch).
    this.on(surface, "pointerdown", (event) => {
      if ((event.target as HTMLElement | null)?.closest?.(".cua-chrome")) return;
      event.preventDefault();
      this.focus();
      if (event.pointerType === "touch") {
        this.touchDown(event);
        return;
      }
      surface.setPointerCapture?.(event.pointerId);
      this.send({
        kind: "pointer",
        phase: "down",
        button: pointerButton(event.button),
        ...this.point(event),
        modifiers: modifiersOf(event),
      });
    });
    this.on(surface, "pointermove", (event) => {
      if (event.pointerType === "touch") {
        this.touchMove(event);
        return;
      }
      const now = performance.now();
      if (now - this.lastMove < MOVE_INTERVAL_MS) return;
      this.lastMove = now;
      this.send({ kind: "pointer", phase: "move", button: null, ...this.point(event), modifiers: modifiersOf(event) });
    });
    this.on(surface, "pointerup", (event) => {
      if (event.pointerType === "touch") {
        this.touchUp(event);
        return;
      }
      this.send({
        kind: "pointer",
        phase: "up",
        button: pointerButton(event.button),
        ...this.point(event),
        modifiers: modifiersOf(event),
      });
    });
    this.on(surface, "pointercancel", (event) => {
      if (event.pointerType === "touch") {
        this.touches.clear();
        this.cancelLongPress();
        if (this.touchMode === "pointer") {
          this.send({ kind: "pointer", phase: "cancel", button: "left", ...this.point(event), modifiers: [] });
        }
        this.touchMode = "none";
      }
    });
    this.on(surface, "contextmenu", (event) => event.preventDefault());
    this.on(
      surface,
      "wheel",
      (event) => {
        event.preventDefault();
        if (event.ctrlKey && event.deltaMode === 0 && Math.abs(event.deltaY) < 50) {
          // Trackpad pinch arrives as ctrl+wheel: never zoom the page.
          return;
        }
        const scale = event.deltaMode === 1 ? 40 : event.deltaMode === 2 ? 400 : 1;
        this.send({
          kind: "scroll",
          ...this.point(event),
          delta_x: event.deltaX * scale,
          delta_y: event.deltaY * scale,
          phase: "none",
          momentum_phase: "none",
          precise: event.deltaMode === 0,
        });
      },
      { passive: false },
    );
  }

  private holdForPaste(keyEvent: InteractiveInputEvent): void {
    // Up to 300 ms for the `paste` event (none comes when the host clipboard
    // is empty or the engine withholds it); the keys go out either way.
    if (this.pendingPaste) window.clearTimeout(this.pendingPaste.timer);
    const release = () => {
      this.pendingPaste = null;
      if (keyEvent.kind !== "key") return;
      this.send(keyEvent);
      this.send({ ...keyEvent, state: "up", repeat: false });
    };
    const timer = window.setTimeout(release, 300);
    this.pendingPaste = { resolve: release, timer };
  }

  private async pasted(event: ClipboardEvent): Promise<void> {
    const pending = this.pendingPaste;
    if (pending) window.clearTimeout(pending.timer);
    try {
      await this.options.onPaste?.(event);
    } finally {
      if (pending) pending.resolve();
      else {
        // A paste without the shortcut (context menu, mobile): type the text.
        const text = event.clipboardData?.getData("text/plain");
        if (text) this.send({ kind: "text_commit", text });
      }
    }
  }

  // ------------------------------------------------------------ touch

  private touchDown(event: PointerEvent): void {
    this.touches.set(event.pointerId, {
      id: event.pointerId,
      x: event.clientX,
      y: event.clientY,
      startX: event.clientX,
      startY: event.clientY,
    });
    if (this.touches.size === 1) {
      this.touchMode = "none";
      this.cancelLongPress();
      this.longPress = window.setTimeout(() => {
        this.longPress = null;
        if (this.touchMode !== "none") return;
        this.touchMode = "longpress";
        const at = this.point(event);
        this.send({ kind: "pointer", phase: "down", button: "right", ...at, modifiers: [] });
        this.send({ kind: "pointer", phase: "up", button: "right", ...at, modifiers: [] });
      }, LONG_PRESS_MS);
    } else if (this.touches.size === 2) {
      this.cancelLongPress();
      if (this.touchMode === "pointer") {
        const first = [...this.touches.values()][0]!;
        this.send({ kind: "pointer", phase: "cancel", button: "left", ...this.point({ clientX: first.x, clientY: first.y }), modifiers: [] });
      }
      this.touchMode = "scroll";
    }
  }

  private touchMove(event: PointerEvent): void {
    const touch = this.touches.get(event.pointerId);
    if (!touch) return;
    const dx = event.clientX - touch.x;
    const dy = event.clientY - touch.y;
    touch.x = event.clientX;
    touch.y = event.clientY;
    if (this.touchMode === "scroll") {
      // Natural scrolling: content follows the fingers.
      if (touch.id !== [...this.touches.keys()][0]) return;
      this.send({
        kind: "scroll",
        ...this.point(event),
        delta_x: -dx,
        delta_y: -dy,
        phase: "none",
        momentum_phase: "none",
        precise: true,
      });
      return;
    }
    const moved = Math.hypot(event.clientX - touch.startX, event.clientY - touch.startY) > TAP_SLOP_PX;
    if (this.touchMode === "none" && moved) {
      this.cancelLongPress();
      this.touchMode = "pointer";
      this.send({ kind: "pointer", phase: "move", button: null, ...this.point({ clientX: touch.startX, clientY: touch.startY }), modifiers: [] });
      this.send({ kind: "pointer", phase: "down", button: "left", ...this.point({ clientX: touch.startX, clientY: touch.startY }), modifiers: [] });
    }
    if (this.touchMode === "pointer") {
      this.send({ kind: "pointer", phase: "move", button: null, ...this.point(event), modifiers: [] });
    }
  }

  private touchUp(event: PointerEvent): void {
    const touch = this.touches.get(event.pointerId);
    this.touches.delete(event.pointerId);
    if (!touch) return;
    if (this.touchMode === "none" && this.touches.size === 0) {
      // A tap: click where the finger was.
      this.cancelLongPress();
      const at = this.point({ clientX: touch.startX, clientY: touch.startY });
      this.send({ kind: "pointer", phase: "move", button: null, ...at, modifiers: [] });
      this.send({ kind: "pointer", phase: "down", button: "left", ...at, modifiers: [] });
      this.send({ kind: "pointer", phase: "up", button: "left", ...at, modifiers: [] });
    } else if (this.touchMode === "pointer" && this.touches.size === 0) {
      this.send({ kind: "pointer", phase: "up", button: "left", ...this.point(event), modifiers: [] });
    }
    if (this.touches.size === 0) this.touchMode = "none";
  }

  private cancelLongPress(): void {
    if (this.longPress !== null) window.clearTimeout(this.longPress);
    this.longPress = null;
  }
}
