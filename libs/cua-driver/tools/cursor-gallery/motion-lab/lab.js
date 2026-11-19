// Motion lab UI: gallery of every candidate animating side by side, a
// single-candidate inspector with kinematics plots, a compare view for picked
// candidates, and a static contact sheet. Pure ES modules, no build step.

import { byId, candidates, categories } from './motion/candidates.js';
import { kinematics, metrics, plan, resolveParams } from './motion/plan.js';
import { sceneList, scenes } from './motion/scenes.js';
import { renderFrame } from './render.js';

const LOOP_PAD_MS = 900;
const PICKS_KEY = 'cua-motion-lab.picks';
const PARAMS_KEY = 'cua-motion-lab.params';

const params = new URLSearchParams(location.search);
const capture = params.get('capture') === '1';
if (capture) document.body.classList.add('capture');
if (params.get('layout')) document.body.classList.add(params.get('layout'));

const SETS = [
  {
    id: 'showcase',
    name: 'Showcase (all but realism refs)',
    filter: (c) => c.tier !== 'reference',
  },
  { id: 'dc', name: "Director's cut (12)", filter: (c) => c.category === 'directors-cut' },
  ...categories
    .filter((c) => c.id !== 'directors-cut')
    .map((cat) => ({
      id: cat.id,
      name: cat.name,
      filter: (c) => c.category === cat.id && c.tier !== 'reference',
    })),
  { id: 'reference', name: 'Realism references', filter: (c) => c.tier === 'reference' },
  { id: 'all', name: 'Everything', filter: () => true },
];

const state = {
  set: params.get('set') ?? 'showcase',
  scene: params.get('scene') ?? 'route',
  timing: params.get('timing') ?? 'native',
  seed: Number(params.get('seed') ?? 7),
  speed: Number(params.get('speed') ?? 1),
  showPath: params.get('path') === '1',
  ids:
    params
      .get('ids')
      ?.split(',')
      .filter((id) => byId[id.split('@')[0]]) ?? null,
  picks: new Set(JSON.parse(localStorage.getItem(PICKS_KEY) ?? '[]')),
  overrides: JSON.parse(localStorage.getItem(PARAMS_KEY) ?? '{}'),
  start: performance.now(),
  virtualTime: null,
};

const plans = new Map();
function planFor(c, sceneId = state.scene, timing = state.timing) {
  const key = `${c.id}|${sceneId}|${state.seed}|${timing}|${JSON.stringify(state.overrides[c.id] ?? {})}`;
  if (!plans.has(key))
    plans.set(
      key,
      plan(c, scenes[sceneId], { seed: state.seed, timing, params: state.overrides[c.id] ?? {} })
    );
  return plans.get(key);
}
const TIMING_LABEL = { native: 'native timing', fitts: 'Fitts timing', fixed: 'fixed 1.43 s' };

const now = () => (state.virtualTime ?? performance.now() - state.start) * state.speed;
const fmt = (v, d = 0) => (Number.isFinite(v) ? v.toFixed(d) : '-');
const catName = (id) => categories.find((c) => c.id === id)?.name ?? id;

// ---------------------------------------------------------------------------
// Controls
// ---------------------------------------------------------------------------

const $ = (id) => document.getElementById(id);
function initControls() {
  $('set').innerHTML = SETS.map((s) => `<option value="${s.id}">${s.name}</option>`).join('');
  $('scene').innerHTML = sceneList
    .map((s) => `<option value="${s.id}">${s.name}</option>`)
    .join('');
  $('set').value = state.set;
  $('scene').value = state.scene;
  $('timing').value = state.timing;
  $('speed').value = state.speed;
  $('speed-out').textContent = `${state.speed.toFixed(2)}x`;
  $('show-path').checked = state.showPath;
  $('set').onchange = (e) => ((state.set = e.target.value), route());
  $('scene').onchange = (e) => ((state.scene = e.target.value), replay(), route());
  $('timing').onchange = (e) => ((state.timing = e.target.value), replay(), route());
  $('speed').oninput = (e) => {
    const t = now();
    state.speed = Number(e.target.value);
    state.start = performance.now() - t / state.speed;
    $('speed-out').textContent = `${state.speed.toFixed(2)}x`;
  };
  $('show-path').onchange = (e) => (state.showPath = e.target.checked);
  $('shuffle').onclick = () => ((state.seed = Math.floor(Math.random() * 1e6)), replay(), route());
  $('replay').onclick = replay;
  updatePickCount();
}

function replay() {
  state.start = performance.now();
}

function savePicks() {
  localStorage.setItem(PICKS_KEY, JSON.stringify([...state.picks]));
  updatePickCount();
}
function updatePickCount() {
  $('pick-count').textContent = state.picks.size ? `(${state.picks.size})` : '';
}

// ---------------------------------------------------------------------------
// Tiles
// ---------------------------------------------------------------------------

let tiles = [];

function sizeCanvas(canvas) {
  const dpr = Math.min(2, window.devicePixelRatio || 1);
  const r = canvas.getBoundingClientRect();
  const w = Math.max(1, Math.round(r.width * dpr));
  const h = Math.max(1, Math.round(r.height * dpr));
  if (canvas.width !== w || canvas.height !== h) {
    canvas.width = w;
    canvas.height = h;
  }
}

function tileEl(c, i, { showNum = true } = {}) {
  const el = document.createElement('article');
  el.className = 'tile';
  el.innerHTML = `
    ${showNum ? `<span class="num">${String(i + 1).padStart(2, '0')}</span>` : ''}
    <a href="#/c/${c.id}"><canvas></canvas></a>
    <div class="meta">
      <div><div class="name">${c.name}</div><div class="sub">${c.id} · ${catName(c.category)}</div></div>
      <button class="pick ${state.picks.has(c.id) ? 'on' : ''}" title="Pick for porting" aria-label="Pick ${c.name}">${state.picks.has(c.id) ? '★' : '☆'}</button>
    </div>`;
  el.querySelector('.pick').onclick = (e) => {
    e.preventDefault();
    if (state.picks.has(c.id)) state.picks.delete(c.id);
    else state.picks.add(c.id);
    savePicks();
    e.target.classList.toggle('on', state.picks.has(c.id));
    e.target.textContent = state.picks.has(c.id) ? '★' : '☆';
  };
  return el;
}

function drawTiles(t) {
  for (const tile of tiles) {
    sizeCanvas(tile.canvas);
    const p = planFor(tile.c, tile.scene, tile.timing ?? state.timing);
    const local = state.noLoop ? Math.min(t, p.duration) : t % (p.duration + LOOP_PAD_MS);
    renderFrame(tile.g, tile.canvas.width, tile.canvas.height, {
      plan: p,
      scene: scenes[tile.scene],
      candidate: tile.c,
      t: local,
      showPath: state.showPath,
    });
  }
}

// ---------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------

function currentList() {
  if (state.ids) return state.ids.map((id) => byId[id.split('@')[0]]);
  const set = SETS.find((s) => s.id === state.set) ?? SETS[0];
  const list = candidates.filter(set.filter);
  const order = categories.map((c) => c.id);
  return list.sort((a, b) => order.indexOf(a.category) - order.indexOf(b.category));
}

function galleryView(view) {
  const list = currentList();
  const cols = params.get('cols');
  view.innerHTML = `
    <div class="intro">
      <div>
        <h1>${list.length} agent cursor <em>motion styles</em></h1>
        <p>Same seed, same targets, every style at once. ${scenes[state.scene].blurb} Timing: ${$('timing').selectedOptions[0].text}.</p>
      </div>
    </div>
    <div class="grid" id="grid"></div>`;
  const grid = view.querySelector('#grid');
  if (cols) grid.style.gridTemplateColumns = `repeat(${cols}, 1fr)`;
  tiles = list.map((c, i) => {
    const timing = state.ids?.[i]?.split('@')[1];
    const el = tileEl(c, i);
    if (timing)
      el.querySelector('.name').textContent = `${c.name} · ${TIMING_LABEL[timing] ?? timing}`;
    grid.appendChild(el);
    const canvas = el.querySelector('canvas');
    return { c, canvas, g: canvas.getContext('2d'), scene: state.scene, timing };
  });
  const title = params.get('title');
  if (title) view.querySelector('.intro h1').innerHTML = title;
  const sub = params.get('sub');
  if (sub !== null) view.querySelector('.intro p').textContent = sub;
}

// Full-bleed single-candidate clip for recordings.
function clipView(view, id) {
  const c = byId[id];
  const sceneId = params.get('scene') ?? c.bestScene ?? 'route';
  view.innerHTML = `
    <div class="clip">
      <canvas></canvas>
      <div class="caption">
        <div class="clip-cat">${catName(c.category)}</div>
        <div class="clip-name">${c.name}</div>
        <div class="clip-look">${c.look}</div>
      </div>
      <div class="clip-brand"><span class="brand-wordmark">cua</span> <span>Driver · agent cursor</span></div>
    </div>`;
  const canvas = view.querySelector('canvas');
  tiles = [{ c, canvas, g: canvas.getContext('2d'), scene: sceneId }];
}

function compareView(view) {
  const list = [...state.picks].map((id) => byId[id]).filter(Boolean);
  view.innerHTML = `
    <div class="intro"><div><h1>Compare <em>picked</em></h1>
    <p>${list.length ? 'Picked styles side by side on the current scene. Export the picks with their parameters for porting.' : 'Pick styles with the star in the gallery.'}</p></div></div>
    <div class="grid" id="grid" style="--tile: 420px"></div>
    ${list.length ? '<div class="row-actions"><button id="export">Export picks JSON</button><button id="clear">Clear picks</button></div><textarea class="export" id="export-out" readonly></textarea>' : ''}`;
  const grid = view.querySelector('#grid');
  tiles = list.map((c, i) => {
    const el = tileEl(c, i);
    grid.appendChild(el);
    const canvas = el.querySelector('canvas');
    return { c, canvas, g: canvas.getContext('2d'), scene: state.scene };
  });
  const out = view.querySelector('#export-out');
  if (out) {
    const json = JSON.stringify(
      {
        timing: state.timing,
        seed: state.seed,
        picks: list.map((c) => ({
          id: c.id,
          name: c.name,
          category: c.category,
          technique: c.technique,
          params: resolveParams(c, state.overrides[c.id] ?? {}),
        })),
      },
      null,
      2
    );
    out.value = json;
    view.querySelector('#export').onclick = () => navigator.clipboard?.writeText(json);
    view.querySelector('#clear').onclick = () => {
      state.picks.clear();
      savePicks();
      route();
    };
  }
}

function sheetView(view) {
  const list = currentList();
  view.innerHTML = `<div class="intro"><div><h1>Contact sheet <em>${list.length} styles</em></h1>
    <p>Full trajectory per style on "${scenes[state.scene].name}"; dots every 100 ms (dense dots = slow).</p></div></div>
    <div class="sheet" id="grid"></div>`;
  const grid = view.querySelector('#grid');
  if (params.get('cols')) grid.style.gridTemplateColumns = `repeat(${params.get('cols')}, 1fr)`;
  tiles = [];
  list.forEach((c, i) => {
    const el = tileEl(c, i);
    el.querySelector('.pick').remove();
    grid.appendChild(el);
    const canvas = el.querySelector('canvas');
    requestAnimationFrame(() => {
      sizeCanvas(canvas);
      const g = canvas.getContext('2d');
      const p = planFor(c);
      renderFrame(g, canvas.width, canvas.height, {
        plan: p,
        scene: scenes[state.scene],
        candidate: c,
        t: p.duration,
        showPath: false,
      });
      g.strokeStyle = 'rgba(159,215,255,0.55)';
      g.lineWidth = 3;
      g.beginPath();
      p.points.forEach((q, k) => (k ? g.lineTo(q.x, q.y) : g.moveTo(q.x, q.y)));
      g.stroke();
      g.fillStyle = '#ffffff';
      let next = 0;
      for (const q of p.points) {
        if (q.t >= next) {
          g.beginPath();
          g.arc(q.x, q.y, 4.5, 0, Math.PI * 2);
          g.fill();
          next += 100;
        }
      }
    });
  });
  document.documentElement.dataset.sheetReady = '1';
}

// Single-candidate inspector.
let single = null;
function singleView(view, id) {
  const c = byId[id];
  if (!c) {
    view.innerHTML = '<p class="empty">Unknown candidate.</p>';
    return;
  }
  const list = currentList();
  const idx = list.findIndex((x) => x.id === id);
  const prev = list[(idx - 1 + list.length) % list.length] ?? c;
  const next = list[(idx + 1) % list.length] ?? c;
  const sceneId =
    params.get('scene') ?? (state.scene === 'route' && c.bestScene ? c.bestScene : state.scene);
  const p = planFor(c, sceneId);
  const m = metrics(p);
  const vals = resolveParams(c, state.overrides[c.id] ?? {});
  view.innerHTML = `
    <div class="single">
      <div>
        <div class="stage"><canvas id="single-canvas"></canvas></div>
        <div class="plots">
          <div class="plot"><h4>Speed (pt/s)</h4><canvas id="plot-speed"></canvas></div>
          <div class="plot"><h4>Tangential acceleration (pt/s²)</h4><canvas id="plot-accel"></canvas></div>
        </div>
      </div>
      <aside class="panel">
        <div class="cat">${catName(c.category)}${c.tier === 'reference' ? ' · realism reference' : ''}</div>
        <h2>${c.name}</h2>
        <div class="tech">${c.id}</div>
        <p>${c.look}</p>
        <p class="tech">${c.technique}</p>
        ${c.basedOn ? `<p class="tech">Built from: ${c.basedOn.join(', ')}</p>` : ''}
        <div class="kv">
          <span>Scene</span><span>${scenes[sceneId].name}</span>
          <span>Total</span><span>${fmt(m.durationMs / 1000, 2)} s</span>
          <span>Moving</span><span>${fmt(m.moveMs / 1000, 2)} s</span>
          <span>Peak speed</span><span>${fmt(m.peakSpeed)} pt/s</span>
          <span>Path efficiency</span><span>${fmt(m.efficiency * 100)}%</span>
          <span>Max overshoot</span><span>${fmt(m.overshootPt, 1)} pt</span>
          <span>Extra submovements</span><span>${m.extraSubmovements}</span>
        </div>
        <div id="params"></div>
        <div class="row-actions">
          <a href="#/c/${prev.id}">← ${prev.name}</a>
          <a href="#/c/${next.id}">${next.name} →</a>
          <button id="pick-one">${state.picks.has(c.id) ? '★ Picked' : '☆ Pick'}</button>
          <button id="reset-params">Reset params</button>
        </div>
      </aside>
    </div>`;
  const box = view.querySelector('#params');
  for (const [k, spec] of Object.entries(c.params ?? {})) {
    const v = vals[k];
    const row = document.createElement('label');
    row.className = 'param';
    row.innerHTML = `<span>${k}</span><input value="${Array.isArray(v) ? v.join(', ') : v}" /><span class="doc">${spec.doc}</span>`;
    row.querySelector('input').onchange = (e) => {
      const parts = e.target.value.split(',').map((s) => Number(s.trim()));
      if (parts.some((n) => !Number.isFinite(n))) return;
      state.overrides[c.id] = {
        ...(state.overrides[c.id] ?? {}),
        [k]: parts.length > 1 ? parts : parts[0],
      };
      localStorage.setItem(PARAMS_KEY, JSON.stringify(state.overrides));
      replay();
      route();
    };
    box.appendChild(row);
  }
  view.querySelector('#pick-one').onclick = (e) => {
    if (state.picks.has(c.id)) state.picks.delete(c.id);
    else state.picks.add(c.id);
    savePicks();
    e.target.textContent = state.picks.has(c.id) ? '★ Picked' : '☆ Pick';
  };
  view.querySelector('#reset-params').onclick = () => {
    delete state.overrides[c.id];
    localStorage.setItem(PARAMS_KEY, JSON.stringify(state.overrides));
    route();
  };
  const canvas = view.querySelector('#single-canvas');
  tiles = [{ c, canvas, g: canvas.getContext('2d'), scene: sceneId }];
  const k = kinematics(p.points);
  single = {
    plan: p,
    k,
    speed: view.querySelector('#plot-speed'),
    accel: view.querySelector('#plot-accel'),
  };
}

function drawPlot(canvas, xs, ys, t, segs, { color, signed = false }) {
  sizeCanvas(canvas);
  const g = canvas.getContext('2d');
  const W = canvas.width;
  const H = canvas.height;
  g.clearRect(0, 0, W, H);
  const T = xs[xs.length - 1] || 1;
  const max = Math.max(1, ...ys.map(Math.abs));
  const y0 = signed ? H / 2 : H - 4;
  const sy = signed ? (H / 2 - 6) / max : (H - 10) / max;
  g.fillStyle = 'rgba(255,255,255,0.04)';
  for (const s of segs) g.fillRect((s.t0 / T) * W, 0, ((s.t1 - s.t0) / T) * W, H);
  g.strokeStyle = 'rgba(255,255,255,0.15)';
  g.beginPath();
  g.moveTo(0, y0);
  g.lineTo(W, y0);
  g.stroke();
  g.strokeStyle = color;
  g.lineWidth = 2;
  g.beginPath();
  xs.forEach((x, i) => {
    const px = (x / T) * W;
    const py = y0 - ys[i] * sy;
    if (i) g.lineTo(px, py);
    else g.moveTo(px, py);
  });
  g.stroke();
  g.fillStyle = 'rgba(255,255,255,0.55)';
  g.font = `${11 * (W / canvas.getBoundingClientRect().width)}px JetBrains Mono, monospace`;
  g.fillText(`max ${Math.round(max)}`, 6, 14 * (W / canvas.getBoundingClientRect().width));
  g.strokeStyle = '#ffffff';
  g.beginPath();
  g.moveTo((t / T) * W, 0);
  g.lineTo((t / T) * W, H);
  g.stroke();
}

// ---------------------------------------------------------------------------
// Router and loop
// ---------------------------------------------------------------------------

function route() {
  const view = $('view');
  const hash = location.hash.replace(/^#/, '') || '/';
  single = null;
  document
    .querySelectorAll('.tabs a')
    .forEach((a) =>
      a.classList.toggle(
        'active',
        (hash === '/' && a.dataset.route === 'gallery') || hash.startsWith(`/${a.dataset.route}`)
      )
    );
  if (hash.startsWith('/c/')) singleView(view, hash.slice(3));
  else if (hash.startsWith('/clip/')) clipView(view, hash.slice(6));
  else if (hash.startsWith('/compare')) compareView(view);
  else if (hash.startsWith('/sheet')) sheetView(view);
  else galleryView(view);
}

function frame() {
  const t = now();
  drawTiles(t);
  if (single) {
    const local = t % (single.plan.duration + LOOP_PAD_MS);
    drawPlot(single.speed, single.k.t, single.k.speed, local, single.plan.segments, {
      color: '#9fd7ff',
    });
    drawPlot(single.accel, single.k.t, single.k.along, local, single.plan.segments, {
      color: '#f6c177',
      signed: true,
    });
  }
}

function loop() {
  if (state.virtualTime === null) frame();
  requestAnimationFrame(loop);
}

// Hooks for deterministic capture (headless Chrome drives the clock).
window.__lab = {
  setTime(ms) {
    state.virtualTime = ms;
    frame();
    return true;
  },
  durations: () => tiles.map((t) => planFor(t.c, t.scene).duration),
  ids: () => tiles.map((t) => t.c.id),
};

initControls();
window.addEventListener('hashchange', () => {
  replay();
  route();
});
route();
if (capture) state.virtualTime = 0;
requestAnimationFrame(loop);
document.documentElement.dataset.labReady = '1';
