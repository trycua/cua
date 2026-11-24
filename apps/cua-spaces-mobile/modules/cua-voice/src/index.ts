// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { requireOptionalNativeModule } from "expo-modules-core";

import type { PermissionWord } from "../../../src/core/phone";

export interface VoiceStatus {
  microphone: PermissionWord;
  speech: PermissionWord;
  /** A recognizer exists for the language and is available now. */
  recognizer: boolean;
  /** It runs on the phone: no audio leaves it. */
  onDevice: boolean;
  listening: boolean;
}

export interface VoiceNative {
  available: boolean;
  status(locale?: string | null): Promise<VoiceStatus>;
  /** Asks for microphone and speech access (two system prompts) and answers where each stands. */
  requestPermissions(): Promise<{ microphone: PermissionWord; speech: PermissionWord }>;
  start(locale?: string | null): Promise<void>;
  stop(): Promise<void>;
  addListener(event: "onResult", cb: (e: { text: string; final: boolean }) => void): { remove(): void };
  addListener(event: "onEnd", cb: () => void): { remove(): void };
  addListener(event: "onError", cb: (e: { kind: string; message: string }) => void): { remove(): void };
}

/** The native speech module, or null where there is none (Android for now, and web). */
export const nativeVoice: VoiceNative | null = requireOptionalNativeModule<VoiceNative>("CuaVoice");
