// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * What a link or `window.open` in the app may hand to the OS: a web page, or
 * the diagnostic report's email to Cua support (a `mailto:` to that one
 * address, subject and body only). Anything else stays closed.
 */
const SUPPORT_MAILTO = /^mailto:support@trycua\.com(\?((subject|body)=[A-Za-z0-9\-._~%]*)(&(subject|body)=[A-Za-z0-9\-._~%]*)*)?$/;

export function opensOutside(url: string): boolean {
  return /^https?:\/\//.test(url) || SUPPORT_MAILTO.test(url);
}
