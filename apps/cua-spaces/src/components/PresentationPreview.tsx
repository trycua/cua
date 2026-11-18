// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useRef, type CSSProperties } from "react";

import trayIcon from "../../src-tauri/icons/tray-template.svg";
import {
  notchMotion,
  presentationPreview,
  previewFrame,
  previewStill,
  type MenuPreview,
  type NotchPreview,
  type PresentationPreview as Scene,
  type PreviewFrame,
} from "../model/presentationPreview";
import { ListGlyph } from "./NavGlyphs";
import { OsIconMark } from "./OsIcon";
import { useLoopTime, useReducedMotion } from "./previewLoop";
import { SearchGlyph } from "./SearchGlyph";
import { GearGlyph } from "./SettingsPanel";

/**
 * A "Where should Cua Spaces show up?" card's animated miniature: the notch
 * opening into the Space tiles, or the menu bar icon's menu dropping down.
 * The scene and every frame are the app core's (`onboarding.preview`,
 * `onboarding.previewFrame`); the SwiftUI app's `PresentationPreview` draws
 * the same, so both play the same beats. This only draws them.
 *
 * It loops while it is on screen, the page is visible and the window has
 * focus, and stops otherwise. With reduced motion it is the core's still:
 * the expanded state.
 */
export function PresentationPreview({ menuBar, fixedMs }: { menuBar: boolean; fixedMs?: number }) {
  const scene = presentationPreview(menuBar);
  const reduced = useReducedMotion();
  const ref = useRef<HTMLDivElement>(null);
  const animate = fixedMs === undefined && !reduced;
  const t = useLoopTime(ref, animate, scene.loopMs);
  const frame =
    fixedMs !== undefined ? previewFrame(menuBar, fixedMs) : reduced ? previewStill(menuBar) : previewFrame(menuBar, t);
  // The desktop and the menu bar span the card; the stage sits in them,
  // centred under the notch or at the menu bar's trailing end.
  return (
    <div
      ref={ref}
      className="obp-picture"
      data-preview={menuBar ? "menu-bar" : "notch"}
      data-still={reduced && fixedMs === undefined ? "true" : undefined}
      style={{ height: scene.height }}
      aria-hidden="true"
    >
      <div className="obp-menubar" style={{ height: scene.menuBarHeight }} />
      <div className="obp-stage" style={{ width: scene.width, height: scene.height }}>
        {scene.notch ? <NotchMiniature scene={scene} notch={scene.notch} frame={frame} /> : null}
        {scene.menu ? <MenuMiniature scene={scene} menu={scene.menu} frame={frame} /> : null}
        <svg
          className="obp-pointer"
          width="12"
          height="16"
          style={{
            transform: `translate(${frame.pointer.x}px, ${frame.pointer.y}px) scale(${frame.pressed ? 0.85 : 1})`,
          }}
        >
          <polygon points={scene.pointer.map((p) => `${p.x},${p.y}`).join(" ")} />
        </svg>
      </div>
    </div>
  );
}

/** The notch's outline (the SwiftUI app's `NotchShape`): concave ears at the
 * top corners, rounded bottom corners. */
function notchPath(w: number, h: number, top: number, bottom: number): string {
  const t = Math.max(0, Math.min(top, w / 4));
  const b = Math.max(0, Math.min(bottom, h / 2, (w - 2 * t) / 2));
  return [
    `M0 0`,
    `Q${t} 0 ${t} ${t}`,
    `L${t} ${h - b}`,
    `Q${t} ${h} ${t + b} ${h}`,
    `L${w - t - b} ${h}`,
    `Q${w - t} ${h} ${w - t} ${h - b}`,
    `L${w - t} ${t}`,
    `Q${w - t} 0 ${w} 0`,
    `Z`,
  ].join(" ");
}

/** The tab's outline (`NotchTabShape`): square on the left, an ear on the
 * top right, a rounded bottom-right corner. */
function tabPath(w: number, h: number, ear: number, corner: number): string {
  return [
    `M0 0`,
    `L${w} 0`,
    `Q${w - ear} 0 ${w - ear} ${ear}`,
    `L${w - ear} ${h - corner}`,
    `Q${w - ear} ${h} ${w - ear - corner} ${h}`,
    `L0 ${h}`,
    `Z`,
  ].join(" ");
}

function NotchMiniature({ scene, notch: n, frame }: { scene: Scene; notch: NotchPreview; frame: PreviewFrame }) {
  const motion = notchMotion();
  const o = frame.open;
  const lerp = (a: number, b: number) => a + (b - a) * o;
  const w = lerp(n.closed.width, n.open.width);
  const h = lerp(n.closed.height, n.open.height);
  const top = lerp(n.closedRadii.top, n.openRadii.top);
  const bottom = lerp(n.closedRadii.bottom, n.openRadii.bottom);
  const x = (scene.width - w) / 2;
  const grow = 1 + (motion.hoverScale - 1) * frame.hover;
  const c = frame.content;
  const contentScale = motion.contentScale + (1 - motion.contentScale) * c;
  const tuck = 20 * n.scale;
  const ear = n.closedRadii.top;
  const tabW = n.tab.width + tuck;
  const shadow = 0.45 * Math.min(Math.max(o, 0), 1);
  const path = notchPath(w, h, top, bottom);
  return (
    <>
      <div className="obp-layer" style={{ transform: `scaleX(${grow})` }}>
        <div
          className="obp-tab"
          style={{
            left: n.tab.x - tuck - (1 - frame.tab) * n.tab.width,
            width: tabW,
            height: n.tab.height,
            opacity: frame.tab,
          }}
        >
          <svg width={tabW} height={n.tab.height}>
            <path d={tabPath(tabW, n.tab.height, ear, 10 * n.scale)} />
          </svg>
          <span
            className="obp-tab-label"
            style={{
              transform: `translateX(${(tuck - ear - 6 * n.scale) / 2}px) scale(${n.scale})`,
            }}
          >
            <span className="obp-tab-count">{n.view.tab.count}</span>
            <span className="obp-tab-word">{n.view.tab.word}</span>
          </span>
        </div>
        <svg
          className="obp-notch"
          width={w}
          height={h}
          style={{
            left: x,
            filter: shadow > 0 ? `drop-shadow(0 ${6 * n.scale}px ${7 * n.scale}px rgba(0,0,0,${shadow}))` : undefined,
          }}
        >
          <path d={path} />
        </svg>
      </div>
      <div className="obp-notch-content" style={{ left: x, width: w, height: h, clipPath: `path("${path}")` }}>
        <div
          className="obp-notch-scaled"
          style={{
            left: (w - n.open.width) / 2,
            width: n.open.width,
            height: n.open.height,
            opacity: c,
            transform: `scale(${contentScale})`,
          }}
        >
          <NotchMiniContent notch={n} />
        </div>
      </div>
    </>
  );
}

/** The open panel's content at real size, scaled into the miniature. */
function NotchMiniContent({ notch: n }: { notch: NotchPreview }) {
  const zone = Math.max(0, (n.contentWidth - n.notch.width / n.scale) / 2 - n.side - 12);
  const style: CSSProperties = {
    width: n.contentWidth,
    height: n.contentHeight,
    padding: `0 ${n.side}px`,
    transform: `scale(${n.scale})`,
  };
  const h = n.view.header;
  return (
    <div className="obp-panel" style={style}>
      {h ? (
        <div className="obp-header" style={{ height: n.notchHeight }}>
          <span className="obp-search" style={{ width: zone }}>
            <SearchGlyph className="obp-search-glyph" />
            <span>{h.placeholder}</span>
          </span>
          <span className="obp-buttons" style={{ width: zone }}>
            {h.buttons.map((b) => (
              <span key={b.symbol} className="obp-button">
                {b.symbol === "gearshape" ? (
                  <GearGlyph className="obp-button-glyph" />
                ) : (
                  <ListGlyph className="obp-button-glyph" />
                )}
              </span>
            ))}
          </span>
        </div>
      ) : null}
      <div className="obp-tiles">
        {n.view.tiles.map((tile) => (
          <div key={tile.id} className="obp-tile" data-dim={tile.dim ? "true" : undefined}>
            <span className="obp-tile-frame">
              <OsIconMark id={tile.symbol} size={26} />
            </span>
            <span className="obp-tile-caption">
              <OsIconMark id={tile.symbol} />
              <span className="obp-tile-dot" data-status={tile.status} />
              <span className="obp-tile-name">{tile.name}</span>
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}

function MenuMiniature({ scene, menu: m, frame }: { scene: Scene; menu: MenuPreview; frame: PreviewFrame }) {
  return (
    <>
      {frame.active ? (
        <span
          className="obp-status-highlight"
          style={{
            left: m.highlight.x,
            top: m.highlight.y,
            width: m.highlight.width,
            height: m.highlight.height,
          }}
        />
      ) : null}
      <span
        className="obp-status-icon"
        style={{
          left: m.icon.x,
          top: m.icon.y,
          width: m.icon.width,
          height: m.icon.height,
          maskImage: `url("${trayIcon}")`,
          WebkitMaskImage: `url("${trayIcon}")`,
        }}
      />
      <span className="obp-clock" style={{ right: scene.width - m.clockRight, height: scene.menuBarHeight }}>
        {m.clock}
      </span>
      {frame.open > 0 ? (
        <div
          className="obp-menu"
          role="presentation"
          style={{
            left: m.menu.x,
            top: m.menu.y,
            width: m.menu.width,
            height: m.menu.height,
            borderRadius: m.radius,
            opacity: frame.open,
            fontSize: m.fontSize,
          }}
        >
          {m.rows.map((row, i) =>
            row.item.id === "separator" ? (
              <span
                key={i}
                className="obp-menu-separator"
                style={{
                  top: row.frame.y - m.menu.y,
                  height: row.frame.height,
                  left: m.inset,
                  right: m.inset,
                }}
              />
            ) : (
              <span
                key={i}
                className="obp-menu-row"
                data-highlighted={frame.highlighted === i ? "true" : undefined}
                data-disabled={row.item.enabled ? undefined : "true"}
                style={{
                  top: row.frame.y - m.menu.y,
                  height: row.frame.height,
                  padding: `0 ${m.inset}px`,
                  ["--obp-inset" as string]: `${m.inset / 2}px`,
                }}
              >
                <span className="obp-menu-label">{row.item.label}</span>
                {row.item.shortcut ? <span className="obp-menu-shortcut">{row.item.shortcut}</span> : null}
              </span>
            ),
          )}
        </div>
      ) : null}
    </>
  );
}
