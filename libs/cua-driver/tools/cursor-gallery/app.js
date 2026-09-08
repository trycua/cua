const actions = [
  ['idle', 'Idle', 'Waiting between actions'],
  ['observe', 'Observe', 'Reading the screen or interface'],
  ['click', 'Click', 'Clicking or selecting an element'],
  ['drag', 'Drag', 'Dragging an element or selection'],
  ['scroll', 'Scroll', 'Scrolling through content'],
  ['text', 'Text', 'Typing or filling text'],
  ['key', 'Key', 'Pressing a key or shortcut'],
  ['navigate', 'Navigate', 'Moving, navigating, or changing tabs'],
  ['app', 'App', 'Managing an application or window'],
  ['transfer', 'Transfer', 'Uploading, downloading, copying, or moving files'],
  ['record', 'Record', 'Recording or replaying a trajectory'],
  ['system', 'System', 'Managing sessions, permissions, or configuration'],
];

const deliveries = [
  ['none', 'None'],
  ['background', 'Background'],
  ['foreground', 'Foreground'],
];

const targets = [
  ['none', 'None'],
  ['ax', 'AX'],
  ['pixel', 'Pixel'],
  ['browser', 'Browser'],
  ['desktop', 'Desktop'],
];

const contexts = deliveries.flatMap(([delivery]) =>
  targets.map(([target]) => ({ delivery, target })),
);
const tones = ['light', 'dark', 'blue'];
let playing = true;
let backgroundMode = 'dark';

function labelFor(options, value) {
  return options.find(([id]) => id === value)?.[1] ?? value;
}

function previewState(action, delivery, target) {
  return `${action}--${delivery}--${target}`;
}

function previewPath(action, delivery, target) {
  return `./generated/previews/${previewState(action, delivery, target)}.webm`;
}

function contextLabel(delivery, target) {
  if (delivery === 'none' && target === 'none') return 'Session only';
  if (target === 'none') return `${labelFor(deliveries, delivery)} only`;
  if (delivery === 'none') return `${labelFor(targets, target)} only`;
  return `${labelFor(deliveries, delivery)} + ${labelFor(targets, target)}`;
}

function contextDescription(delivery, target) {
  if (delivery === 'none' && target === 'none') return 'No execution-context chips';
  if (target === 'none') return 'Filled delivery chip';
  if (delivery === 'none') return 'Outlined target chip';
  return 'Filled delivery · outlined target';
}

function actionCard([id, label, description], index) {
  const article = document.createElement('article');
  article.className = `state-card gallery-card card-${tones[index % tones.length]}`;
  article.tabIndex = 0;
  article.dataset.index = String(index);
  article.innerHTML = `
    <div class="demo">
      <video class="cursor-video" src="./generated/actions/${id}.webm"
        data-group="actions" data-state="${id}"
        autoplay loop muted playsinline aria-hidden="true"></video>
    </div>
    <div class="card-copy">
      <h3>${label}</h3>
      <p>${description}</p>
    </div>
  `;
  return article;
}

function contextCard({ delivery, target }, index) {
  const state = previewState('observe', delivery, target);
  const article = document.createElement('article');
  article.className = `state-card context-card gallery-card card-${tones[index % tones.length]}`;
  article.tabIndex = 0;
  article.dataset.index = String(index);
  article.innerHTML = `
    <div class="demo">
      <video class="cursor-video" src="${previewPath('observe', delivery, target)}"
        data-group="previews" data-state="${state}"
        autoplay loop muted playsinline aria-hidden="true"></video>
    </div>
    <div class="card-copy">
      <h3>${contextLabel(delivery, target)}</h3>
      <p>${contextDescription(delivery, target)}</p>
    </div>
  `;
  return article;
}

function videos() {
  return [...document.querySelectorAll('.cursor-video')];
}

function selectedSpeed() {
  return Number(document.querySelector('#speed').value);
}

function playAtCurrentSettings(video) {
  video.playbackRate = selectedSpeed();
  if (playing) void video.play().catch(() => {});
  else video.pause();
}

function updateRuntimePreview() {
  const action = document.querySelector('#preview-action').value;
  const delivery = document.querySelector('#preview-delivery').value;
  const target = document.querySelector('#preview-target').value;
  const actionLabel = labelFor(actions, action);
  const deliveryLabel = labelFor(deliveries, delivery);
  const targetLabel = labelFor(targets, target);
  const context = contextLabel(delivery, target);
  const state = previewState(action, delivery, target);
  const video = document.querySelector('#runtime-preview');
  const nextPath = previewPath(action, delivery, target);

  document.querySelector('#runtime-combination').textContent =
    `${actionLabel} · ${deliveryLabel} · ${targetLabel}`;
  document.querySelector('#runtime-preview-title').textContent = `${actionLabel} · ${context}`;
  document.querySelector('#anatomy-action').textContent = `${actionLabel} animation`;
  document.querySelector('#anatomy-delivery').textContent =
    delivery === 'none' ? 'No delivery chip' : `Filled ${deliveryLabel.toLowerCase()} chip`;
  document.querySelector('#anatomy-target').textContent =
    target === 'none' ? 'No target chip' : `Outlined ${targetLabel.toLowerCase()} chip`;
  video.dataset.state = state;
  video.setAttribute('aria-label', `${actionLabel} cursor with ${context.toLowerCase()}`);

  if (video.getAttribute('src') !== nextPath) {
    video.setAttribute('src', nextPath);
    video.load();
  }
  playAtCurrentSettings(video);
}

function updateCardTones() {
  document.querySelectorAll('.gallery-card').forEach((element) => {
    element.classList.remove('card-light', 'card-dark', 'card-blue');
    const index = Number(element.dataset.index);
    const tone = backgroundMode === 'mixed' ? tones[index % tones.length] : backgroundMode;
    element.classList.add(`card-${tone}`);
  });
}

function render() {
  const actionSelect = document.querySelector('#preview-action');
  actions.forEach(([id, label]) => {
    const option = document.createElement('option');
    option.value = id;
    option.textContent = label;
    option.selected = id === 'observe';
    actionSelect.append(option);
  });

  const contextGrid = document.querySelector('#contexts-grid');
  contexts.forEach((context, index) => contextGrid.append(contextCard(context, index)));

  const actionGrid = document.querySelector('#actions-grid');
  actions.forEach((state, index) => actionGrid.append(actionCard(state, index)));

  updateCardTones();
  updateRuntimePreview();
  document.documentElement.dataset.galleryVideoCount = String(videos().length);
}

document.querySelector('#play-toggle').addEventListener('click', (event) => {
  playing = !playing;
  event.currentTarget.textContent = playing ? 'Pause' : 'Play';
  videos().forEach(playAtCurrentSettings);
});

document.querySelector('#replay').addEventListener('click', () => {
  videos().forEach((video) => {
    video.currentTime = 0;
    playAtCurrentSettings(video);
  });
});

document.querySelector('#speed').addEventListener('change', () => {
  videos().forEach(playAtCurrentSettings);
});

document.querySelector('#background-toggle').addEventListener('click', (event) => {
  const options = ['dark', 'mixed', 'light', 'blue'];
  const labels = { dark: 'Dark', mixed: 'Mixed', light: 'Light', blue: 'Brand' };
  backgroundMode = options[(options.indexOf(backgroundMode) + 1) % options.length];
  event.currentTarget.textContent = labels[backgroundMode];
  updateCardTones();
});

document.querySelectorAll('.preview-controls select').forEach((select) => {
  select.addEventListener('change', updateRuntimePreview);
});

render();
