// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { SendHorizontalIcon, TriangleAlertIcon, XIcon } from "lucide-react";
import { useImperativeHandle, useLayoutEffect, useRef, useState, type KeyboardEvent, type ReactNode, type Ref } from "react";

import { activeMention, insertMention, matchMentions, mentionedHandles, mentionSegments, startsWithMention, type MentionCandidate } from "@/lib/mentions";
import { cn } from "@/lib/utils";

import { MemberAvatar } from "./avatar";

/** The composer, for the thread's Reply button. */
export interface ComposerHandle {
  /** Puts `@handle ` at the start (once) and focuses the box, caret after what was already written. */
  reply(handle: string): void;
  focus(): void;
}

/** Who the next message goes to without an @ typed: shown as a chip with an X. */
export interface ComposerAddressee {
  handles: string[];
  /** How the chip names a handle (`Claude Code`). */
  label: (handle: string) => string;
  /** Why the agent can't take it yet, in words ("Claude Code needs to be connected"), or null. */
  problem: (handle: string) => string | null;
  /** The chip's X. */
  onRemove: (handle: string) => void;
  /** Every handle an agent answers to: an @ of one of them in the text means the chip is not used. */
  agentHandles: ReadonlySet<string>;
}

/**
 * The thread's message box. Enter sends, Shift+Enter is a new line. `@`
 * opens a picker of the Space's members (agents first): arrows move, Enter
 * or Tab picks, Escape closes. A mention of a member draws as a chip while
 * you type (a highlight layer under a transparent textarea, so the caret
 * and selection stay the system's own).
 */
export function Composer({
  placeholder,
  candidates,
  handles,
  agentHarness,
  onSend,
  disabled,
  disabledReason,
  footer,
  className,
  addressee,
  composerRef,
}: {
  placeholder: string;
  candidates: readonly MentionCandidate[];
  handles: ReadonlySet<string>;
  /** An agent's harness by handle, for the picker's marks. */
  agentHarness: (handle: string) => string;
  onSend: (text: string) => Promise<void>;
  disabled?: boolean;
  /** Why you can't post here (a guest's view-only share): the box stays, off, and says so. */
  disabledReason?: string | null;
  /** A row inside the box, beside Send (the Agents tab's "In <Space> · Members see it"). */
  footer?: ReactNode;
  className?: string;
  /** Who a message goes to when it names no agent (the sticky chip). */
  addressee?: ComposerAddressee;
  composerRef?: Ref<ComposerHandle>;
}) {
  const off = disabled || !!disabledReason;
  const [text, setText] = useState("");
  const [caret, setCaret] = useState(0);
  const [picked, setPicked] = useState(0);
  const [closedAt, setClosedAt] = useState<number | null>(null);
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const area = useRef<HTMLTextAreaElement | null>(null);
  const layer = useRef<HTMLDivElement | null>(null);
  const nextCaret = useRef<number | null>(null);

  useImperativeHandle(
    composerRef,
    () => ({
      focus: () => area.current?.focus(),
      reply: (handle) => {
        const el = area.current;
        if (!el) return;
        const cur = el.value;
        // Once: a box already addressed to them keeps its text.
        const next = startsWithMention(cur, handle) ? cur : `@${handle} ${cur.replace(/^\s+/, "")}`;
        const end = next.length;
        nextCaret.current = end;
        setText(next);
        setCaret(end);
        setError(null);
        el.focus();
        el.setSelectionRange(Math.min(end, el.value.length), Math.min(end, el.value.length));
      },
    }),
    [],
  );

  const mention = activeMention(text, caret);
  const matches = mention && closedAt !== mention.start ? matchMentions(mention.query, candidates) : [];
  const open = matches.length > 0;
  const index = Math.min(picked, Math.max(0, matches.length - 1));

  // Grows with its text up to six lines; the caret lands after a picked name.
  useLayoutEffect(() => {
    const el = area.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 6 * 20 + 16)}px`;
    if (layer.current) layer.current.scrollTop = el.scrollTop;
    if (nextCaret.current !== null) {
      el.setSelectionRange(nextCaret.current, nextCaret.current);
      nextCaret.current = null;
    }
  }, [text]);

  const pick = (c: MentionCandidate) => {
    if (!mention || c.disabled) return;
    const next = insertMention(text, mention.start, caret, c.handle);
    nextCaret.current = next.caret;
    setText(next.text);
    setCaret(next.caret);
    setPicked(0);
  };

  // Typing an @ of an agent replaces the chip for this message.
  const chips = addressee && !mentionedHandles(text, addressee.agentHandles).some((h) => addressee.agentHandles.has(h)) ? addressee.handles : [];

  const send = () => {
    const t = text.trim();
    if (!t || sending || off) return;
    setSending(true);
    setError(null);
    onSend(t).then(
      () => {
        setText("");
        setCaret(0);
        setSending(false);
      },
      (e: unknown) => {
        setSending(false);
        setError(e instanceof Error ? e.message : String(e));
      },
    );
  };

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.nativeEvent.isComposing) return;
    if (open) {
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        setPicked((index + (e.key === "ArrowDown" ? 1 : matches.length - 1)) % matches.length);
        return;
      }
      if (e.key === "Enter" || e.key === "Tab") {
        e.preventDefault();
        pick(matches[index]!);
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        setClosedAt(mention!.start);
        return;
      }
    }
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      send();
      return;
    }
    // Backspace in an empty box takes the last chip off, like a recipient field.
    if (e.key === "Backspace" && !text && chips.length && addressee) {
      e.preventDefault();
      addressee.onRemove(chips[chips.length - 1]!);
    }
  };

  const sync = (el: HTMLTextAreaElement) => {
    setText(el.value);
    setCaret(el.selectionStart ?? el.value.length);
    setError(null);
  };

  return (
    <div className={cn("relative", className)}>
      {open ? (
        <div
          role="listbox"
          aria-label="Mention"
          data-mention-picker=""
          className="absolute bottom-full left-0 z-20 mb-2 w-72 overflow-hidden rounded-lg border bg-popover p-1 text-popover-foreground shadow-float"
        >
          {matches.map((c, i) => (
            <div
              key={`${c.kind}:${c.handle}`}
              role="option"
              aria-selected={i === index}
              aria-disabled={c.disabled || undefined}
              data-mention-option={c.handle}
              onMouseDown={(e) => {
                e.preventDefault();
                pick(c);
              }}
              onMouseEnter={() => setPicked(i)}
              className={cn("flex h-9 cursor-default items-center gap-2.5 rounded-lg px-2 text-[13px]", i === index && "bg-accent", c.disabled && "opacity-60")}
            >
              <MemberAvatar kind={c.kind} name={c.label} harness={c.kind === "agent" ? (c.harness ?? agentHarness(c.handle)) : ""} size={24} />
              <span className="min-w-0 flex-1 truncate">{c.label}</span>
              <span className="text-xs text-muted-foreground" data-mention-detail={c.detail ? "" : undefined}>
                {c.detail ?? `@${c.handle}`}
              </span>
            </div>
          ))}
        </div>
      ) : null}
      <div className={cn("relative flex gap-2 rounded-lg bg-muted py-1.5 pr-1.5 pl-3 focus-within:ring-2 focus-within:ring-ring/50", footer ? "flex-col" : "items-end")}>
        <div className="min-w-0 flex-1">
          {chips.length && addressee ? (
            <div role="group" aria-label={`Talking to ${chips.map(addressee.label).join(", ")}`} data-addressee="" className="flex flex-wrap items-center gap-1.5 pt-1">
              {chips.map((h) => {
                const problem = addressee.problem(h);
                return (
                  <span
                    key={h}
                    data-addressee-chip={h}
                    data-addressee-problem={problem ? "" : undefined}
                    className={cn("inline-flex h-6 max-w-full items-center gap-1 rounded-lg bg-background pr-0.5 pl-2 text-xs text-foreground")}
                  >
                    {problem ? <TriangleAlertIcon aria-hidden className="size-3 shrink-0 text-warning" /> : null}
                    <span className="min-w-0 truncate">{problem ?? addressee.label(h)}</span>
                    <button
                      type="button"
                      data-addressee-remove={h}
                      aria-label={`Stop talking to ${addressee.label(h)}`}
                      onClick={() => {
                        addressee.onRemove(h);
                        area.current?.focus();
                      }}
                      className="inline-flex size-5 shrink-0 cursor-default items-center justify-center rounded-md text-muted-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
                    >
                      <XIcon className="size-3" />
                    </button>
                  </span>
                );
              })}
            </div>
          ) : null}
          <div className="relative">
          {/* What you typed, with mentions as chips; the textarea above it draws only the caret. */}
          <div ref={layer} aria-hidden className="pointer-events-none absolute inset-0 overflow-hidden py-1.5 text-[13px] leading-5 break-words whitespace-pre-wrap text-foreground">
            {mentionSegments(text, handles).map((s, i) =>
              s.mention ? (
                <span key={i} className="text-brand-strong">
                  {s.text}
                </span>
              ) : (
                <span key={i}>{s.text}</span>
              ),
            )}
            {"​"}
          </div>
          <textarea
            ref={area}
            data-composer=""
            rows={1}
            value={text}
            placeholder={disabledReason || placeholder}
            aria-label={placeholder}
            data-composer-off={disabledReason ? "" : undefined}
            disabled={off}
            onChange={(e) => sync(e.currentTarget)}
            onSelect={(e) => setCaret(e.currentTarget.selectionStart ?? 0)}
            onKeyDown={onKeyDown}
            onScroll={(e) => {
              if (layer.current) layer.current.scrollTop = e.currentTarget.scrollTop;
            }}
            className={cn(
              "relative block w-full resize-none bg-transparent py-1.5 text-[13px] leading-5 break-words text-transparent caret-foreground outline-none placeholder:text-muted-foreground disabled:opacity-50",
              // A guest's reason reads as a sentence, not a faded hint.
              disabledReason && "disabled:opacity-100",
            )}
          />
          </div>
        </div>
        {disabledReason ? null : footer ? (
          <div className="flex items-center gap-2" data-composer-footer="">
            <div className="min-w-0 flex-1">{footer}</div>
            {sendButton()}
          </div>
        ) : (
          sendButton()
        )}
      </div>
      {error ? <p className="mt-1.5 px-1 text-xs text-destructive">{error}</p> : null}
    </div>
  );

  function sendButton() {
    return (
        <button
          type="button"
          data-send=""
          aria-label="Send"
          disabled={!text.trim() || sending || off}
          onClick={send}
          className="mb-0.5 inline-flex size-7 shrink-0 cursor-default items-center justify-center rounded-lg text-muted-foreground outline-none transition-colors hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60 disabled:opacity-40 enabled:bg-primary enabled:text-primary-foreground enabled:hover:bg-primary/90"
        >
          <SendHorizontalIcon className="size-3.5" />
        </button>
    );
  }
}
