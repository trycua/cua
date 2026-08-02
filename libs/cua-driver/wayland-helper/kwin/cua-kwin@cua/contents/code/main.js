/*
 * SPDX-FileCopyrightText: 2026 Cua contributors
 * SPDX-License-Identifier: MIT
 *
 * Passive KWin 6 adapter primitives for cua-driver. Loading this package does
 * not enumerate, activate, raise, minimize, or otherwise mutate any window.
 * The trusted driver transport invokes exactly one function per request and
 * JSON-encodes its returned object.
 */

const CUA_KWIN_PROTOCOL_VERSION = 1;

function cuaKWinWindowUuid(window) {
  if (!window || window.internalId === null || window.internalId === undefined) {
    return '';
  }
  if (typeof window.internalId === 'string') {
    return window.internalId;
  }
  return window.internalId.toString();
}

/**
 * Return a fresh, compositor-owned snapshot. Later entries in `stackingOrder`
 * cover earlier entries, so the array index is the authoritative stacking
 * position for this snapshot.
 */
function cuaKWinSnapshot() {
  const stackingOrder = workspace.stackingOrder;
  const windows = [];

  for (let stacking = 0; stacking < stackingOrder.length; stacking += 1) {
    const window = stackingOrder[stacking];
    const geometry = window.frameGeometry;
    const minimized = Boolean(window.minimized);
    const hidden = Boolean(window.hidden);

    windows.push({
      uuid: cuaKWinWindowUuid(window),
      pid: Number(window.pid),
      title: window.caption === null || window.caption === undefined ? '' : String(window.caption),
      // KWin reports frame geometry in logical pixels that are fractional under
      // fractional display scaling. cua-driver's snapshot contract is integer
      // screen pixels, so round here rather than emit a float the driver would
      // reject as a malformed snapshot.
      x: Math.round(Number(geometry.x)),
      y: Math.round(Number(geometry.y)),
      width: Math.round(Number(geometry.width)),
      height: Math.round(Number(geometry.height)),
      active: Boolean(window.active),
      minimized,
      hidden,
      visible: !minimized && !hidden,
      stacking,
    });
  }

  const activeWindow = workspace.activeWindow;
  const activeWindowUuid = activeWindow ? cuaKWinWindowUuid(activeWindow) : null;
  return {
    protocol_version: CUA_KWIN_PROTOCOL_VERSION,
    active_window_uuid: activeWindowUuid || null,
    windows,
  };
}

/**
 * Request activation of exactly one full KWin internal UUID. The return value
 * only reports whether KWin accepted the request; cua-driver must take a new
 * snapshot and verify `active_window_uuid` before sending global input.
 */
function cuaKWinActivate(uuid) {
  const requestedUuid = typeof uuid === 'string' ? uuid : '';
  const response = {
    protocol_version: CUA_KWIN_PROTOCOL_VERSION,
    accepted: false,
    target_uuid: requestedUuid,
  };
  if (requestedUuid.length === 0) {
    return response;
  }

  const stackingOrder = workspace.stackingOrder;
  const matches = [];
  for (let index = 0; index < stackingOrder.length; index += 1) {
    const window = stackingOrder[index];
    if (cuaKWinWindowUuid(window) === requestedUuid) {
      matches.push(window);
    }
  }
  if (matches.length !== 1) {
    return response;
  }

  const target = matches[0];
  try {
    if (target.minimized) {
      target.minimized = false;
    }
    workspace.raiseWindow(target);
    workspace.activeWindow = target;
    response.accepted = true;
  } catch (_error) {
    response.accepted = false;
  }
  return response;
}
