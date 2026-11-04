// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { ReactNode } from "react";


/**
 * One onboarding page in the two-column layout: title and one sentence on
 * the left; the details (paths, agents, choices) in a card on the right.
 *
 * The buttons sit in the window's bottom corners like macOS Setup
 * Assistant (the SwiftUI app's `OnboardingView` places them the same):
 * Back bottom-left; Skip, then the primary action, bottom-right with the
 * primary in the corner. Which buttons show and their labels are the app
 * core's (`onboarding::view`); only the placement is this layout's.
 */
export function Page({
  title,
  lede,
  back,
  skip,
  primary,
  below,
  children,
}: {
  /** Unused: pages carry no decorative glyph. */
  icon?: string;
  title: string;
  lede: ReactNode;
  /** Bottom-left. */
  back?: ReactNode;
  /** Bottom-right, left of the primary action. */
  skip?: ReactNode;
  /** Bottom-right corner. */
  primary?: ReactNode;
  /** Full width under both columns. */
  below?: ReactNode;
  children?: ReactNode;
}) {
  const footer = back || skip || primary;
  return (
    <div className="ob-page">
      <div className="ob-page-left">
        <h2 className="onboarding-title">{title}</h2>
        {lede ? <div className="host-setup-lede">{lede}</div> : null}
      </div>
      {children ? <div className="ob-page-card">{children}</div> : null}
      {below ? <div className="ob-page-below">{below}</div> : null}
      {footer ? (
        <footer className="ob-footer">
          <div className="ob-footer-start">{back}</div>
          <div className="ob-footer-end">
            {skip}
            {primary}
          </div>
        </footer>
      ) : null}
    </div>
  );
}
