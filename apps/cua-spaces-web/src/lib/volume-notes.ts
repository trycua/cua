// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Why a Space has no Cua Volume, in plain words. The cua daemon lists the
 * Spaces that connected without one (`volume_errors` in
 * `volume_sync_status` and `volume_mount_status`) with its own reason:
 * the Space's cua-spacesd predates the volume, or the guest can't mount
 * it (a container on runc has no /dev/fuse). Without a mount, the
 * Space's volume folder is only a folder inside the Space: what is saved
 * there doesn't sync. Newer images make it read-only and say why
 * (cua-spacesd 2c6a8a10f); older ones (cua-spacesd 0.5.x) leave it
 * writable, so the app says it here. The Linux images (libs/images/linux,
 * libs/images/omarchy) still need a rebuild with the new cua-spacesd for
 * the read-only folder itself.
 */

const PREFIXES = [/^no Cua Volume in this Space:\s*/i, /^Cua Volume not mounted:\s*/i];

const STAYS = "Files saved in its volume folder stay in this Space and don't sync.";

/** The note for one daemon reason. */
export function volumeUnavailableNote(error: string): string {
  const why = PREFIXES.reduce((s, p) => s.replace(p, ""), error.trim());
  if (/predates/i.test(why)) return `Cua Volume isn't available in this Space (its image predates it). ${STAYS}`;
  if (/\/dev\/fuse|\bfuse\b/i.test(why)) {
    return `Cua Volume isn't available in this Space: its container can't mount it (no FUSE on runc). ${STAYS}`;
  }
  const reason = why.replace(/\.$/, "");
  return reason ? `Cua Volume isn't available in this Space: ${reason}. ${STAYS}` : `Cua Volume isn't available in this Space. ${STAYS}`;
}
