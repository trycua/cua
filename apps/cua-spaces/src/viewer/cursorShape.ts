// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Guest cursor shapes → CSS cursors (shared by stream viewers and presence
 * overlays). Kept from the v1 client unchanged.
 */

/** Map one protocol `CursorShape` onto a CSS `cursor` keyword.
 *
 * Keywords rather than bitmaps: the browser draws its own native cursor art, so
 * an I-beam stays crisp at any zoom, follows the viewer's OS theme and
 * accessibility cursor size, and costs nothing to transfer. Only a guest cursor
 * with no portable equivalent arrives as pixels (`custom`), and that one is
 * drawn from its own PNG with its own hot spot.
 */
export function cursorShapeToCss(shape: unknown): string | null {
  if (shape == null || typeof shape !== "object") return null;
  const kind = (shape as { kind?: unknown }).kind;
  switch (kind) {
    case "default":
      return "default";
    case "text":
      return "text";
    case "vertical_text":
      return "vertical-text";
    case "pointer":
      return "pointer";
    case "grab":
      return "grab";
    case "grabbing":
      return "grabbing";
    case "crosshair":
      return "crosshair";
    case "wait":
      return "wait";
    case "not_allowed":
      return "not-allowed";
    case "resize": {
      const axis = (shape as { axis?: unknown }).axis;
      switch (axis) {
        case "north_south":
          return "ns-resize";
        case "east_west":
          return "ew-resize";
        case "north_east_south_west":
          return "nesw-resize";
        case "north_west_south_east":
          return "nwse-resize";
        case "all":
          return "move";
        case "column":
          return "col-resize";
        case "row":
          return "row-resize";
        default:
          return "default";
      }
    }
    case "custom": {
      const png = (shape as { png?: unknown }).png;
      const bytes = Array.isArray(png) ? (png as number[]) : null;
      if (!bytes || bytes.length === 0) return null;
      let binary = "";
      for (const byte of bytes) binary += String.fromCharCode(byte);
      const hotX = Number((shape as { hotspot_x?: unknown }).hotspot_x) || 0;
      const hotY = Number((shape as { hotspot_y?: unknown }).hotspot_y) || 0;
      return `url(data:image/png;base64,${btoa(binary)}) ${Math.round(hotX)} ${Math.round(
        hotY,
      )}, default`;
    }
    // `unknown` means the host cannot report a shape. It is NOT `default`:
    // returning null holds whatever shape the cursor already had rather than
    // snapping it to an arrow. `unsupported` is a shape added after this build
    // and gets the same treatment.
    case "unknown":
    case "unsupported":
    default:
      return null;
  }
}

/** Apply a reported shape to one participant's cursor overlay element.
 *
 * The overlay keeps its identity arrow (colored, name-tagged) as the marker of
 * WHO the cursor belongs to, and carries the guest's shape as a CSS `cursor` so
 * hovering the element reads correctly and the shape is inspectable. A null
 * shape leaves the previous one untouched.
 */
export function applyCursorShape(el: HTMLElement, shape: unknown): void {
  const css = cursorShapeToCss(shape);
  if (css == null) return;
  el.style.cursor = css;
  el.dataset.cursorShape =
    typeof shape === "object" && shape != null
      ? String((shape as { kind?: unknown }).kind ?? "")
      : "";
}
