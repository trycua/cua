// Cua Cursor Motion playground. Plain ES module over the built package (../dist).
// The cursor is drawn inside the canvas; the page's own pointer is untouched.
import * as M from '../dist/index.js';

const $ = (id) => document.getElementById(id);
const stage = $('stage');
const plot = $('speedplot');

const state = {
  mode: 'style', // or 'custom'
  params: M.motionParams(),
  spec: M.defaultSpec(),
  a: null,
  b: null,
  target: { w: 96, h: 36 },
  ghost: null,
};

// ── Stage ────────────────────────────────────────────────────────────────

const rectAround = (p) => [
  p.x - state.target.w / 2,
  p.y - state.target.h / 2,
  state.target.w,
  state.target.h,
];

function drawScene(g) {
  const w = stage.clientWidth;
  const h = stage.clientHeight;
  g.fillStyle = '#0b0d10';
  g.fillRect(0, 0, w, h);
  g.fillStyle = 'rgba(255,255,255,0.05)';
  for (let x = 32; x < w; x += 32) for (let y = 32; y < h; y += 32) g.fillRect(x - 1, y - 1, 2, 2);
  for (const [p, label] of [
    [state.a, 'start'],
    [state.b, 'end'],
  ]) {
    const [x, y, rw, rh] = rectAround(p);
    g.fillStyle = 'rgba(255,255,255,0.04)';
    g.strokeStyle = 'rgba(255,255,255,0.18)';
    g.lineWidth = 1;
    g.beginPath();
    g.roundRect(x, y, rw, rh, 7);
    g.fill();
    g.stroke();
    g.fillStyle = 'rgba(236,242,250,0.55)';
    g.font = '500 12px system-ui, -apple-system, sans-serif';
    g.textAlign = 'center';
    g.fillText(label, p.x, y + rh + 16);
  }
  if ($('ghost').checked && state.ghost) {
    g.strokeStyle = 'rgba(159,215,255,0.28)';
    g.lineWidth = 1.5;
    g.setLineDash([4, 5]);
    g.beginPath();
    state.ghost.samples.forEach((s, i) => (i ? g.lineTo(s.x, s.y) : g.moveTo(s.x, s.y)));
    g.stroke();
    g.setLineDash([]);
  }
  for (const p of [state.a, state.b]) {
    g.strokeStyle = 'rgba(236,242,250,0.85)';
    g.lineWidth = 1.5;
    g.beginPath();
    g.arc(p.x, p.y, 6, 0, 2 * Math.PI);
    g.stroke();
  }
}

const player = new M.MotionPlayer(stage, { drawScene });

function moveOptions(to) {
  const common = { target: rectAround(to), click: true };
  return state.mode === 'custom'
    ? { ...common, spec: state.spec }
    : { ...common, params: state.params };
}

function planGhost() {
  const req = { from: state.a, to: state.b, target: rectAround(state.b), seed: 'ghost' };
  state.ghost =
    state.mode === 'custom' ? M.planSpec(state.spec, req) : M.planMove(state.params, req);
  const t = state.ghost;
  $('stats').textContent =
    `${Math.round(t.arrivalT * 1000)} ms to arrive, ${Math.round(t.duration() * 1000)} ms with settle`;
  drawPlot(t);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms / player.timeScale));
let run = 0;
async function play() {
  const id = ++run;
  planGhost();
  player.place(state.a);
  await sleep(350);
  while (id === run) {
    await player.moveTo(state.b, moveOptions(state.b));
    if (id !== run) return;
    await sleep(900);
    if (!$('loop').checked || id !== run) return;
    await player.moveTo(state.a, moveOptions(state.a));
    if (id !== run) return;
    await sleep(900);
  }
}

let pending = 0;
function changed() {
  clearTimeout(pending);
  planGhost();
  renderExport();
  pending = setTimeout(play, 120);
}

// Drag the start and end points. Only the canvas drawing changes; the real
// pointer keeps its normal look.
let dragging = null;
const local = (e) => {
  const r = stage.getBoundingClientRect();
  return { x: e.clientX - r.left, y: e.clientY - r.top };
};
stage.addEventListener('pointerdown', (e) => {
  const p = local(e);
  const near = (q) => Math.hypot(q.x - p.x, q.y - p.y) < 24;
  dragging = near(state.b) ? 'b' : near(state.a) ? 'a' : null;
  if (dragging) {
    stage.setPointerCapture(e.pointerId);
    run++;
  }
});
stage.addEventListener('pointermove', (e) => {
  if (!dragging) return;
  const p = local(e);
  state[dragging] = {
    x: Math.min(Math.max(p.x, 12), stage.clientWidth - 12),
    y: Math.min(Math.max(p.y, 12), stage.clientHeight - 12),
  };
  planGhost();
  player.place(state.a);
});
stage.addEventListener('pointerup', () => {
  if (!dragging) return;
  dragging = null;
  changed();
});

// ── Speed plot ───────────────────────────────────────────────────────────

function drawPlot(traj) {
  const dpr = window.devicePixelRatio || 1;
  const w = plot.clientWidth;
  const h = plot.clientHeight;
  plot.width = Math.round(w * dpr);
  plot.height = Math.round(h * dpr);
  const g = plot.getContext('2d');
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  g.clearRect(0, 0, w, h);
  const s = traj.samples;
  const speeds = s.slice(1).map((q, i) => {
    const p = s[i];
    const dt = q.t - p.t;
    return { t: q.t, v: dt > 0 ? Math.hypot(q.x - p.x, q.y - p.y) / dt : 0 };
  });
  const tMax = Math.max(traj.duration(), 0.001);
  const vMax = Math.max(...speeds.map((p) => p.v), 1);
  const pad = 22;
  const X = (t) => pad + (t / tMax) * (w - 2 * pad);
  const Y = (v) => h - 18 - (v / vMax) * (h - 30);
  g.strokeStyle = 'rgba(255,255,255,0.12)';
  g.beginPath();
  g.moveTo(pad, h - 18);
  g.lineTo(w - pad, h - 18);
  g.stroke();
  g.strokeStyle = '#5ec0e8';
  g.lineWidth = 1.75;
  g.beginPath();
  speeds.forEach((p, i) => (i ? g.lineTo(X(p.t), Y(p.v)) : g.moveTo(X(p.t), Y(p.v))));
  g.stroke();
  g.strokeStyle = 'rgba(236,242,250,0.5)';
  g.setLineDash([3, 4]);
  g.beginPath();
  g.moveTo(X(traj.arrivalT), 6);
  g.lineTo(X(traj.arrivalT), h - 18);
  g.stroke();
  g.setLineDash([]);
  g.fillStyle = 'rgba(236,242,250,0.6)';
  g.font = '11px system-ui, -apple-system, sans-serif';
  g.textAlign = 'left';
  g.fillText(`speed, peak ${Math.round(vMax)} pt/s`, pad, 12);
  g.textAlign = 'center';
  g.fillText('arrival', X(traj.arrivalT), h - 4);
}

// ── Controls ─────────────────────────────────────────────────────────────

function slider(parent, label, { min, max, step, get, set, fmt = (v) => v }) {
  const row = document.createElement('label');
  row.className = 'slider';
  const name = document.createElement('span');
  name.textContent = label;
  const input = Object.assign(document.createElement('input'), { type: 'range', min, max, step });
  input.value = get();
  const out = document.createElement('output');
  out.textContent = fmt(Number(input.value));
  input.addEventListener('input', () => {
    set(Number(input.value));
    out.textContent = fmt(Number(input.value));
    changed();
  });
  row.append(name, input, out);
  parent.append(row);
  return row;
}

function select(parent, label, options, get, set) {
  const row = document.createElement('label');
  row.className = 'row';
  row.append(label + ' ');
  const sel = document.createElement('select');
  for (const [value, text] of options) sel.append(new Option(text, value));
  sel.value = get();
  sel.addEventListener('change', () => {
    set(sel.value);
    build();
    changed();
  });
  row.append(sel);
  parent.append(row);
}

const clearBox = (el) => el.querySelectorAll(':scope > :not(legend)').forEach((n) => n.remove());
const f2 = (v) => v.toFixed(2);

function buildStyle() {
  const style = $('style');
  style.replaceChildren(
    ...M.MOTION_STYLES.map((s) => new Option(s, s)),
    new Option('custom', 'custom')
  );
  style.value = state.mode === 'custom' ? 'custom' : state.params.style;
  $('timing').value = state.params.timing;
  $('timing').disabled = state.mode === 'custom';

  clearBox($('glide-row'));
  if (state.mode === 'style' && state.params.timing === 'fixed') {
    slider($('glide-row'), 'Move time', {
      min: 0,
      max: 3000,
      step: 10,
      get: () => state.params.glideDurationMs,
      set: (v) => (state.params.glideDurationMs = v),
      fmt: (v) => (v ? `${v} ms` : '1430 ms'),
    });
  }
  clearBox($('target-row'));
  slider($('target-row'), 'Target width', {
    min: 8,
    max: 240,
    step: 1,
    get: () => state.target.w,
    set: (v) => {
      state.target.w = v;
      state.target.h = Math.min(v, 36);
    },
    fmt: (v) => `${v} pt`,
  });
}

function buildKnobs() {
  const box = $('knobs');
  clearBox(box);
  box.hidden = state.mode === 'custom';
  if (box.hidden) return;
  const p = state.params;
  const k = (label, key, min, max, step, note) =>
    slider(box, note ? `${label} (${note})` : label, {
      min,
      max,
      step,
      get: () => p[key],
      set: (v) => (p[key] = v),
      fmt: f2,
    });
  const arc = !['magnetic', 'adaptive', 'classic'].includes(p.style);
  if (arc) {
    k('Arc size', 'arcSize', 0, 1, 0.01);
    k('Arc flow', 'arcFlow', -1, 1, 0.01);
    k('Start handle', 'startHandle', 0, 1, 0.01);
    k('End handle', 'endHandle', 0, 1, 0.01);
  } else if (p.style === 'classic') {
    k('Spring', 'spring', 0.3, 1, 0.01);
    k('Turn radius', 'turnRadius', 10, 300, 1);
  } else {
    const note = document.createElement('p');
    note.className = 'note';
    note.textContent =
      p.style === 'magnetic'
        ? 'Magnetic is a physics simulation: it slows to a 40 pt capture radius, then the target pulls it in.'
        : 'Adaptive picks a precise approach for targets under 16 pt, a swoop beyond 900 pt, and a Fitts min-jerk glide otherwise. Try the target width.';
    box.append(note);
  }
}

function buildEffects() {
  const box = $('effects');
  clearBox(box);
  for (const name of M.EFFECT_NAMES) {
    if (state.mode === 'custom') {
      const row = document.createElement('label');
      row.className = 'check';
      const input = Object.assign(document.createElement('input'), {
        type: 'checkbox',
        checked: state.spec.effects[name],
      });
      input.addEventListener('change', () => {
        state.spec.effects[name] = input.checked;
        changed();
      });
      row.append(input, ' ' + name);
      box.append(row);
    } else {
      const base = M.defaultEffects(state.params.style)[name];
      select(
        box,
        name,
        [
          ['default', `style default (${base ? 'on' : 'off'})`],
          ['on', 'on'],
          ['off', 'off'],
        ],
        () => {
          const v = state.params.effects[name];
          return v == null ? 'default' : v ? 'on' : 'off';
        },
        (v) => (state.params.effects[name] = v === 'default' ? null : v === 'on')
      );
    }
  }
}

function buildCustom() {
  const box = $('custom');
  clearBox(box);
  box.hidden = state.mode !== 'custom';
  $('to-custom').hidden = state.mode === 'custom';
  if (box.hidden) return;
  const s = state.spec;
  const sub = (title) => {
    const h = document.createElement('h3');
    h.textContent = title;
    box.append(h);
  };
  const num = (obj, label, key, min, max, step, fmt = f2) =>
    slider(box, label, { min, max, step, get: () => obj[key], set: (v) => (obj[key] = v), fmt });

  sub('Path');
  select(
    box,
    'Shape',
    [
      ['arc', 'arc (Cua bezier)'],
      ['bow', 'bow'],
      ['straight', 'straight'],
    ],
    () => s.path.type,
    (v) => {
      s.path =
        v === 'arc'
          ? { type: 'arc', startHandle: 0.3, endHandle: 0.3, arcSize: 0.16, arcFlow: 0.15 }
          : v === 'bow'
            ? { type: 'bow', amount: 0.06 }
            : { type: 'straight' };
    }
  );
  if (s.path.type === 'arc') {
    num(s.path, 'Arc size', 'arcSize', -0.6, 0.6, 0.01);
    num(s.path, 'Arc flow', 'arcFlow', -1, 1, 0.01);
    num(s.path, 'Start handle', 'startHandle', 0, 1, 0.01);
    num(s.path, 'End handle', 'endHandle', 0, 1, 0.01);
  } else if (s.path.type === 'bow') {
    num(s.path, 'Bow', 'amount', -0.4, 0.4, 0.01);
  }

  sub('Speed curve');
  const easeType = typeof s.ease === 'string' ? s.ease : s.ease.type;
  select(
    box,
    'Ease',
    [...M.EASE_NAMES.map((n) => [n, n]), ['cubic_bezier', 'cubic-bezier()']],
    () => easeType,
    (v) => {
      s.ease = v === 'cubic_bezier' ? { type: 'cubic_bezier', x1: 0.3, y1: 0, x2: 0.1, y2: 1 } : v;
    }
  );
  if (easeType === 'cubic_bezier') {
    num(s.ease, 'x1', 'x1', 0, 1, 0.01);
    num(s.ease, 'y1', 'y1', -0.5, 1.5, 0.01);
    num(s.ease, 'x2', 'x2', 0, 1, 0.01);
    num(s.ease, 'y2', 'y2', -0.5, 1.5, 0.01);
  }

  sub('Overshoot and settle');
  select(
    box,
    'Settle',
    [
      ['none', 'none'],
      ['follow_through', 'follow-through'],
      ['spring', 'spring'],
    ],
    () => s.settle.type,
    (v) => {
      s.settle =
        v === 'follow_through'
          ? { type: 'follow_through', amount: 0.018, maxPt: 8, at: 0.82 }
          : v === 'spring'
            ? {
                type: 'spring',
                amount: 0.05,
                maxPt: 6,
                cycles: 1.3,
                decay: 2.6,
                start: 0.55,
                glideEnd: 0.68,
              }
            : { type: 'none' };
    }
  );
  if (s.settle.type === 'follow_through') {
    num(s.settle, 'Amount', 'amount', 0, 0.15, 0.001, (v) => v.toFixed(3));
    num(s.settle, 'Max', 'maxPt', 0, 40, 0.5, (v) => `${v} pt`);
    num(s.settle, 'Peak at', 'at', 0.5, 0.98, 0.01);
  } else if (s.settle.type === 'spring') {
    num(s.settle, 'Amount', 'amount', 0, 0.2, 0.001, (v) => v.toFixed(3));
    num(s.settle, 'Max', 'maxPt', 0, 40, 0.5, (v) => `${v} pt`);
    num(s.settle, 'Cycles', 'cycles', 0.5, 4, 0.1);
    num(s.settle, 'Decay', 'decay', 0.5, 8, 0.1);
    num(s.settle, 'Starts at', 'start', 0.2, 0.9, 0.01);
    num(s.settle, 'Glide ends', 'glideEnd', 0.3, 1, 0.01);
  }

  sub('Duration');
  select(
    box,
    'Model',
    [
      ['fitts', "Fitts' law"],
      ['fixed', 'fixed'],
      ['distance', 'distance'],
    ],
    () => s.duration.type,
    (v) => {
      s.duration =
        v === 'fitts'
          ? M.fittsDuration(1.1)
          : v === 'fixed'
            ? { type: 'fixed', ms: 600 }
            : { type: 'distance', baseMs: 350, msPerPt: 0.35, minMs: 450, maxMs: 1100 };
    }
  );
  if (s.duration.type === 'fitts') num(s.duration, 'Scale', 'scale', 0.3, 3, 0.05);
  else if (s.duration.type === 'fixed')
    num(s.duration, 'Time', 'ms', 80, 3000, 10, (v) => `${v} ms`);
  else {
    num(s.duration, 'Base', 'baseMs', 0, 1000, 10, (v) => `${v} ms`);
    num(s.duration, 'Per point', 'msPerPt', 0, 2, 0.01, (v) => `${v} ms`);
  }
  select(
    box,
    'Heading',
    [
      ['tangent', 'tip leads'],
      ['fixed', 'rest pose'],
    ],
    () => s.heading,
    (v) => (s.heading = v)
  );

  sub('Trail');
  num(s.trail, 'Length', 'secs', 0.05, 0.6, 0.01, (v) => `${Math.round(v * 1000)} ms`);
  num(s.trail, 'Width', 'headWidth', 2, 28, 0.5, (v) => `${v} pt`);
  num(s.trail, 'Opacity', 'alpha', 0.05, 1, 0.01);
}

function build() {
  buildStyle();
  buildKnobs();
  buildCustom();
  buildEffects();
}

$('style').addEventListener('change', (e) => {
  const v = e.target.value;
  if (v === 'custom') {
    toCustom();
    return;
  }
  state.mode = 'style';
  state.params.style = v;
  build();
  changed();
});
$('timing').addEventListener('change', (e) => {
  state.params.timing = e.target.value;
  build();
  changed();
});
function toCustom() {
  state.spec = M.specForStyle(state.params.style, state.params) ?? M.defaultSpec();
  state.mode = 'custom';
  build();
  changed();
}
$('to-custom').addEventListener('click', toCustom);
$('replay').addEventListener('click', play);
$('speed').addEventListener('change', (e) => (player.timeScale = Number(e.target.value)));
$('loop').addEventListener('change', () => $('loop').checked && play());
$('ghost').addEventListener('change', () => {});

// ── Export ───────────────────────────────────────────────────────────────

let tab = 'ts';
function renderExport() {
  let text;
  if (tab === 'ts') {
    text = state.mode === 'custom' ? M.specSnippet(state.spec) : M.paramsSnippet(state.params);
  } else if (state.mode === 'custom') {
    text =
      '# Custom motions play in your own renderer (Rust: cua-cursor-motion plan_spec,\n' +
      '# TypeScript: planSpec). Cua Driver takes one of the six styles and its\n' +
      '# knobs: pick a style to get its cua-driver config.';
  } else {
    const d = M.driverSnippets(state.params, 'demo');
    text = [
      '# Default for new sessions',
      ...d.configSet,
      '',
      '# One running session',
      d.cursorMotion,
      '',
      d.callOnly.length
        ? `# set_agent_cursor_motion (${d.callOnly.join(', ')} only travel this way)`
        : '# set_agent_cursor_motion',
      `cua-driver set_agent_cursor_motion '${JSON.stringify(d.setAgentCursorMotion)}'`,
    ].join('\n');
  }
  $('snippet').textContent = text;
}
document.querySelectorAll('[data-tab]').forEach((b) =>
  b.addEventListener('click', () => {
    tab = b.dataset.tab;
    document
      .querySelectorAll('[data-tab]')
      .forEach((x) => x.setAttribute('aria-selected', String(x === b)));
    renderExport();
  })
);
$('copy').addEventListener('click', async () => {
  await navigator.clipboard.writeText($('snippet').textContent);
  $('copy').textContent = 'Copied';
  setTimeout(() => ($('copy').textContent = 'Copy'), 1200);
});

// ── Start ────────────────────────────────────────────────────────────────

function layout() {
  const w = stage.clientWidth;
  const h = stage.clientHeight;
  state.a = { x: Math.round(w * 0.16), y: Math.round(h * 0.74) };
  state.b = { x: Math.round(w * 0.8), y: Math.round(h * 0.3) };
}
layout();
build();
renderExport();
play();
let resized = 0;
window.addEventListener('resize', () => {
  clearTimeout(resized);
  resized = setTimeout(() => {
    layout();
    changed();
  }, 150);
});
