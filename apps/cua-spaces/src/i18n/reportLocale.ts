// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { telemetryBridge } from "../native/telemetry";
import { currentUiLocale } from "./index";

/** Tell telemetry which interface language is in use (`ui_locale` on `cua_app_active`). */
export function reportUiLocale(): void {
  telemetryBridge().setUiLocale(currentUiLocale());
}
