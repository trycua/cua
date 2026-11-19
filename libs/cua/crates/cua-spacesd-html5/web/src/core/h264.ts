// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * H.264 helpers for the per-window RCDP client.
 *
 * Kept out of `WindowStream.tsx` so that file exports only its React component
 * (mixing a component with other exports disables React Fast Refresh).
 */

/**
 * Build a WebCodecs `avc1.PPCCLL` codec string from the SPS of an Annex-B
 * access unit (profile_idc, constraint flags, level_idc). Returns null when no
 * SPS is present.
 */
export function avcCodecFromAnnexB(data: Uint8Array): string | null {
  let i = 0;
  while (i + 4 < data.length) {
    // Match a 3- or 4-byte start code, then read the NAL header byte.
    const isStart3 = data[i] === 0 && data[i + 1] === 0 && data[i + 2] === 1;
    const isStart4 =
      data[i] === 0 && data[i + 1] === 0 && data[i + 2] === 0 && data[i + 3] === 1;
    if (isStart3 || isStart4) {
      const nalStart = i + (isStart4 ? 4 : 3);
      const nalType = (data[nalStart] ?? 0) & 0x1f;
      if (nalType === 7 && nalStart + 3 < data.length) {
        const profile = data[nalStart + 1] ?? 0;
        const constraints = data[nalStart + 2] ?? 0;
        const level = data[nalStart + 3] ?? 0;
        const hex = (v: number) => v.toString(16).padStart(2, "0").toUpperCase();
        return `avc1.${hex(profile)}${hex(constraints)}${hex(level)}`;
      }
      i = nalStart;
    } else {
      i += 1;
    }
  }
  return null;
}
