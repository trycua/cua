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

const modifiers = [
  ['background', 'Background', 'Delivering input without foreground focus'],
  ['foreground', 'Foreground', 'Delivering input to the active foreground window'],
  ['ax', 'AX', 'Using accessibility semantics'],
  ['pixel', 'Pixel', 'Using exact pixel coordinates'],
  ['browser', 'Browser', 'Using typed browser control'],
  ['desktop', 'Desktop', 'Using native desktop control'],
];

const tones = ['light', 'dark', 'blue'];
let playing = true;
let backgroundMode = 'mixed';

function card([id, label, description], index, group) {
  const article = document.createElement('article');
  article.className = `state-card card-${tones[index % tones.length]}`;
  article.tabIndex = 0;
  article.dataset.index = String(index);
  article.innerHTML = `
    <div class="demo">
      <video class="cursor-video" src="./generated/${group}/${id}.webm"
        data-group="${group}" data-state="${id}"
        autoplay loop muted playsinline aria-hidden="true"></video>
    </div>
    <div class="card-copy">
      <h3>${label}</h3>
      <p>${description}</p>
    </div>
  `;
  return article;
}

function render() {
  const actionGrid = document.querySelector('#actions-grid');
  const modifierGrid = document.querySelector('#modifiers-grid');
  actions.forEach((state, index) => actionGrid.append(card(state, index, 'actions')));
  modifiers.forEach((state, index) => modifierGrid.append(card(state, index, 'modifiers')));
}

function videos() {
  return [...document.querySelectorAll('.cursor-video')];
}

document.querySelector('#play-toggle').addEventListener('click', (event) => {
  playing = !playing;
  event.currentTarget.textContent = playing ? 'Pause' : 'Play';
  videos().forEach((video) => {
    if (playing) void video.play();
    else video.pause();
  });
});

document.querySelector('#replay').addEventListener('click', () => {
  videos().forEach((video) => {
    video.currentTime = 0;
    if (playing) void video.play();
  });
});

document.querySelector('#speed').addEventListener('change', (event) => {
  const speed = Number(event.currentTarget.value);
  videos().forEach((video) => {
    video.playbackRate = speed;
  });
});

document.querySelector('#background-toggle').addEventListener('click', (event) => {
  const options = ['mixed', 'light', 'dark', 'blue'];
  backgroundMode = options[(options.indexOf(backgroundMode) + 1) % options.length];
  event.currentTarget.textContent = backgroundMode[0].toUpperCase() + backgroundMode.slice(1);
  document.querySelectorAll('.state-card').forEach((element) => {
    element.classList.remove('card-light', 'card-dark', 'card-blue');
    const index = Number(element.dataset.index);
    const tone = backgroundMode === 'mixed' ? tones[index % tones.length] : backgroundMode;
    element.classList.add(`card-${tone}`);
  });
});

render();
