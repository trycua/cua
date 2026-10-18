// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import cuaMark from "../assets/brand/cua-mark-white.svg";

/** Back to front: offset, fill token, shadow strength. */
const LAYERS = [
  { offset: 0, fill: "var(--stack-mark-back)", shadow: 0.18 },
  { offset: 18, fill: "var(--stack-mark-mid)", shadow: 0.24 },
  { offset: 36, fill: "var(--stack-mark-front)", shadow: 0.32 },
] as const;

const SIDE = 84;
const RADIUS = 20;
const LOGO = 50;

/**
 * The onboarding mark: the white Cua logo in a rounded square, three times,
 * offset diagonally and overlapping like a stack of three Spaces. Opaque
 * fills (no glass), depth from the fills and a soft shadow per layer; the
 * fill tokens have light and dark values (`.stack-mark` in desktop.css).
 * Vector throughout, so it stays crisp at any size.
 *
 * The app icon and the menu bar template use the same stack
 * (scripts/icons/gen-icon-svgs.py).
 */
export function SpacesStackMark({ size = 120 }: { size?: number }) {
  const view = SIDE + LAYERS[LAYERS.length - 1]!.offset + 8;
  return (
    <svg
      className="stack-mark"
      width={size}
      height={size}
      viewBox={`-4 -2 ${view} ${view}`}
      role="img"
      aria-label="Cua Spaces"
    >
      <defs>
        {LAYERS.map((l, i) => (
          <filter key={i} id={`stack-mark-shadow-${i}`} x="-30%" y="-30%" width="160%" height="160%">
            <feDropShadow dx="0" dy="2.5" stdDeviation="3" floodColor="#000" floodOpacity={l.shadow} />
          </filter>
        ))}
      </defs>
      {LAYERS.map((l, i) => (
        <g key={i} transform={`translate(${l.offset} ${l.offset})`}>
          <rect
            width={SIDE}
            height={SIDE}
            rx={RADIUS}
            fill={l.fill}
            stroke="var(--stack-mark-edge)"
            strokeWidth="1"
            filter={`url(#stack-mark-shadow-${i})`}
          />
          <image
            href={cuaMark}
            x={(SIDE - LOGO) / 2}
            y={(SIDE - LOGO) / 2}
            width={LOGO}
            height={LOGO}
            preserveAspectRatio="xMidYMid meet"
          />
        </g>
      ))}
    </svg>
  );
}
