// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * What this browser can do, probed once. Decides the codecs the viewer
 * asks for (WebCodecs H.264 when the engine decodes it, PNG frames
 * otherwise) and which features the toolbar offers.
 */

export interface BrowserSupport {
  secureContext: boolean;
  h264: boolean;
  opusDecode: boolean;
  opusEncode: boolean;
  audioWorklet: boolean;
  microphone: boolean;
  clipboardRead: boolean;
  clipboardWrite: boolean;
  clipboardImages: boolean;
  folderAccess: boolean;
  folderObserver: boolean;
  fullscreen: boolean;
  keyboardLock: boolean;
  mediaRecorder: boolean;
}

async function supportedVideo(): Promise<boolean> {
  if (typeof VideoDecoder === "undefined") return false;
  for (const codec of ["avc1.42E01F", "avc1.4D401F", "avc1.64001F"]) {
    try {
      const result = await VideoDecoder.isConfigSupported({ codec, optimizeForLatency: true });
      if (result.supported) return true;
    } catch {
      // try the next profile
    }
  }
  return false;
}

async function supportedAudio(kind: "decode" | "encode"): Promise<boolean> {
  try {
    if (kind === "decode") {
      if (typeof AudioDecoder === "undefined") return false;
      return Boolean((await AudioDecoder.isConfigSupported({ codec: "opus", sampleRate: 48000, numberOfChannels: 2 })).supported);
    }
    if (typeof AudioEncoder === "undefined") return false;
    return Boolean((await AudioEncoder.isConfigSupported({ codec: "opus", sampleRate: 48000, numberOfChannels: 1, bitrate: 32000 })).supported);
  } catch {
    return false;
  }
}

export async function probe(): Promise<BrowserSupport> {
  const g = globalThis as unknown as Record<string, unknown>;
  const nav = typeof navigator !== "undefined" ? navigator : undefined;
  const [h264, opusDecode, opusEncode] = await Promise.all([supportedVideo(), supportedAudio("decode"), supportedAudio("encode")]);
  return {
    secureContext: Boolean(g.isSecureContext),
    h264,
    opusDecode,
    opusEncode,
    audioWorklet: typeof AudioWorkletNode !== "undefined",
    microphone: Boolean(nav?.mediaDevices?.getUserMedia),
    clipboardRead: Boolean(nav?.clipboard?.read || nav?.clipboard?.readText),
    clipboardWrite: Boolean(nav?.clipboard?.writeText),
    clipboardImages: typeof g.ClipboardItem !== "undefined" && Boolean(nav?.clipboard?.write),
    folderAccess: typeof g.showDirectoryPicker === "function",
    folderObserver: typeof g.FileSystemObserver === "function",
    fullscreen: typeof document !== "undefined" && Boolean(document.documentElement.requestFullscreen),
    keyboardLock: Boolean((nav as unknown as { keyboard?: { lock?: unknown } })?.keyboard?.lock),
    mediaRecorder: typeof g.MediaRecorder !== "undefined",
  };
}
