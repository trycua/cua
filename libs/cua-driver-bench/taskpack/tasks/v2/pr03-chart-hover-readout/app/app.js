"use strict";
// A canvas bar chart. Exact figures appear only in the tooltip while the pointer is over a bar.
const WEEKS = [
  ["W30", "20 Jul"], ["W31", "27 Jul"], ["W32", "3 Aug"], ["W33", "10 Aug"], ["W34", "17 Aug"], ["W35", "24 Aug"],
  ["W36", "31 Aug"], ["W37", "7 Sep"], ["W38", "14 Sep"], ["W39", "21 Sep"], ["W40", "28 Sep"], ["W41", "5 Oct"],
];
const SERIES = [
  { name: "Returned by customers", color: "#2f7f86", values: [288, 301, 356, 417, 334, 297, 415, 371, 418, 322, 344, 398] },
  { name: "Refused at the door", color: "#d0912f", values: [140, 162, 431, 128, 175, 150, 166, 422, 139, 181, 157, 170] },
];
const W = 1000, H = 360;
const PLOT = { left: 64, right: 980, top: 46, bottom: 312 };
const MAX = 500;
const BAR_W = 24, GAP = 4;
const $ = (id) => document.getElementById(id);
const canvas = $("chart");
const ctx = canvas.getContext("2d");
let state = { submissions: [] };
let bars = [];

function layout() {
  bars = [];
  const slot = (PLOT.right - PLOT.left) / WEEKS.length;
  WEEKS.forEach((wk, i) => {
    const mid = Math.round(PLOT.left + slot * (i + 0.5));
    SERIES.forEach((s, j) => {
      const h = Math.round(((PLOT.bottom - PLOT.top) * s.values[i]) / MAX);
      const x = j === 0 ? mid - GAP / 2 - BAR_W : mid + GAP / 2;
      bars.push({ x, y: PLOT.bottom - h, w: BAR_W, h, week: i, series: j });
    });
  });
}
function draw() {
  const dpr = window.devicePixelRatio || 1;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, W, H);
  ctx.font = "12px -apple-system, Helvetica, sans-serif";
  ctx.textBaseline = "middle";
  // axis: three labels only, no gridlines between them
  [0, 250, 500].forEach((v) => {
    const y = Math.round(PLOT.bottom - ((PLOT.bottom - PLOT.top) * v) / MAX) + 0.5;
    ctx.strokeStyle = v === 0 ? "#555" : "#e4e7ea";
    ctx.beginPath(); ctx.moveTo(PLOT.left, y); ctx.lineTo(PLOT.right, y); ctx.stroke();
    ctx.fillStyle = "#555"; ctx.textAlign = "right";
    ctx.fillText(String(v), PLOT.left - 8, y);
  });
  bars.forEach((b) => { ctx.fillStyle = SERIES[b.series].color; ctx.fillRect(b.x, b.y, b.w, b.h); });
  ctx.fillStyle = "#333"; ctx.textAlign = "center";
  WEEKS.forEach((wk, i) => {
    const slot = (PLOT.right - PLOT.left) / WEEKS.length;
    const mid = PLOT.left + slot * (i + 0.5);
    ctx.fillText(wk[0], mid, PLOT.bottom + 16);
    ctx.fillStyle = "#777"; ctx.fillText(wk[1], mid, PLOT.bottom + 32); ctx.fillStyle = "#333";
  });
  let x = PLOT.left;
  ctx.textAlign = "left";
  SERIES.forEach((s) => {
    ctx.fillStyle = s.color; ctx.fillRect(x, 14, 14, 14);
    ctx.fillStyle = "#222"; ctx.font = "13px -apple-system, Helvetica, sans-serif";
    ctx.fillText(s.name, x + 20, 21);
    x += 40 + ctx.measureText(s.name).width;
  });
}
function hit(px, py) {
  return bars.find((b) => px >= b.x && px < b.x + b.w && py >= b.y - 6 && py <= PLOT.bottom) || null;
}
let lastTip = null;
function onMove(ev) {
  const r = canvas.getBoundingClientRect();
  const px = ((ev.clientX - r.left) * W) / r.width;
  const py = ((ev.clientY - r.top) * H) / r.height;
  const b = hit(px, py);
  const tip = $("tooltip");
  if (!b) { tip.hidden = true; return; }
  const wk = WEEKS[b.week];
  tip.textContent = `Week ${wk[0].slice(1)} (${wk[1]}) · ${SERIES[b.series].name}: ${SERIES[b.series].values[b.week]}`;
  tip.hidden = false;
  const key = `${b.series}:${b.week}`;
  if (key !== lastTip) { lastTip = key; v2.event("tooltip_shown", { series: SERIES[b.series].name, week: wk[0] }); }  // audit: real readouts
  tip.style.left = `${Math.min(b.x + b.w + 8, W - 260)}px`;
  tip.style.top = `${Math.max(b.y - 30, 0)}px`;
}

async function submit(ev) {
  ev.preventDefault();
  const err = $("form-error");
  err.hidden = true;
  const week = $("week").value;
  const raw = $("count").value.trim();
  if (!week) { err.textContent = "Choose a week."; err.hidden = false; return; }
  if (!/^\d+$/.test(raw)) { err.textContent = "Enter the number of returns as a whole number."; err.hidden = false; return; }
  const entry = { depot: $("depot").value, week, returns: Number(raw) };
  state.submissions.push(entry);
  await v2.save(state);
  await v2.event("answer_submitted", { ...entry, trusted: ev.isTrusted });
  const c = $("confirmation");
  c.textContent = `Answer sent for ${entry.depot}: ${week}, ${entry.returns} returns.`;
  c.hidden = false;
}

async function init() {
  state = await v2.state();
  if (!Array.isArray(state.submissions)) state.submissions = [];
  const dpr = window.devicePixelRatio || 1;
  canvas.width = W * dpr;
  canvas.height = H * dpr;
  layout();
  draw();
  WEEKS.forEach(([id, start]) => $("week").append(v2.el("option", { value: id, text: `Week ${id.slice(1)} (from ${start})` })));
  canvas.addEventListener("mousemove", onMove);
  canvas.addEventListener("mouseleave", () => { $("tooltip").hidden = true; });
  $("answer").addEventListener("submit", submit);
}
init();
