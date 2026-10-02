// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { SpaceDetailView } from "../model/window";
import { CopyButton, type WriteText } from "./CopyButton";
import { Sym } from "./desktop/Sym";

/**
 * A Space's facts (the core's `detail_live`): one line each, the full value
 * (and an image's digest) as the tooltip, a copy button after Image and
 * Identifier, and a warning symbol with its tooltip where the core sets one
 * (an emulated local Space's Architecture).
 */
export function FactList({ facts, copyText }: { facts: SpaceDetailView["facts"]; copyText?: WriteText }) {
  return (
    <dl className="dw-list" aria-label="Details">
      {facts.map((fact) => (
        <div key={fact.label}>
          <dt>{fact.label}</dt>
          <dd className="dw-fact-value">
            <span className={fact.copy ? "dw-mono" : undefined} title={fact.help ?? fact.value}>
              {fact.value}
            </span>
            {fact.warning && (
              <span className="dw-fact-warning" role="img" aria-label={fact.warning.help} title={fact.warning.help}>
                <Sym name={fact.warning.symbol} size={12} />
              </span>
            )}
            {fact.copy && <CopyButton copy={fact.copy} write={copyText} />}
          </dd>
        </div>
      ))}
    </dl>
  );
}
