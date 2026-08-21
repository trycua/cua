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
const devMode = document.documentElement.dataset.devServer === 'true';
let playing = true;
let backgroundMode = 'dark';
let workbenchPlaying = true;
let workbenchAction = 'observe';
let devStatus = null;

const fallbackActionFacts = {
  idle: { authored_frames: 1, still_frame: 0, playback: 'resting' },
  click: { authored_frames: 20, still_frame: 8, playback: 'one_shot' },
  observe: { authored_frames: 48, still_frame: 8, playback: 'loop' },
  drag: { authored_frames: 48, still_frame: 8, playback: 'held' },
  scroll: { authored_frames: 48, still_frame: 8, playback: 'loop' },
  text: { authored_frames: 48, still_frame: 8, playback: 'held' },
  key: { authored_frames: 48, still_frame: 8, playback: 'one_shot' },
  navigate: { authored_frames: 48, still_frame: 8, playback: 'one_shot' },
  app: { authored_frames: 48, still_frame: 8, playback: 'one_shot' },
  transfer: { authored_frames: 48, still_frame: 8, playback: 'loop' },
  record: { authored_frames: 48, still_frame: 8, playback: 'loop' },
  system: { authored_frames: 48, still_frame: 8, playback: 'one_shot' },
};

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

function buildForComparison() {
  return devStatus?.previous_build ?? null;
}

function sceneGroup(scene) {
  return scene === 'isolated' ? 'actions' : scene;
}

function mediaPath(build, action, scene) {
  if (build) return `${build.asset_root}/${sceneGroup(scene)}/${action}.webm`;
  if (scene === 'runtime') return previewPath(action, 'background', 'browser');
  return `./generated/actions/${action}.webm`;
}

function actionFacts(action) {
  const facts = devStatus?.build?.manifest?.actions?.find((item) => item.id === action);
  return facts ?? fallbackActionFacts[action];
}

function playbackLabel(value) {
  return {
    resting: 'Resting loop',
    loop: 'Loop',
    held: 'Held',
    one_shot: 'One shot',
  }[value] ?? value;
}

function setVideoSource(video, source) {
  if (!source) {
    video.removeAttribute('src');
    video.load();
    return;
  }
  if (video.getAttribute('src') === source) return;
  video.setAttribute('src', source);
  video.load();
}

function syncWorkbenchPlayback() {
  const current = document.querySelector('#workbench-video');
  const comparison = document.querySelector('#comparison-video');
  [current, comparison].forEach((video) => {
    video.playbackRate = selectedSpeed();
    if (workbenchPlaying && !document.querySelector('#reduced-motion').checked) {
      void video.play().catch(() => {});
    } else {
      video.pause();
    }
  });
  document.querySelector('#workbench-play').textContent = workbenchPlaying ? 'Pause' : 'Play';
}

function updateWorkbench() {
  const scene = document.querySelector('#workbench-scene').value;
  const currentBuild = devStatus?.build ?? null;
  const previousBuild = buildForComparison();
  const currentVideo = document.querySelector('#workbench-video');
  const comparisonVideo = document.querySelector('#comparison-video');
  const still = document.querySelector('#workbench-still');
  const reducedToggle = document.querySelector('#reduced-motion');
  const comparisonToggle = document.querySelector('#compare-build');
  const comparisonFrame = document.querySelector('#comparison-frame');
  const label = labelFor(actions, workbenchAction);
  const facts = actionFacts(workbenchAction);
  const sceneLabel = {
    isolated: 'Animation only',
    runtime: 'Runtime composition',
    movement: 'Production movement path',
  }[scene];

  document.querySelector('#workbench-stage').dataset.scene = scene;

  reducedToggle.disabled = !currentBuild;
  if (reducedToggle.disabled) reducedToggle.checked = false;
  const showStill = reducedToggle.checked;
  setVideoSource(
    currentVideo,
    devMode && !currentBuild ? null : mediaPath(currentBuild, workbenchAction, scene),
  );
  currentVideo.setAttribute('aria-label', `Current ${label} animation`);
  currentVideo.hidden = showStill;
  still.hidden = !showStill;
  if (showStill && currentBuild) {
    still.src = `${currentBuild.frame_root}/reduced/${sceneGroup(scene)}/${workbenchAction}.png`;
  }

  comparisonToggle.disabled = !previousBuild || showStill;
  if (comparisonToggle.disabled) comparisonToggle.checked = false;
  const comparing = comparisonToggle.checked && Boolean(previousBuild);
  comparisonFrame.hidden = !comparing;
  if (comparing) {
    setVideoSource(comparisonVideo, mediaPath(previousBuild, workbenchAction, scene));
    comparisonVideo.currentTime = currentVideo.currentTime;
  }

  document.querySelector('#inspector-action').textContent = label;
  document.querySelector('#inspector-playback').textContent = playbackLabel(facts.playback);
  document.querySelector('#inspector-frames').textContent = `${facts.authored_frames} authored`;
  document.querySelector('#inspector-still').textContent = `Frame ${facts.still_frame}`;
  document.querySelector('#inspector-scene').textContent = sceneLabel;
  const cadence = scene === 'movement'
    ? (currentBuild?.manifest?.movement_fps ?? 62.5)
    : (currentBuild?.manifest?.fps ?? 30);
  document.querySelector('#inspector-cadence').textContent = `${cadence} fps`;
  document.querySelector('#workbench-timeline').step = String(1 / cadence);
  document.querySelectorAll('.action-rail-button').forEach((button) => {
    button.setAttribute('aria-pressed', String(button.dataset.action === workbenchAction));
  });
  document.querySelector('#workbench-action').value = workbenchAction;
  syncWorkbenchPlayback();
}

function selectWorkbenchAction(action) {
  workbenchAction = action;
  document.querySelector('#workbench-timeline').value = '0';
  updateWorkbench();
}

function renderWorkbenchControls() {
  const select = document.querySelector('#workbench-action');
  const rail = document.querySelector('#action-rail-buttons');
  actions.forEach(([id, label]) => {
    const option = document.createElement('option');
    option.value = id;
    option.textContent = label;
    select.append(option);

    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'action-rail-button';
    button.dataset.action = id;
    button.textContent = label;
    button.setAttribute('aria-pressed', String(id === workbenchAction));
    button.addEventListener('click', () => selectWorkbenchAction(id));
    rail.append(button);
  });
}

function applyDevStatus(status) {
  devStatus = status;
  const statusElement = document.querySelector('#build-status');
  const detail = document.querySelector('#build-detail');
  const diagnostics = document.querySelector('#build-diagnostics');
  const priorBuildId = document.documentElement.dataset.devBuild;
  const nextBuildId = status.build?.id ?? '';

  document.documentElement.dataset.devState = status.state;
  document.documentElement.dataset.devBuild = nextBuildId;
  statusElement.className = `build-status status-${status.state}`;
  statusElement.textContent = {
    starting: 'Starting',
    building: 'Building',
    ready: 'Renderer current',
    error: 'Build failed',
  }[status.state] ?? status.state;
  detail.textContent = status.build
    ? `${status.build.manifest.theme_name} · ${status.build.manifest.content_hash.slice(0, 10)} · ${status.message}`
    : status.message;
  diagnostics.textContent = status.error || 'No build errors.';
  if (nextBuildId !== priorBuildId || status.state === 'error') updateWorkbench();
}

async function pollDevStatus() {
  try {
    const response = await fetch('/__cursor_dev/status.json', { cache: 'no-store' });
    if (!response.ok) return;
    applyDevStatus(await response.json());
  } catch (_) {
    // The ordinary static gallery intentionally has no development endpoint.
  }
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
  renderWorkbenchControls();
  if (devMode) {
    document.querySelector('#runtime-preview-section').hidden = true;
    document.querySelector('#badge-contexts').hidden = true;
    document.querySelector('[data-capture-group="actions"]').hidden = true;
    updateWorkbench();
    document.documentElement.dataset.galleryVideoCount = '0';
    return;
  }

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
  updateWorkbench();
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
  syncWorkbenchPlayback();
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

document.querySelector('#workbench-action').addEventListener('change', (event) => {
  selectWorkbenchAction(event.currentTarget.value);
});

document.querySelector('#workbench-scene').addEventListener('change', updateWorkbench);
document.querySelector('#reduced-motion').addEventListener('change', updateWorkbench);
document.querySelector('#compare-build').addEventListener('change', updateWorkbench);

document.querySelector('#workbench-background').addEventListener('change', (event) => {
  const stage = document.querySelector('#workbench-stage');
  stage.className = `workbench-stage stage-${event.currentTarget.value}`;
});

document.querySelector('#workbench-play').addEventListener('click', () => {
  workbenchPlaying = !workbenchPlaying;
  syncWorkbenchPlayback();
});

document.querySelector('#workbench-replay').addEventListener('click', () => {
  [document.querySelector('#workbench-video'), document.querySelector('#comparison-video')]
    .forEach((video) => { video.currentTime = 0; });
  workbenchPlaying = true;
  syncWorkbenchPlayback();
});

document.querySelector('#workbench-timeline').addEventListener('input', (event) => {
  const time = Number(event.currentTarget.value);
  workbenchPlaying = false;
  [document.querySelector('#workbench-video'), document.querySelector('#comparison-video')]
    .forEach((video) => { video.currentTime = time; });
  syncWorkbenchPlayback();
});

document.querySelector('#workbench-video').addEventListener('timeupdate', (event) => {
  const duration = Number.isFinite(event.currentTarget.duration) ? event.currentTarget.duration : 4;
  const time = event.currentTarget.currentTime;
  const timeline = document.querySelector('#workbench-timeline');
  timeline.max = String(duration);
  if (workbenchPlaying) timeline.value = String(time);
  document.querySelector('#workbench-time').textContent = `${time.toFixed(2)} / ${duration.toFixed(2)}s`;
  const comparison = document.querySelector('#comparison-video');
  const comparisonFrame = document.querySelector('#comparison-frame');
  if (!comparisonFrame.hidden && Math.abs(comparison.currentTime - time) > 0.08) {
    comparison.currentTime = time;
  }
});

document.querySelector('#actions-grid').addEventListener('click', (event) => {
  const card = event.target.closest('.state-card');
  if (card) selectWorkbenchAction(actions[Number(card.dataset.index)][0]);
});

render();
if (devMode) {
  void pollDevStatus();
  setInterval(() => { void pollDevStatus(); }, 900);
}
