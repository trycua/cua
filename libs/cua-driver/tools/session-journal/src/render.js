// Renders the journal model as one HTML page: a cover with the session facts,
// a contents page and a comic panel per turn. The page loads nothing from the
// network; screenshots arrive as URLs from the caller, usually data: URIs.

const OUTCOME_LABELS = {
  confirmed: 'Confirmed',
  done: 'Done',
  partial: 'Partial',
  unverifiable: 'Unverified',
  suspected_noop: 'Suspected no-op',
  failed: 'Failed',
  refused: 'Refused',
};

const DELIVERY_LABELS = {
  background: 'background',
  foreground: 'foreground',
  unknown: 'delivery unknown',
};

const TARGETING_LABELS = {
  element: 'by element',
  pixel: 'by pixel',
  focused: 'focused control',
  desktop: 'whole desktop',
};

const HIDDEN_FRAMES = {
  desktop: 'Full-desktop screenshot hidden. Rebuild with --include-desktop to show it.',
  privacy: 'Cua Driver kept screenshots out of the recording for this turn.',
  missing: 'No screenshot was captured for this turn.',
};

const LEGEND = [
  '<b>Confirmed</b>: the driver read the result back.',
  '<b>Done</b>: no error and nothing to verify.',
  '<b>Unverified</b>: input was sent but not read back.',
  '<b>Failed</b>: the call returned an error.',
  '<b>Refused</b>: the driver stopped before sending input.',
  '<b>No visible change</b>: the before and after screenshots are identical.',
].join(' ');

// 24×24 stroke icons for the action kinds in journal.js.
const GLYPHS = {
  click: 'M5 4l14 6-6 2-2 6z M13 12l6 6',
  type: 'M4 7V5h11v2 M9.5 5v14 M7 19h5 M18 9v10 M16.5 9h3 M16.5 19h3',
  key: 'M5 6h14a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2z M8 14h8',
  hotkey: 'M5 6h14a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2z M12 9v6 M9 12h6',
  value: 'M4 20h4L19 9l-4-4L4 16z M13 7l4 4',
  scroll: 'M8 9l4-4 4 4 M8 15l4 4 4-4 M12 5v14',
  drag: 'M12 3v18 M3 12h18 M9 6l3-3 3 3 M9 18l3 3 3-3 M6 9l-3 3 3 3 M18 9l3 3-3 3',
  move: 'M12 2v6 M12 16v6 M2 12h6 M16 12h6 M12 10a2 2 0 1 0 0 4 2 2 0 0 0 0-4z',
  launch: 'M3 5h18v14H3z M10 9l5 3-5 3z',
  kill: 'M3 5h18v14H3z M9 9l6 6 M15 9l-6 6',
  window: 'M8 4h12v10H8z M4 8v12h12v-2',
  navigate:
    'M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18z M3 12h18 M12 3c3.5 4 3.5 14 0 18 M12 3c-3.5 4-3.5 14 0 18',
  dialog: 'M4 5h16v11H10l-5 4v-4H4z',
  upload: 'M12 16V4 M7 9l5-5 5 5 M4 20h16',
  download: 'M12 4v12 M7 11l5 5 5-5 M4 20h16',
  script: 'M8 7l-5 5 5 5 M16 7l5 5-5 5 M14 4l-4 16',
  clipboard: 'M9 3h6v4H9z M7 5H5v16h14V5h-2',
  record: 'M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18z M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8z',
  inspect: 'M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6z',
  tool: 'M14.5 4a5 5 0 0 0-5 6.6L4 16v4h4l5.4-5.5A5 5 0 0 0 20 9.5l-3 1-3.5-3.5z',
};

// Manga sound effects. Decorative only: hidden from assistive technology.
const SFX = {
  click: 'カチッ',
  type: 'カタカタ',
  key: 'ポン',
  hotkey: 'カチャ',
  value: 'カキッ',
  scroll: 'スーッ',
  drag: 'ズズッ',
  move: 'スッ',
  launch: 'ガチャッ',
  kill: 'バタン',
  window: 'パッ',
  navigate: 'ヒュン',
  script: 'ピピッ',
  clipboard: 'コピー',
  record: 'ジーッ',
};

// A 12-point starburst behind the failed and refused stamps.
const BURST = Array.from({ length: 24 }, (_, i) => {
  const angle = (i * Math.PI) / 12;
  const radius = i % 2 ? 37 : 50;
  return `${(50 + radius * Math.cos(angle)).toFixed(1)}% ${(50 + radius * Math.sin(angle)).toFixed(1)}%`;
}).join(',');

export function renderJournal(journal, { imageUrl = (image) => image.file, logoUrl = null } = {}) {
  const sheet = imageSheet(imageUrl);
  const panels = journal.panels.map((panel) => renderPanel(panel, sheet)).join('\n');
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src data: file: 'self'; style-src 'unsafe-inline'">
<title>${esc(journal.title)} · Cua Driver session journal</title>
<style>
${STYLES}
${sheet.css()}
</style>
</head>
<body>
<main class="journal">
${renderCover(journal, logoUrl)}
${renderContents(journal)}
<section class="panels" aria-label="Turns">
${panels}
</section>
${renderColophon(journal)}
</main>
</body>
</html>
`;
}

// Recordings repeat the same screenshot across turns; each image is written
// into the stylesheet once and panels refer to it by class.
function imageSheet(imageUrl) {
  const classes = new Map();
  return {
    use(image) {
      if (!classes.has(image.sha256))
        classes.set(image.sha256, { name: `i${classes.size}`, image });
      return classes.get(image.sha256).name;
    },
    css() {
      return [...classes.values()]
        .map(({ name, image }) => `.${name}{background-image:url("${imageUrl(image)}")}`)
        .join('\n');
    },
  };
}

function renderCover(journal, logoUrl) {
  const { stats } = journal;
  const problems = stats.failed + stats.refused;
  const tiles = [
    ['Turns', stats.turns, ''],
    ['Duration', shortDuration(journal.durationMs) || '–', ''],
    ['Failed or refused', problems, problems ? 'bad' : ''],
    ['Unverified', stats.unverified, stats.unverified ? 'warn' : ''],
    ['In the background', stats.background, 'calm'],
  ];
  const facts = [
    journal.startedAt == null ? '' : formatDate(journal.startedAt),
    ...journal.apps,
    ...journal.windows.map((title) => `“${title}”`),
  ].filter(Boolean);
  const firstLook = journal.panels.find((panel) => panel.attention);
  return `<header class="cover">
<p class="cover-band">${logoUrl ? `<img src="${esc(logoUrl)}" alt="">` : ''}<span>Cua Driver</span><span class="cover-kicker">Session journal</span></p>
<h1>${esc(journal.title)}</h1>
${facts.length ? `<p class="cover-facts">${facts.map(esc).join(' · ')}</p>` : ''}
<dl class="cover-stats">
${tiles.map(([label, value, tone]) => `<div${tone ? ` class="${tone}"` : ''}><dt>${label}</dt><dd>${esc(value)}</dd></div>`).join('\n')}
</dl>
<p class="cover-bubble">${esc(headline(stats))}${firstLook ? ` <a href="#${firstLook.id}">First one: turn ${firstLook.index}</a>` : ''}</p>
<span class="cover-sfx" aria-hidden="true">ドン</span>
</header>`;
}

function headline(stats) {
  const problems = stats.failed + stats.refused;
  const parts = [];
  if (problems) parts.push(`${plural(problems, 'turn')} failed or got refused`);
  if (stats.unchanged) {
    parts.push(
      `${problems ? stats.unchanged : plural(stats.unchanged, 'turn')} sent input that left the screen unchanged`
    );
  }
  if (!parts.length) return 'No turn failed, got refused or left the screen unchanged.';
  return `${parts.join(' and ')}. They are marked in red.`;
}

function renderContents(journal) {
  const rows = journal.panels.map(
    (panel) =>
      `<li${panel.attention ? ' class="attention"' : ''}><a href="#${panel.id}"><span class="no">${panel.index}</span><time>${formatOffset(panel.startMs)}</time><span class="what">${esc(panel.title)}${panel.target ? ` <small>${esc(shorten(panel.target, 60))}</small>` : ''}</span><span class="mini mini-${esc(panel.outcome)}">${esc(outcomeLabel(panel.outcome))}</span></a></li>`
  );
  return `<nav class="contents" aria-labelledby="contents-title">
<h2 id="contents-title">Contents</h2>
<ol>
${rows.join('\n')}
</ol>
<p class="legend">${LEGEND}</p>
</nav>`;
}

function renderPanel(panel, sheet) {
  const classes = ['panel', `outcome-${panel.outcome}`, panel.attention ? 'attention' : '']
    .filter(Boolean)
    .join(' ');
  const refusalHint =
    panel.refusal?.hint && panel.refusal.hint !== panel.summary
      ? ` ${esc(panel.refusal.hint)}`
      : '';
  const body = [
    panel.target && `<p class="target">${esc(shorten(panel.target, 140))}</p>`,
    renderChips(panel),
    panel.summary && `<p class="bubble">${esc(shorten(firstParagraph(panel.summary), 240))}</p>`,
    panel.refusal && `<p class="refusal"><code>${esc(panel.refusal.code)}</code>${refusalHint}</p>`,
    panel.advice && `<p class="advice"><b>Driver:</b> ${esc(panel.advice)}</p>`,
    renderDetails(panel, sheet),
  ].filter(Boolean);
  return `<article class="${esc(classes)}" id="${panel.id}">
<header class="panel-head"><span class="panel-no">${panel.index}</span>${glyph(panel.kind)}<h3>${esc(panel.title)}</h3><time>${formatOffset(panel.startMs)}</time></header>
<div class="frame">${renderFrame(panel, sheet)}${renderStamps(panel)}${renderSfx(panel)}</div>
<div class="panel-body">
${body.join('\n')}
</div>
</article>`;
}

function renderFrame(panel, sheet) {
  const { frame } = panel;
  if (frame.hidden)
    return `<div class="shot-hidden"><p>${esc(HIDDEN_FRAMES[frame.hidden])}</p></div>`;
  const label = `${frame.caption}, turn ${panel.index}`;
  return `${shot(frame.image, sheet, frame.crop, label)}<span class="frame-caption">${esc(frame.caption)}</span>`;
}

function shot(image, sheet, crop, label) {
  const style = [
    `aspect-ratio:${crop?.width ?? image.width ?? 16}/${crop?.height ?? image.height ?? 10}`,
  ];
  if (crop) {
    const x = crop.x / (image.width - crop.width) || 0;
    const y = crop.y / (image.height - crop.height) || 0;
    style.push(
      `background-size:${percent(image.width / crop.width)} auto`,
      `background-position:${percent(x)} ${percent(y)}`
    );
  }
  return `<div class="shot ${sheet.use(image)}" role="img" aria-label="${esc(label)}" style="${style.join(';')}"></div>`;
}

function renderStamps(panel) {
  const label = esc(outcomeLabel(panel.outcome));
  const stamp =
    panel.outcome === 'failed' || panel.outcome === 'refused'
      ? `<span class="burst"><span>${label}</span></span>`
      : `<span class="stamp stamp-${esc(panel.outcome)}">${label}</span>`;
  const quiet = panel.screen === 'unchanged' && panel.outcome !== 'refused';
  return stamp + (quiet ? '<span class="stamp-note">No visible change</span>' : '');
}

function renderSfx(panel) {
  const text =
    panel.outcome === 'refused'
      ? 'ダメ'
      : panel.outcome === 'failed'
        ? 'ガーン'
        : panel.screen === 'unchanged'
          ? 'シーン'
          : SFX[panel.kind];
  return text ? `<span class="sfx" aria-hidden="true">${text}</span>` : '';
}

function renderChips(panel) {
  const chips = [];
  if (DELIVERY_LABELS[panel.delivery])
    chips.push([panel.delivery, DELIVERY_LABELS[panel.delivery]]);
  else if (!panel.delivery && panel.requestedDelivery) {
    chips.push(['requested', `${panel.requestedDelivery} requested`]);
  }
  if (panel.targeting) chips.push(['targeting', TARGETING_LABELS[panel.targeting]]);
  if (!chips.length) return '';
  return `<ul class="chips">${chips.map(([tone, text]) => `<li class="${esc(tone)}">${esc(text)}</li>`).join('')}</ul>`;
}

function renderDetails(panel, sheet) {
  const took =
    panel.startMs != null && panel.endMs != null ? formatDuration(panel.endMs - panel.startMs) : '';
  const facts = [
    ['Tool', panel.tool],
    ['Took', took],
    ['Window', panel.windowTitle],
    ['Route', panel.route],
    ['Transport', panel.transport],
  ].filter(([, value]) => value);
  const shots = panel.pair
    ? ['before', 'after']
        .filter((phase) => panel.pair[phase])
        .map(
          (phase) =>
            `<figure>${shot(panel.pair[phase], sheet, null, `${phase}, turn ${panel.index}`)}<figcaption>${phase}</figcaption></figure>`
        )
        .join('')
    : '';
  return `<details><summary>Details</summary>
<dl>${facts.map(([label, value]) => `<dt>${label}</dt><dd>${esc(value)}</dd>`).join('')}</dl>
${shots ? `<div class="pair">${shots}</div>` : ''}
<pre>${esc(JSON.stringify(panel.arguments, null, 2))}</pre>
${panel.summary ? `<p class="full">${esc(panel.summary)}</p>` : ''}
</details>`;
}

function renderColophon(journal) {
  const { privacy } = journal;
  const notes = [
    'Built offline by session-journal from a Cua Driver recording. This page loads nothing from the network.',
  ];
  if (privacy.typedHidden) {
    notes.push(
      `Typed text and values are hidden in ${plural(privacy.typedHidden, 'turn')}; rebuild with --show-typed-text to show them. Screenshots can still show what was typed.`
    );
  }
  if (privacy.desktopHidden) {
    notes.push(
      `${plural(privacy.desktopHidden, 'full-desktop screenshot')} hidden; rebuild with --include-desktop to show ${privacy.desktopHidden === 1 ? 'it' : 'them'}.`
    );
  }
  if (journal.skipped.length) {
    notes.push(`Skipped, no action.json yet: ${journal.skipped.join(', ')}.`);
  }
  return `<footer class="colophon">${notes.map((note) => `<p>${esc(note)}</p>`).join('\n')}</footer>`;
}

function glyph(kind) {
  return `<svg viewBox="0 0 24 24" aria-hidden="true"><path d="${GLYPHS[kind] ?? GLYPHS.tool}"/></svg>`;
}

function outcomeLabel(outcome) {
  return OUTCOME_LABELS[outcome] ?? String(outcome).replaceAll('_', ' ');
}

function formatOffset(ms) {
  if (ms == null) return '';
  const tenths = Math.round(ms / 100);
  const minutes = Math.floor(tenths / 600);
  const seconds = ((tenths % 600) / 10).toFixed(1).padStart(4, '0');
  return `${String(minutes).padStart(2, '0')}:${seconds}`;
}

function formatDuration(ms) {
  if (!Number.isFinite(ms) || ms < 0) return '';
  if (ms < 1000) return `${Math.round(ms)} ms`;
  const seconds = Math.round(ms / 1000);
  if (seconds < 60) return `${seconds} s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes} min ${seconds % 60} s`;
  return `${Math.floor(minutes / 60)} h ${minutes % 60} min`;
}

// Cover tiles are narrow: 1m 12s rather than 1 min 12 s.
function shortDuration(ms) {
  return formatDuration(ms)
    .replace(' h', 'h')
    .replace(' min', 'm')
    .replace(/ (m?s)$/, '$1');
}

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

function formatDate(ms) {
  const date = new Date(ms);
  const time = [date.getUTCHours(), date.getUTCMinutes()].map((n) => String(n).padStart(2, '0'));
  return `${date.getUTCDate()} ${MONTHS[date.getUTCMonth()]} ${date.getUTCFullYear()}, ${time.join(':')} UTC`;
}

function firstParagraph(text) {
  return text.split(/\n\s*\n/)[0];
}

function shorten(text, max) {
  const flat = String(text).replace(/\s+/g, ' ').trim();
  if (flat.length <= max) return flat;
  const cut = flat.slice(0, max - 1);
  return `${cut.slice(0, cut.lastIndexOf(' ') > max / 2 ? cut.lastIndexOf(' ') : cut.length)}…`;
}

function plural(count, word) {
  return `${count} ${word}${count === 1 ? '' : 's'}`;
}

function percent(value) {
  return `${(value * 100).toFixed(2)}%`;
}

function esc(value) {
  return String(value ?? '').replace(
    /[&<>"']/g,
    (char) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[char]
  );
}

const STYLES = `
:root {
  color-scheme: light;
  --ink: #141414;
  --paper: #fbfaf6;
  --cream: #ebe6d9;
  --muted: #6e6b65;
  --hair: #d9d6cf;
  --navy: #1a2e40;
  --blue: #5ec0e8;
  --accent: #e0765f;
  --ok: #2f7d4f;
  --warn: #a8600a;
  --bad: #c4402f;
  --display: 'Arial Black', 'Segoe UI Black', 'Helvetica Neue', Impact, system-ui, sans-serif;
  --mono: ui-monospace, 'Cascadia Mono', Consolas, Menlo, monospace;
  --sfx: 'Yu Gothic', 'Hiragino Sans', Meiryo, 'Noto Sans CJK JP', 'Noto Sans JP', sans-serif;
}
* { box-sizing: border-box; }
body {
  margin: 0;
  color: var(--ink);
  background: var(--cream) radial-gradient(rgb(20 20 20 / 7%) 1px, transparent 1.3px) 0 0 / 7px 7px;
  font: 15px/1.45 system-ui, -apple-system, 'Segoe UI', Roboto, sans-serif;
}
.journal { max-width: 1180px; margin: 0 auto; padding: 24px 16px 64px; }
h1, h2, h3 { margin: 0; font-family: var(--display); font-weight: 900; text-transform: uppercase; }
code, pre { font-family: var(--mono); font-size: 0.92em; }
a { color: inherit; }

.cover {
  position: relative;
  overflow: hidden;
  padding: 24px 28px 28px;
  color: #fff;
  background: var(--navy);
  border: 4px solid var(--ink);
  box-shadow: 8px 8px 0 var(--ink);
}
.cover::before {
  content: '';
  position: absolute;
  inset: 0;
  pointer-events: none;
  background: repeating-conic-gradient(from 0deg at 86% 72%, rgb(94 192 232 / 14%) 0deg 3deg, transparent 3deg 10deg);
}
.cover > * { position: relative; }
.cover-band {
  display: flex;
  align-items: center;
  gap: 10px;
  margin: 0;
  font: 900 14px/1 var(--display);
  letter-spacing: 0.08em;
  text-transform: uppercase;
}
.cover-band img { width: 32px; height: 32px; }
.cover-kicker {
  margin-left: auto;
  padding: 5px 10px;
  color: var(--navy);
  background: var(--blue);
  border: 2px solid var(--ink);
  transform: rotate(-2deg);
}
.cover h1 { margin: 22px 0 8px; font-size: clamp(34px, 7vw, 76px); line-height: 0.95; overflow-wrap: anywhere; }
.cover-facts { margin: 0; color: #cfe6f2; overflow-wrap: anywhere; }
.cover-stats {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(130px, 1fr));
  gap: 12px;
  margin: 24px 0 0;
}
.cover-stats div {
  padding: 10px 14px 12px;
  color: var(--ink);
  background: var(--paper);
  border: 3px solid var(--ink);
  box-shadow: 4px 4px 0 var(--ink);
}
.cover-stats dt { color: var(--muted); font-size: 12px; font-weight: 700; letter-spacing: 0.06em; text-transform: uppercase; }
.cover-stats dd { margin: 4px 0 0; font: 900 34px/1 var(--display); white-space: nowrap; }
.cover-stats .bad dd { color: var(--bad); }
.cover-stats .warn dd { color: var(--warn); }
.cover-stats .calm dd { color: #2a6f97; }
.cover-bubble {
  position: relative;
  max-width: 640px;
  margin: 26px 0 0;
  padding: 12px 16px;
  color: var(--ink);
  background: #fff;
  border: 3px solid var(--ink);
  border-radius: 22px;
  font-weight: 600;
}
.cover-bubble::before {
  content: '';
  position: absolute;
  top: -11px;
  left: 34px;
  width: 18px;
  height: 18px;
  background: #fff;
  border-top: 3px solid var(--ink);
  border-left: 3px solid var(--ink);
  transform: rotate(45deg);
}
.cover-bubble a { color: var(--bad); font-weight: 800; white-space: nowrap; }
.cover-sfx {
  position: absolute;
  right: 28px;
  bottom: 18px;
  color: var(--accent);
  font: 900 clamp(48px, 8vw, 88px)/1 var(--sfx);
  -webkit-text-stroke: 3px var(--ink);
  paint-order: stroke fill;
  transform: rotate(10deg);
  pointer-events: none;
}

.contents {
  margin-top: 28px;
  padding: 20px 24px;
  background: var(--paper);
  border: 3px solid var(--ink);
  box-shadow: 6px 6px 0 var(--ink);
}
.contents h2 { font-size: 22px; }
.contents ol { margin: 12px 0 0; padding: 0; list-style: none; columns: 2 420px; column-gap: 32px; }
.contents li { break-inside: avoid; border-bottom: 1px dashed var(--hair); }
.contents a {
  display: grid;
  grid-template-columns: 2.4em 4.4em 1fr auto;
  gap: 8px;
  align-items: baseline;
  padding: 6px 2px;
  text-decoration: none;
}
.contents a:hover { background: var(--cream); }
.contents .no { font: 900 14px/1 var(--display); }
.contents time { color: var(--muted); font-variant-numeric: tabular-nums; }
.contents .what { min-width: 0; overflow-wrap: anywhere; }
.contents small { color: var(--muted); }
.contents .attention .no { color: var(--bad); }
.mini {
  padding: 1px 6px;
  color: var(--muted);
  border: 2px solid currentColor;
  border-radius: 3px;
  font-size: 11px;
  font-weight: 800;
  letter-spacing: 0.05em;
  text-transform: uppercase;
  white-space: nowrap;
}
.mini-confirmed { color: var(--ok); }
.mini-done { color: var(--ink); }
.mini-unverifiable, .mini-partial, .mini-suspected_noop { color: var(--warn); }
.mini-failed, .mini-refused { color: var(--bad); }
.legend { margin: 14px 0 0; color: var(--muted); font-size: 13px; }

.panels {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
  gap: 20px;
  margin-top: 28px;
}
.panel {
  display: flex;
  flex-direction: column;
  min-width: 0;
  background: var(--paper);
  border: 3px solid var(--ink);
  box-shadow: 5px 5px 0 var(--ink);
}
.panel.attention { border-color: var(--bad); box-shadow: 5px 5px 0 var(--bad); }
.panel-head {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 10px;
  border-bottom: 3px solid var(--ink);
}
.attention .panel-head {
  color: #fff;
  background: var(--bad) radial-gradient(rgb(0 0 0 / 22%) 1px, transparent 1.4px) 0 0 / 5px 5px;
  border-bottom-color: var(--bad);
}
.panel-no { min-width: 28px; padding: 1px 6px; color: #fff; background: var(--ink); font: 900 14px/1.4 var(--display); text-align: center; }
.panel-head svg {
  flex: none;
  width: 20px;
  height: 20px;
  fill: none;
  stroke: currentColor;
  stroke-width: 2;
  stroke-linecap: round;
  stroke-linejoin: round;
}
.panel-head h3 { font-size: 17px; }
.panel-head time { margin-left: auto; font-size: 13px; font-variant-numeric: tabular-nums; opacity: 0.85; }
.frame { position: relative; overflow: hidden; background: #fff; border-bottom: 3px solid var(--ink); }
.attention .frame { border-bottom-color: var(--bad); }
.shot {
  width: 100%;
  background: #fff no-repeat 0 0 / 100% 100%;
  print-color-adjust: exact;
  -webkit-print-color-adjust: exact;
}
.shot-hidden {
  display: grid;
  place-items: center;
  aspect-ratio: 16 / 9;
  padding: 24px;
  color: var(--muted);
  background: repeating-linear-gradient(-45deg, #f1eee6 0 10px, #e7e2d6 10px 20px);
  font-weight: 600;
  text-align: center;
}
.frame-caption {
  position: absolute;
  top: 0;
  left: 0;
  padding: 2px 8px;
  color: #fff;
  background: var(--ink);
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.06em;
  text-transform: uppercase;
}
.stamp {
  position: absolute;
  top: 14px;
  right: 14px;
  padding: 3px 10px;
  color: var(--muted);
  background: rgb(255 255 255 / 90%);
  border: 4px double currentColor;
  border-radius: 4px;
  font: 900 15px/1.2 var(--display);
  text-transform: uppercase;
  transform: rotate(-7deg);
}
.stamp-confirmed { color: var(--ok); }
.stamp-done { color: var(--ink); }
.stamp-unverifiable, .stamp-partial, .stamp-suspected_noop { color: var(--warn); }
.burst {
  position: absolute;
  top: 2px;
  right: 2px;
  padding: 3px;
  background: var(--ink);
  clip-path: polygon(${BURST});
  transform: rotate(-8deg);
}
.burst span {
  display: block;
  padding: 26px 30px;
  color: #fff;
  background: var(--bad);
  clip-path: polygon(${BURST});
  font: 900 17px/1 var(--display);
  text-transform: uppercase;
}
.stamp-note {
  position: absolute;
  right: 12px;
  bottom: 12px;
  padding: 2px 8px;
  color: #fff;
  background: var(--warn);
  border: 2px solid var(--ink);
  font-size: 12px;
  font-weight: 800;
  letter-spacing: 0.04em;
  text-transform: uppercase;
  transform: rotate(2deg);
}
.sfx {
  position: absolute;
  bottom: 6px;
  left: 10px;
  color: #fff;
  font: 900 26px/1 var(--sfx);
  -webkit-text-stroke: 2px var(--ink);
  paint-order: stroke fill;
  transform: rotate(-8deg);
  pointer-events: none;
}
.attention .sfx { color: var(--accent); font-size: 34px; }
.panel-body { display: flex; flex-direction: column; gap: 8px; padding: 10px 12px 12px; }
.target { margin: 0; font-weight: 700; overflow-wrap: anywhere; }
.chips { display: flex; flex-wrap: wrap; gap: 6px; margin: 0; padding: 0; list-style: none; }
.chips li { padding: 1px 8px; border: 2px solid var(--ink); border-radius: 999px; font-size: 12px; font-weight: 700; }
.chips .background { background: #dff1fa; }
.chips .foreground { background: #fde7df; }
.chips .unknown, .chips .requested { background: var(--cream); }
.bubble {
  position: relative;
  margin: 6px 0 0;
  padding: 9px 12px;
  background: #fff;
  border: 2px solid var(--ink);
  border-radius: 16px;
  overflow-wrap: anywhere;
}
.bubble::before {
  content: '';
  position: absolute;
  top: -8px;
  left: 22px;
  width: 13px;
  height: 13px;
  background: #fff;
  border-top: 2px solid var(--ink);
  border-left: 2px solid var(--ink);
  transform: rotate(45deg);
}
.refusal { margin: 0; padding: 8px 10px; background: #fbe4df; border-left: 5px solid var(--bad); overflow-wrap: anywhere; }
.refusal code { color: var(--bad); font-weight: 700; }
.advice { margin: 0; padding: 8px 10px; background: var(--cream); border: 2px solid var(--ink); font-size: 14px; }
details { color: var(--muted); font-size: 13px; }
summary { color: var(--ink); font-weight: 700; cursor: pointer; }
details dl { display: grid; grid-template-columns: max-content 1fr; gap: 2px 12px; margin: 8px 0; }
details dt { font-weight: 700; }
details dd { margin: 0; overflow-wrap: anywhere; }
details pre {
  margin: 8px 0 0;
  padding: 8px;
  overflow-x: auto;
  background: #fff;
  border: 1px solid var(--hair);
  white-space: pre-wrap;
  word-break: break-word;
}
.pair { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; margin-top: 8px; }
.pair figure { margin: 0; }
.pair .shot { border: 1px solid var(--hair); }
.pair figcaption { font-size: 11px; text-transform: uppercase; }
.full { white-space: pre-wrap; overflow-wrap: anywhere; }
.colophon { margin-top: 32px; color: var(--muted); font-size: 13px; }

.panel { container-type: inline-size; }
@container (max-width: 440px) {
  .stamp { font-size: 13px; }
  .burst span { padding: 18px 22px; font-size: 14px; }
  .stamp-note { font-size: 11px; }
  .sfx, .attention .sfx { font-size: 22px; }
}
@media (max-width: 960px) {
  .cover-sfx { display: none; }
}
@media print {
  body { background: #fff; }
  .cover, .contents, .panel { box-shadow: none; break-inside: avoid; }
  details { display: none; }
}
`;
