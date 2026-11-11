// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { Msg } from "../bridge/contracts/teleport";
import { tMsg } from "../i18n";

/**
 * A core message (`message::Msg`) as text in the current language: its code
 * is a catalog key (`src/i18n/locales/<lng>/<area>.json`), with the
 * variables filled in. An unknown code shows as it is.
 */
export function messageText(msg: Msg): string {
  return tMsg(msg);
}
