// Canvas renderer for the motion lab: draws a scene, its UI state at time t
// (derived from plan events), effects, and the in-scene Cua agent cursor.
// This never touches the real system pointer.

import { STAGE, center, inside } from './motion/scenes.js';
import { sampleAt } from './motion/plan.js';

// Cua Driver cursor body (cursor-overlay/assets/build_default_theme.py
// CURSOR_PATH), hotspot at (55, 30). Fill BLUE, 5 pt white stroke, blue glow.
const CURSOR_D =
  'M55 30 C48 28 42 33 43 41 C43 41 64 98 64 98 C67 106 73 106 77 99 C77 99 86 79 86 79 C88 75 91 72 95 70 C95 70 108 63 108 63 C115 59 114 53 107 50 C107 50 55 30 55 30 Z';
const HOT = { x: 55, y: 30 };
const CURSOR_SCALE = 0.5; // art units -> stage points (about 38 pt tall)
const BLUE = '#5ec0e8';
const BRAND = '#9fd7ff';

let cursorPath = null;
const getCursorPath = () => (cursorPath ??= new Path2D(CURSOR_D));

const easeOut = (t) => 1 - (1 - Math.min(1, Math.max(0, t))) ** 3;

function roundRect(g, x, y, w, h, r) {
  g.beginPath();
  g.roundRect(x, y, w, h, r);
}

// ---------------------------------------------------------------------------
// Scene state from events
// ---------------------------------------------------------------------------

function deriveState(plan, scene, t) {
  const st = {
    clicked: new Map(),
    pressedAt: null,
    snapped: null,
    failed: new Map(),
    scroll: 0,
    typing: null,
    think: null,
    menuOpen: false,
    submenuOpen: false,
    ghost: [],
    ripples: [],
  };
  for (const e of plan.events) {
    if (e.t > t) break;
    const age = t - e.t;
    switch (e.type) {
      case 'click':
        st.clicked.set(e.target, (st.clicked.get(e.target) ?? 0) + 1);
        st.ripples.push({ x: e.x, y: e.y, age, ms: 520, kind: 'click' });
        if (e.target === 'file') st.menuOpen = true;
        break;
      case 'press':
        st.pressedAt = e;
        break;
      case 'release':
        st.pressedAt = null;
        break;
      case 'snap':
        st.snapped = { target: e.target, age };
        break;
      case 'fail':
        st.failed.set(e.target, age);
        break;
      case 'scroll':
        st.scroll += e.dy * easeOut(age / 140);
        break;
      case 'type-start':
        st.typing = { target: e.target, text: e.text, start: e.t, ms: e.ms, done: false };
        break;
      case 'type-end':
        if (st.typing) st.typing.done = true;
        break;
      case 'think':
      case 'consider':
      case 'read':
        st.think = age < e.ms ? { type: e.type, age, ms: e.ms } : null;
        break;
      case 'ghost':
        if (age < e.ms) st.ghost.push({ ...e, age });
        break;
      case 'ripple':
        st.ripples.push({ x: e.x, y: e.y, age, ms: e.ms, kind: 'teleport' });
        break;
      case 'hover':
        if (e.target === 'export') st.submenuOpen = true;
        break;
    }
  }
  if (st.typing)
    st.typed = st.typing.done
      ? st.typing.text
      : st.typing.text.slice(
          0,
          Math.floor(((t - st.typing.start) / st.typing.ms) * st.typing.text.length)
        );
  return st;
}

// Position of a dragged card at time t.
function cardPos(plan, card, t) {
  const press = plan.events.find((e) => e.type === 'press' && e.target === card.id);
  if (!press || t < press.t) return { x: card.x, y: card.y };
  const release = plan.events.find(
    (e) => e.type === 'release' && e.target === card.id && e.t >= press.t
  );
  const at = (tt) => {
    const q = sampleAt(plan.points, tt);
    return { x: q.cx ?? q.x, y: q.cy ?? q.y };
  };
  const p0 = at(press.t);
  const off = { x: card.x - p0.x, y: card.y - p0.y };
  const p = at(release && t > release.t ? release.t : t);
  return { x: p.x + off.x, y: p.y + off.y, lifted: !release || t <= release.t };
}

// ---------------------------------------------------------------------------
// Scene drawing
// ---------------------------------------------------------------------------

function drawBackdrop(g) {
  g.fillStyle = '#0b0d10';
  g.fillRect(0, 0, STAGE.w, STAGE.h);
  g.fillStyle = 'rgba(255,255,255,0.045)';
  for (let x = 40; x < STAGE.w; x += 40)
    for (let y = 40; y < STAGE.h; y += 40) g.fillRect(x - 1, y - 1, 2, 2);
}

function label(
  g,
  text,
  x,
  y,
  { size = 26, color = 'rgba(236,242,250,0.92)', align = 'center', weight = 600 } = {}
) {
  g.font = `${weight} ${size}px Urbanist, Inter, system-ui, sans-serif`;
  g.fillStyle = color;
  g.textAlign = align;
  g.textBaseline = 'middle';
  g.fillText(text, x, y);
}

function drawTarget(g, wp, ui) {
  const { hover, clicked, magnet, failed, pressed } = ui;
  const glow = magnet ? Math.max(0, 1 - magnet.age / 700) : 0;
  if (glow > 0) {
    g.save();
    g.shadowColor = BRAND;
    g.shadowBlur = 40 * glow;
    g.strokeStyle = `rgba(159,215,255,${0.9 * glow})`;
    g.lineWidth = 4;
    roundRect(g, wp.x - 6, wp.y - 6, wp.w + 12, wp.h + 12, 16);
    g.stroke();
    g.restore();
  }
  const failT = failed !== undefined ? Math.max(0, 1 - failed / 700) : 0;
  switch (wp.kind) {
    case 'button':
    case 'menubar': {
      const base =
        wp.kind === 'menubar'
          ? hover || clicked
            ? '#26303d'
            : 'rgba(255,255,255,0.0)'
          : clicked
            ? '#3a7fa3'
            : hover
              ? '#2b3a4a'
              : '#1c242e';
      g.fillStyle = base;
      roundRect(g, wp.x, wp.y, wp.w, wp.h, Math.min(16, wp.h / 2.6));
      g.fill();
      g.strokeStyle = hover ? 'rgba(159,215,255,0.55)' : 'rgba(255,255,255,0.10)';
      g.lineWidth = 2;
      g.stroke();
      if (wp.label)
        label(g, wp.label, wp.x + wp.w / 2, wp.y + wp.h / 2 + (pressed ? 1 : 0), {
          size: Math.min(30, wp.h * 0.42),
        });
      break;
    }
    case 'checkbox': {
      g.fillStyle = clicked ? BLUE : hover ? '#24303c' : '#151b22';
      roundRect(g, wp.x, wp.y, wp.w, wp.h, 8);
      g.fill();
      g.strokeStyle = clicked ? BLUE : 'rgba(255,255,255,0.35)';
      g.lineWidth = 2.5;
      g.stroke();
      if (clicked) {
        g.strokeStyle = '#06131b';
        g.lineWidth = 4;
        g.beginPath();
        g.moveTo(wp.x + wp.w * 0.24, wp.y + wp.h * 0.52);
        g.lineTo(wp.x + wp.w * 0.43, wp.y + wp.h * 0.7);
        g.lineTo(wp.x + wp.w * 0.77, wp.y + wp.h * 0.3);
        g.stroke();
      }
      g.fillStyle = 'rgba(255,255,255,0.10)';
      roundRect(g, wp.x + wp.w + 22, wp.y + wp.h / 2 - 7, 180, 14, 7);
      g.fill();
      break;
    }
    case 'link': {
      const col =
        failT > 0
          ? `rgba(255,${120 + 100 * (1 - failT)},${120 + 100 * (1 - failT)},1)`
          : hover
            ? BRAND
            : '#7fb6e0';
      label(g, wp.label, wp.x + wp.w / 2, wp.y + wp.h / 2, { size: 28, color: col });
      g.fillStyle = col;
      g.fillRect(wp.x + 8, wp.y + wp.h - 4, wp.w - 16, 2.5);
      break;
    }
    case 'menu':
    case 'row': {
      g.fillStyle = clicked
        ? 'rgba(94,192,232,0.28)'
        : hover
          ? 'rgba(255,255,255,0.10)'
          : 'rgba(255,255,255,0.03)';
      roundRect(g, wp.x, wp.y, wp.w, wp.h, 10);
      g.fill();
      if (wp.label) label(g, wp.label, wp.x + 24, wp.y + wp.h / 2, { size: 26, align: 'left' });
      break;
    }
    case 'tiny': {
      const c = center(wp);
      g.strokeStyle = clicked ? BLUE : 'rgba(255,255,255,0.28)';
      g.lineWidth = 2;
      g.beginPath();
      g.arc(c.x, c.y, 22, 0, Math.PI * 2);
      g.stroke();
      g.fillStyle = clicked ? BLUE : hover ? '#ffffff' : 'rgba(255,255,255,0.75)';
      g.beginPath();
      g.arc(c.x, c.y, wp.w / 2, 0, Math.PI * 2);
      g.fill();
      break;
    }
    case 'zone': {
      g.save();
      g.setLineDash([14, 10]);
      g.strokeStyle = hover ? 'rgba(159,215,255,0.8)' : 'rgba(255,255,255,0.22)';
      g.lineWidth = 3;
      roundRect(g, wp.x, wp.y, wp.w, wp.h, 22);
      g.stroke();
      g.restore();
      label(g, wp.label ?? 'Drop', wp.x + wp.w / 2, wp.y + wp.h / 2, {
        size: 26,
        color: 'rgba(255,255,255,0.3)',
      });
      break;
    }
    case 'input': {
      g.fillStyle = '#12171d';
      roundRect(g, wp.x, wp.y, wp.w, wp.h, 14);
      g.fill();
      g.strokeStyle = clicked ? BLUE : hover ? 'rgba(159,215,255,0.5)' : 'rgba(255,255,255,0.16)';
      g.lineWidth = 2.5;
      g.stroke();
      break;
    }
    default:
      break;
  }
}

function drawScene(g, plan, scene, t, cursorPt) {
  const st = deriveState(plan, scene, t);
  drawBackdrop(g);

  // Window chrome.
  g.fillStyle = 'rgba(255,255,255,0.025)';
  roundRect(g, 20, 20, STAGE.w - 40, STAGE.h - 40, 26);
  g.fill();

  if (scene.list) {
    const L = scene.list;
    g.save();
    g.fillStyle = '#10151b';
    roundRect(g, L.x, L.y, L.w, L.h, 20);
    g.fill();
    g.clip();
    for (let i = 0; i < L.rows; i++) {
      const y = L.y + 24 + i * L.rowH - st.scroll;
      g.fillStyle = 'rgba(255,255,255,0.06)';
      roundRect(g, L.x + 30, y + 18, L.w * (0.45 + 0.35 * (((i * 37) % 10) / 10)), 16, 8);
      g.fill();
    }
    g.restore();
  }

  if (scene.id === 'menu') {
    if (st.menuOpen) {
      g.fillStyle = '#161c23';
      roundRect(g, 70, 100, 320, 310, 16);
      g.fill();
      ['New', 'Open', '', 'Print'].forEach(
        (s, i) =>
          s &&
          label(g, s, 104, 140 + i * 66, {
            size: 26,
            align: 'left',
            color: 'rgba(236,242,250,0.6)',
          })
      );
    }
    if (st.submenuOpen) {
      g.fillStyle = '#1b222b';
      roundRect(g, 384, 212, 280, 230, 16);
      g.fill();
      label(g, 'PNG', 416, 246, { size: 26, align: 'left', color: 'rgba(236,242,250,0.6)' });
      label(g, 'PDF', 416, 402, { size: 26, align: 'left', color: 'rgba(236,242,250,0.6)' });
    }
  }
  if (scene.id === 'route') {
    label(g, 'I agree to the terms', 254, 280, {
      size: 24,
      align: 'left',
      color: 'rgba(236,242,250,0.5)',
      weight: 500,
    });
  }

  for (const wp of scene.waypoints) {
    if (scene.id === 'menu' && wp.id === 'export' && !st.menuOpen) continue;
    if (scene.id === 'menu' && wp.id === 'svg' && !st.submenuOpen) continue;
    if (wp.kind === 'card') continue;
    if (wp.kind === 'list') continue;
    if (
      wp.kind === 'row' &&
      st.scroll < (scene.waypoints.find((w) => w.kind === 'list')?.scroll ?? 0) * 0.9
    )
      continue;
    const hover = cursorPt && inside(cursorPt, wp);
    drawTarget(g, wp, {
      hover,
      clicked: st.clicked.has(wp.id),
      magnet: st.snapped?.target === wp.id ? st.snapped : null,
      failed: st.failed.get(wp.id),
      pressed: hover && st.pressedAt,
    });
  }

  if (st.typing || st.typed) {
    const wp = scene.waypoints.find((w) => w.kind === 'input');
    if (wp && st.typed !== undefined) {
      label(
        g,
        st.typed + (Math.floor(t / 400) % 2 && !st.typing?.done ? '|' : ''),
        wp.x + 26,
        wp.y + wp.h / 2,
        { size: 28, align: 'left', weight: 500 }
      );
    }
  }
  const field = scene.waypoints.find((w) => w.kind === 'input');
  if (field && !st.typed)
    label(g, field.label, field.x + 26, field.y + field.h / 2, {
      size: 28,
      align: 'left',
      color: 'rgba(236,242,250,0.3)',
      weight: 500,
    });

  // Cards (drawn above zones).
  for (const card of scene.waypoints.filter((w) => w.kind === 'card')) {
    const p = cardPos(plan, card, t);
    g.save();
    if (p.lifted) {
      g.shadowColor = 'rgba(0,0,0,0.6)';
      g.shadowBlur = 30;
      g.shadowOffsetY = 12;
    }
    g.fillStyle = p.lifted ? '#2a3542' : '#1f2832';
    roundRect(g, p.x, p.y, card.w, card.h, 18);
    g.fill();
    g.restore();
    g.strokeStyle = 'rgba(255,255,255,0.12)';
    g.lineWidth = 2;
    roundRect(g, p.x, p.y, card.w, card.h, 18);
    g.stroke();
    label(g, card.label, p.x + card.w / 2, p.y + card.h / 2, { size: 28 });
  }
  return st;
}

// ---------------------------------------------------------------------------
// Effects and cursor
// ---------------------------------------------------------------------------

function velocityAt(points, t) {
  const a = sampleAt(points, t - 12);
  const b = sampleAt(points, t + 12);
  return { x: (b.x - a.x) / 0.024, y: (b.y - a.y) / 0.024 };
}

// Trail anchor: the back of the arrow body (55% of the way from the hotspot down the arrow's axis), not the hotspot. The trail
// is painted before the cursor, so the body covers where it starts: it flows out from behind the arrow and the tip stays clean.
// The offset follows the velocity, ramped in with the same weight as the tangent heading (full speed -> the arrow's real axis).
const TRAIL_BACK = Math.hypot(0.55 * (91 - 55), 0.55 * (79.5 - 30)); // art units, hotspot -> arrow back
function drawTrail(g, points, t, { ms = 240, opacity = 0.45 } = {}, scale = 1) {
  const steps = 26;
  const d = TRAIL_BACK * CURSOR_SCALE * scale;
  const anchor = (tt) => {
    const q = sampleAt(points, tt);
    const v = velocityAt(points, tt);
    const sp = Math.hypot(v.x, v.y);
    const w = Math.min(1, Math.max(0, (sp - 40) / 260));
    return sp > 1 ? { x: q.x - (v.x / sp) * d * w, y: q.y - (v.y / sp) * d * w } : { x: q.x, y: q.y };
  };
  const pts = [];
  let len = 0;
  for (let i = 0; i <= steps; i++) {
    const p = anchor(t - ms + (ms * i) / steps);
    if (i) len += Math.hypot(p.x - pts[i - 1].x, p.y - pts[i - 1].y);
    pts.push(p);
  }
  const fade = Math.min(1, len / 60); // a very short trail (start, landing) fades out instead of showing a stub
  let prev = pts[0];
  for (let i = 1; i <= steps; i++) {
    const q = pts[i];
    const k = i / steps;
    const dd = Math.hypot(q.x - prev.x, q.y - prev.y);
    if (dd > 0.3) {
      g.strokeStyle = `rgba(159,215,255,${opacity * k * k * fade})`;
      g.lineWidth = 2 + 12 * k;
      g.lineCap = 'round';
      g.beginPath();
      g.moveTo(prev.x, prev.y);
      g.lineTo(q.x, q.y);
      g.stroke();
    }
    prev = q;
  }
}

function drawPathGhost(g, points, t) {
  g.strokeStyle = 'rgba(159,215,255,0.16)';
  g.lineWidth = 2;
  g.beginPath();
  let started = false;
  for (const q of points) {
    if (q.t > t) break;
    if (!started) g.moveTo(q.x, q.y);
    else g.lineTo(q.x, q.y);
    started = true;
  }
  g.stroke();
}

function drawCursor(g, q, { vel, fx, pressK, st, boost = 1 }) {
  const speed = Math.hypot(vel.x, vel.y);
  // Velocity fog: a soft glow offset against velocity, growing with speed.
  if (fx.fog || fx.magnet) {
    const off = Math.min(18, speed * 0.009);
    const ux = speed > 1 ? vel.x / speed : 0;
    const uy = speed > 1 ? vel.y / speed : 0;
    const r = 34 * boost * (1 + Math.min(0.44, speed * 0.00024));
    const a = Math.min(0.5, 0.12 + speed * 0.00012) * (q.opacity ?? 1);
    const gx = q.x + 10 - ux * off;
    const gy = q.y + 16 - uy * off;
    const grad = g.createRadialGradient(gx, gy, 0, gx, gy, r);
    grad.addColorStop(0, `rgba(94,192,232,${a})`);
    grad.addColorStop(1, 'rgba(94,192,232,0)');
    g.fillStyle = grad;
    g.beginPath();
    g.arc(gx, gy, r, 0, Math.PI * 2);
    g.fill();
  }
  const draw = (alpha, dx = 0, dy = 0) => {
    g.save();
    g.globalAlpha = alpha * (q.opacity ?? 1);
    g.translate(q.x + dx, q.y + dy);
    // Stretch along velocity (volume preserving).
    if ((q.sq ?? 1) !== 1 && speed > 1) {
      const ang = Math.atan2(vel.y, vel.x);
      g.rotate(ang);
      g.scale(q.sq, 1 / q.sq);
      g.rotate(-ang);
    } else if ((q.sq ?? 1) !== 1) g.scale(1 / q.sq, q.sq);
    const s = (q.scale ?? 1) * (1 - pressK) * boost;
    g.rotate(q.heading ?? 0);
    g.scale(s * CURSOR_SCALE, s * CURSOR_SCALE);
    g.translate(-HOT.x, -HOT.y);
    const path = getCursorPath();
    g.shadowColor = 'rgba(94,192,232,0.65)';
    g.shadowBlur = 18;
    g.lineJoin = 'round';
    g.lineWidth = 7;
    g.strokeStyle = '#ffffff';
    g.stroke(path);
    g.shadowBlur = 0;
    g.fillStyle = BLUE;
    g.fill(path);
    g.restore();
  };
  if (fx.shadow) {
    const lift = Math.max(0, (q.scale ?? 1) - 1);
    g.save();
    g.globalAlpha = 0.35 * (q.opacity ?? 1);
    g.fillStyle = '#000';
    g.filter = 'blur(6px)';
    g.translate(q.x + (8 + lift * 90) * boost, q.y + (20 + lift * 140) * boost);
    g.scale(CURSOR_SCALE * boost * (1 - lift * 0.6), CURSOR_SCALE * boost * (1 - lift * 0.6));
    g.translate(-HOT.x, -HOT.y);
    g.fill(getCursorPath());
    g.restore();
  }
  if (fx.blur && speed > 600) {
    const k = Math.min(1, (speed - 600) / 3000);
    for (let i = 3; i >= 1; i--) draw(0.16 * k, (-vel.x * 0.006 * i) / 1, (-vel.y * 0.006 * i) / 1);
  }
  draw(1);
  void st;
}

function drawThink(g, q, think, t, boost = 1) {
  if (!think) return;
  const a = Math.min(1, think.age / 150, (think.ms - think.age) / 150);
  g.save();
  g.translate(q.x, q.y);
  g.scale(boost, boost);
  g.translate(-q.x, -q.y);
  g.globalAlpha = Math.max(0, a) * 0.9;
  const bx = q.x + 46;
  const by = q.y - 34;
  g.fillStyle = 'rgba(24,30,38,0.92)';
  roundRect(g, bx, by - 18, 84, 36, 18);
  g.fill();
  for (let i = 0; i < 3; i++) {
    const ph = Math.sin(t / 160 - i * 0.9);
    g.fillStyle = `rgba(159,215,255,${0.45 + 0.4 * ph})`;
    g.beginPath();
    g.arc(bx + 22 + i * 20, by - ph * 2, 5.5, 0, Math.PI * 2);
    g.fill();
  }
  g.restore();
}

const boostFor = (s) => Math.min(3, Math.max(1, 22 / (38 * s)));

// Render one frame of `plan` in `scene` at time t into a 2D context sized
// (w, h) device pixels.
export function renderFrame(g, w, h, { plan, scene, candidate, t, showPath = false }) {
  const s = Math.min(w / STAGE.w, h / STAGE.h);
  g.setTransform(1, 0, 0, 1, 0, 0);
  g.clearRect(0, 0, w, h);
  g.setTransform(s, 0, 0, s, (w - STAGE.w * s) / 2, (h - STAGE.h * s) / 2);
  const q = sampleAt(plan.points, t);
  const st = drawScene(g, plan, scene, t, q);
  const fx = candidate.fx ?? {};
  if (showPath) drawPathGhost(g, plan.points, t);
  for (const gh of st.ghost) {
    const k = 1 - gh.age / gh.ms;
    g.save();
    g.setLineDash([10, 10]);
    g.strokeStyle = `rgba(159,215,255,${0.6 * k})`;
    g.lineWidth = 3;
    const mx = (gh.from.x + gh.to.x) / 2;
    const my =
      (gh.from.y + gh.to.y) / 2 - Math.hypot(gh.to.x - gh.from.x, gh.to.y - gh.from.y) * 0.15;
    g.beginPath();
    g.moveTo(gh.from.x, gh.from.y);
    g.quadraticCurveTo(mx, my, gh.to.x, gh.to.y);
    g.stroke();
    g.restore();
  }
  if (fx.trail) drawTrail(g, plan.points, t, fx.trail === true ? {} : fx.trail, boostFor(s));
  for (const r of st.ripples) {
    if (r.age > r.ms) continue;
    const k = r.age / r.ms;
    g.strokeStyle = `rgba(159,215,255,${0.75 * (1 - k)})`;
    g.lineWidth = (4 * (1 - k) + 1) * boostFor(s);
    g.beginPath();
    g.arc(
      r.x,
      r.y,
      (8 + (r.kind === 'teleport' ? 70 : 44) * easeOut(k)) * Math.sqrt(boostFor(s)),
      0,
      Math.PI * 2
    );
    g.stroke();
  }
  // Press squish: quick in, springy out.
  let pressK = 0;
  const squash = 1 - (fx.squashPress ?? 0.88);
  const lastPress = [...plan.events]
    .reverse()
    .find((e) => e.t <= t && (e.type === 'press' || e.type === 'release'));
  if (lastPress) {
    const age = t - lastPress.t;
    pressK =
      lastPress.type === 'press'
        ? squash * Math.min(1, age / 50)
        : squash *
          Math.max(0, Math.cos(Math.min(1, age / 220) * Math.PI * 1.5)) *
          Math.max(0, 1 - age / 220);
  }
  const vel = velocityAt(plan.points, t);
  // Keep the cursor legible in small tiles (at least ~22 device px tall).
  const boost = boostFor(s);
  drawCursor(g, q, { vel, fx, pressK, st, boost });
  drawThink(g, q, st.think, t, boost);
  return q;
}
