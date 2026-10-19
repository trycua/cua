// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { SfIcon } from "../SfIcon";

/**
 * The main window's icons: the real SF Symbol on macOS (so toolbar and
 * sidebar glyphs match the system), with a small stroke fallback elsewhere.
 */
const PATHS: Record<string, string> = {
  plus: "M12 5v14M5 12h14",
  terminal: "M3.5 5h17v14h-17zM7.5 10l3 2.5-3 2.5M12.5 15.5h4",
  magnifyingglass: "M10.5 17a6.5 6.5 0 1 0 0-13 6.5 6.5 0 0 0 0 13Zm4.7-1.8L20 20",
  cloud: "M7 18h10a4 4 0 0 0 .5-7.97A5.5 5.5 0 0 0 6.6 9.1 4.5 4.5 0 0 0 7 18Z",
  desktopcomputer: "M3.5 5h17v11h-17zM9 20h6M12 16v4",
  link: "M10 14a4 4 0 0 0 5.66 0l3-3a4 4 0 0 0-5.66-5.66l-1 1M14 10a4 4 0 0 0-5.66 0l-3 3a4 4 0 0 0 5.66 5.66l1-1",
  gearshape:
    "M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6ZM19 12l2-1-1-3-2 .3-1.3-1.3.3-2-3-1-1 2h-2l-1-2-3 1 .3 2L5 7.3 3 7l-1 3 2 1v2l-2 1 1 3 2-.3 1.3 1.3-.3 2 3 1 1-2h2l1 2 3-1-.3-2 1.3-1.3 2 .3 1-3-2-1Z",
  "play.rectangle": "M3.5 5h17v14h-17zM10 9.5v5l4.5-2.5z",
  "pip.enter": "M3 5h18v14H3zM12 12h7v5h-7z",
  "pip.exit": "M3 5h18v14H3zM5 7h7v5H5z",
  "arrow.down.circle": "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18ZM12 7.5v8M8.5 12.5 12 16l3.5-3.5",
  "arrow.down.circle.fill": "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18ZM12 7.5v8M8.5 12.5 12 16l3.5-3.5",
  trash: "M4 7h16M9 7V4.5h6V7M6.5 7l1 12.5h9l1-12.5",
  power: "M12 3.5v8M7.2 6.2a7.5 7.5 0 1 0 9.6 0",
  cpu: "M7 7h10v10H7zM10 3v4M14 3v4M10 17v4M14 17v4M3 10h4M3 14h4M17 10h4M17 14h4",
  memorychip: "M3 8h18v8H3zM7 16v3M11 16v3M15 16v3M7 11h2M11 11h2M15 11h2",
  checkmark: "m5 12.5 4.5 4.5L19 7.5",
  "exclamationmark.triangle": "M12 4.5 21 19.5H3zM12 10v4.5M12 17v.01",
  "doc.on.doc": "M8 8h11v12H8zM5 16V4h11",
  xmark: "M6 6l12 12M18 6 6 18",
  "arrow.clockwise": "M19 12a7 7 0 1 1-2.05-4.95M19 4v4h-4",
  "person.crop.circle":
    "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18Zm0-9a3 3 0 1 0 0-6 3 3 0 0 0 0 6Zm-6 6.5c1.5-2 3.6-3 6-3s4.5 1 6 3",
  "square.grid.2x2": "M4 4h7v7H4zM13 4h7v7h-7zM4 13h7v7H4zM13 13h7v7h-7z",
  shippingbox: "M12 3 20 7.5v9L12 21l-8-4.5v-9zM4 7.5l8 4.5 8-4.5M12 12v9",
  sparkles: "M12 3v6M9 6h6M18 12v5M15.5 14.5h5M7 14v6M4 17h6",
  "lock.shield": "M12 3 19.5 6v5.5c0 4.6-3.2 8.2-7.5 9.5-4.3-1.3-7.5-4.9-7.5-9.5V6zM9.5 11.5v-1a2.5 2.5 0 0 1 5 0v1M8.75 11.5h6.5v4.5h-6.5z",
};

export function Sym({ name, className, size = 16 }: { name: string; className?: string; size?: number }) {
  const d = PATHS[name] ?? PATHS.plus!;
  return (
    <SfIcon
      name={name}
      size={size}
      className={className}
      fallback={
        <svg
          className={className}
          viewBox="0 0 24 24"
          width={size}
          height={size}
          fill="none"
          stroke="currentColor"
          strokeWidth="1.7"
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden="true"
        >
          <path d={d} />
        </svg>
      }
    />
  );
}
