// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Push-to-talk dictation into the message box. The phone's speech recognizer does the listening (on the phone when
 * the language supports it; nothing goes to a Cua server for transcription). The words for every state (permission
 * never asked, refused, no recognizer, errors) and where a transcript lands in the box are the app core's
 * (`voice.view`, `voice.insert`); this hook runs the native module and the permission prompts.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { Linking } from "react-native";

import { voiceInsert, voiceView, type PermissionWord, type VoiceView } from "../../core/phone";
import { nativeVoice, type VoiceNative } from "../../../modules/cua-voice/src";

let override: VoiceNative | null | undefined;

/** Tests (and nothing else) install another speech module. */
export function setVoiceForTests(v: VoiceNative | null | undefined): void {
  override = v;
}

const voiceModule = (): VoiceNative | null => (override === undefined ? nativeVoice : override);

export interface DictationBox {
  text: string;
  start: number;
  end: number;
}

export interface Dictation {
  /** A speech recognizer exists on this platform (the mic button is shown). */
  supported: boolean;
  /** The core's words for the current state; null where the core is not linked. */
  view: VoiceView | null;
  listening: boolean;
  /** Press and hold: starts listening (or asks for permission on the first press). */
  pressIn(): void;
  /** Release: stops and keeps what was heard. */
  pressOut(): void;
  /** One press for people who cannot hold (VoiceOver): starts, or stops while listening. */
  toggle(): void;
  /** The notice's action: open Settings, or try again. */
  act(action: "open-settings" | "retry"): void;
  dismissError(): void;
}

/**
 * `read()` gives the box (its text and selection) when dictation starts; `write(text, cursor)` puts the box back as
 * the recognizer hears more. Every update starts again from the box as it was at the start, so a changed guess
 * replaces the earlier one instead of piling up.
 */
export function useDictation(read: () => DictationBox, write: (text: string, cursor: number) => void): Dictation {
  const native = voiceModule();
  const [perm, setPerm] = useState<{ microphone: PermissionWord; speech: PermissionWord; recognizer: boolean; onDevice: boolean }>({
    microphone: "undetermined",
    speech: "undetermined",
    recognizer: true,
    onDevice: false,
  });
  const [listening, setListening] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const base = useRef<DictationBox | null>(null);
  const held = useRef(false);
  const starting = useRef<Promise<void> | null>(null);
  const listeningNow = useRef(false);
  listeningNow.current = listening;

  const refresh = useCallback(async () => {
    if (!native) return;
    try {
      const s = await native.status(null);
      setPerm({ microphone: s.microphone, speech: s.speech, recognizer: s.recognizer, onDevice: s.onDevice });
      setListening(s.listening);
    } catch {
      /* the status is a convenience */
    }
  }, [native]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    if (!native) return;
    const subs = [
      native.addListener("onResult", (e) => {
        const b = base.current;
        if (!b) return;
        const r = voiceInsert(b.text, b.start, b.end, e.text);
        if (r) write(r.text, r.cursor);
      }),
      native.addListener("onEnd", () => {
        setListening(false);
        base.current = null;
        void refresh();
      }),
      native.addListener("onError", (e) => setError(e.kind)),
    ];
    return () => subs.forEach((s) => s.remove());
  }, [native, write, refresh]);

  const start = useCallback(async () => {
    if (!native) return;
    setError(null);
    // The first press asks the system (two prompts); it listens from the next one.
    if (perm.microphone === "undetermined" || perm.speech === "undetermined") {
      try {
        const p = await native.requestPermissions();
        setPerm((s) => ({ ...s, microphone: p.microphone, speech: p.speech }));
      } catch {
        /* a refusal shows as the denied state */
      }
      void refresh();
      return;
    }
    if (perm.microphone !== "granted" || perm.speech !== "granted") return;
    base.current = read();
    try {
      await native.start(null);
      setListening(true);
    } catch {
      base.current = null;
      setError("other");
      void refresh();
    }
  }, [native, perm.microphone, perm.speech, read, refresh]);

  const stop = useCallback(async () => {
    if (!native) return;
    try {
      await native.stop();
    } catch {
      /* it ends by itself */
    }
  }, [native]);

  const view = native
    ? voiceView({ microphone: perm.microphone, speech: perm.speech, recognizerAvailable: perm.recognizer, onDevice: perm.onDevice, listening, error })
    : null;

  return {
    supported: native !== null,
    view,
    listening,
    pressIn() {
      held.current = true;
      const p = start().finally(() => {
        if (starting.current === p) starting.current = null;
      });
      starting.current = p;
    },
    pressOut() {
      held.current = false;
      const p = starting.current;
      // Let go while it is still starting: it stops as soon as it has started.
      if (p) void p.then(() => (held.current ? undefined : stop()));
      else if (listeningNow.current) void stop();
    },
    toggle() {
      if (listening) void stop();
      else void start();
    },
    act(action) {
      if (action === "open-settings") void Linking.openSettings();
      else {
        setError(null);
        void start();
      }
    },
    dismissError: () => setError(null),
  };
}
