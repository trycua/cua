// Review scenes on a 1280 x 800 point stage (a typical laptop screen in
// logical points). Rects are top-left based. Every candidate runs the same
// waypoints, so tiles are directly comparable.
//
// Waypoint actions: click | dblclick | hover | drag (press at the previous
// point, release here) | scroll (wheel at this point) | type.

export const STAGE = { w: 1280, h: 800 };

export const scenes = {
  route: {
    id: 'route',
    name: 'Click route',
    blurb: 'Mixed distances and target sizes: big button, small checkbox, link, menu item.',
    start: { x: 150, y: 660 },
    waypoints: [
      { id: 'run', label: 'Run', kind: 'button', x: 940, y: 92, w: 220, h: 80, action: 'click' },
      { id: 'agree', label: '', kind: 'checkbox', x: 196, y: 262, w: 36, h: 36, action: 'click' },
      {
        id: 'docs',
        label: 'Docs',
        kind: 'link',
        x: 880,
        y: 636,
        w: 128,
        h: 40,
        action: 'click',
        fails: true,
      },
      {
        id: 'settings',
        label: 'Settings',
        kind: 'menu',
        x: 470,
        y: 420,
        w: 280,
        h: 56,
        action: 'click',
      },
    ],
  },
  precision: {
    id: 'precision',
    name: 'Tiny targets',
    blurb: '12 pt targets: tests precision approach, overshoot and corrections.',
    start: { x: 640, y: 400 },
    waypoints: [
      { id: 't1', kind: 'tiny', x: 1060, y: 150, w: 12, h: 12, action: 'click' },
      { id: 't2', kind: 'tiny', x: 260, y: 190, w: 12, h: 12, action: 'click' },
      { id: 't3', kind: 'tiny', x: 680, y: 610, w: 12, h: 12, action: 'click' },
      { id: 't4', kind: 'tiny', x: 1120, y: 660, w: 12, h: 12, action: 'click' },
      { id: 't5', kind: 'tiny', x: 180, y: 520, w: 12, h: 12, action: 'click' },
    ],
  },
  drag: {
    id: 'drag',
    name: 'Drag and drop',
    blurb: 'Pick up a card, carry it into a drop zone, twice.',
    start: { x: 120, y: 700 },
    waypoints: [
      {
        id: 'cardA',
        label: 'Card A',
        kind: 'card',
        x: 150,
        y: 220,
        w: 220,
        h: 130,
        action: 'hover',
      },
      {
        id: 'zoneA',
        label: 'Drop',
        kind: 'zone',
        x: 860,
        y: 160,
        w: 320,
        h: 220,
        action: 'drag',
        carries: 'cardA',
      },
      {
        id: 'cardB',
        label: 'Card B',
        kind: 'card',
        x: 150,
        y: 480,
        w: 220,
        h: 130,
        action: 'hover',
      },
      {
        id: 'zoneB',
        label: 'Drop',
        kind: 'zone',
        x: 860,
        y: 460,
        w: 320,
        h: 220,
        action: 'drag',
        carries: 'cardB',
      },
    ],
  },
  sweep: {
    id: 'sweep',
    name: 'Multi-target sweep',
    blurb: 'Six checkboxes then submit: short hops where flow matters more than speed.',
    start: { x: 120, y: 120 },
    waypoints: [
      { id: 'c1', kind: 'checkbox', x: 240, y: 170, w: 36, h: 36, action: 'click' },
      { id: 'c2', kind: 'checkbox', x: 240, y: 310, w: 36, h: 36, action: 'click' },
      { id: 'c3', kind: 'checkbox', x: 240, y: 450, w: 36, h: 36, action: 'click' },
      { id: 'c4', kind: 'checkbox', x: 640, y: 170, w: 36, h: 36, action: 'click' },
      { id: 'c5', kind: 'checkbox', x: 640, y: 310, w: 36, h: 36, action: 'click' },
      { id: 'c6', kind: 'checkbox', x: 640, y: 450, w: 36, h: 36, action: 'click' },
      {
        id: 'submit',
        label: 'Submit',
        kind: 'button',
        x: 960,
        y: 640,
        w: 220,
        h: 72,
        action: 'click',
      },
    ],
  },
  scroll: {
    id: 'scroll',
    name: 'Scroll then click',
    blurb: 'Park over a list, scroll it, click the revealed row, then a button.',
    start: { x: 200, y: 640 },
    list: { x: 720, y: 80, w: 440, h: 640, rows: 18, rowH: 72 },
    waypoints: [
      { id: 'list', kind: 'list', x: 760, y: 220, w: 360, h: 80, action: 'scroll', scroll: 360 },
      { id: 'row', label: 'Row 9', kind: 'row', x: 760, y: 500, w: 360, h: 64, action: 'click' },
      { id: 'save', label: 'Save', kind: 'button', x: 160, y: 300, w: 200, h: 68, action: 'click' },
    ],
  },
  type: {
    id: 'type',
    name: 'Click, type, send',
    blurb: 'Focus a field, type while the cursor stays out of the way, send.',
    start: { x: 1100, y: 680 },
    waypoints: [
      {
        id: 'field',
        label: 'Message',
        kind: 'input',
        x: 300,
        y: 160,
        w: 560,
        h: 68,
        action: 'type',
        text: 'ship the cursor update',
      },
      { id: 'send', label: 'Send', kind: 'button', x: 960, y: 600, w: 200, h: 72, action: 'click' },
    ],
  },
  menu: {
    id: 'menu',
    name: 'Cascading menu',
    blurb: 'Open a menu, hover a submenu, steer into the nested item without crossing siblings.',
    start: { x: 900, y: 600 },
    waypoints: [
      { id: 'file', label: 'File', kind: 'menubar', x: 80, y: 40, w: 100, h: 48, action: 'click' },
      {
        id: 'export',
        label: 'Export',
        kind: 'menu',
        x: 80,
        y: 220,
        w: 300,
        h: 52,
        action: 'hover',
      },
      { id: 'svg', label: 'SVG', kind: 'menu', x: 392, y: 324, w: 260, h: 52, action: 'click' },
    ],
  },
  long: {
    id: 'long',
    name: 'Long throws',
    blurb: 'Corner to corner across the whole screen.',
    start: { x: 60, y: 740 },
    waypoints: [
      { id: 'tr', label: 'A', kind: 'button', x: 1160, y: 40, w: 80, h: 48, action: 'click' },
      { id: 'tl', label: 'B', kind: 'button', x: 48, y: 48, w: 80, h: 48, action: 'click' },
      { id: 'br', label: 'C', kind: 'button', x: 1160, y: 700, w: 80, h: 48, action: 'click' },
      { id: 'mid', label: 'D', kind: 'button', x: 600, y: 376, w: 80, h: 48, action: 'click' },
    ],
  },
};

export const sceneList = Object.values(scenes);

export const center = (r) => ({ x: r.x + r.w / 2, y: r.y + r.h / 2 });
export const inside = (p, r, pad = 0) =>
  p.x >= r.x - pad && p.x <= r.x + r.w + pad && p.y >= r.y - pad && p.y <= r.y + r.h + pad;
