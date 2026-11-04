// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useState, type ReactNode } from "react";

import { sfSymbol } from "../native/sfSymbol";

/**
 * Render a macOS SF Symbol when available (real system icon, themed via a CSS
 * mask over `currentColor`), falling back to `fallback` (an inline SVG glyph)
 * off macOS or if the symbol can't be rendered. `className` is applied to both
 * so sizing/colour stay consistent either way.
 */
export function SfIcon({
  name,
  className,
  size = 14,
  fallback,
}: {
  name: string;
  className?: string;
  size?: number;
  fallback: ReactNode;
}) {
  const [url, setUrl] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void sfSymbol(name, size).then((resolved) => {
      if (!cancelled) setUrl(resolved);
    });
    return () => {
      cancelled = true;
    };
  }, [name, size]);

  if (!url) return <>{fallback}</>;
  return (
    <span
      className={className ? `sf-icon ${className}` : "sf-icon"}
      style={{ maskImage: `url("${url}")`, WebkitMaskImage: `url("${url}")` }}
      aria-hidden="true"
    />
  );
}
