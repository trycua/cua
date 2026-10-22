'use strict';
// Event identities never depend on display order. No captured HTML is executed.
const token = location.hash.slice(1);
history.replaceState(null, '', location.pathname);
const $ = (id) => document.getElementById(id);
let rows = [],
  total = 0,
  received = -1;
let cursor = -1,
  generation = null,
  selected = null,
  page = 0,
  paused = false,
  polling = false,
  follow = true,
  completed = false,
  previewURL = null,
  previewEvent = null,
  previewIndex = 0,
  previewEpoch = 0;
const pageSize = 100;
let relatedPage = 0,
  relatedEpoch = 0;
let pageEpoch = 0,
  selectionEpoch = 0,
  displayedCursor = -1,
  refreshRequested = false;
async function request(path) {
  const response = await fetch(path, {
    headers: { Authorization: `Bearer ${token}` },
    cache: 'no-store',
  });
  if (!response.ok) {
    const failure = await response.json();
    throw new Error(failure.error || `HTTP ${response.status}`);
  }
  return response;
}
function query(extra = {}) {
  return new URLSearchParams({ generation, through: cursor, order: $('order').value, ...extra });
}
async function refreshPage(keepSelection = false) {
  if (!generation) return;
  const epoch = ++pageEpoch;
  const response = await request(
    '/page?' + query({ page, ...(keepSelection && selected !== null ? { selected } : {}) })
  );
  const batch = await response.json();
  if (epoch !== pageEpoch) return;
  displayedCursor = batch.cursor;
  rows = batch.events;
  total = batch.total;
  page = batch.page;
  render();
  await refreshRelated();
}
function render() {
  const list = $('events');
  list.replaceChildren();
  for (const event of rows) {
    const li = document.createElement('li');
    const button = document.createElement('button');
    button.dataset.sequence = event.sequence;
    button.setAttribute('aria-current', String(event.sequence === selected));
    button.textContent = `${event.sequence} · ${event.event_name}`;
    const detail = document.createElement('small');
    detail.textContent =
      $('order').value === 'observed'
        ? event.context.aligned_at ||
          `Unaligned · ${event.context.clock_domain || 'unknown clock'} · arrival order`
        : event.recorded_at;
    button.append(detail);
    button.onclick = () =>
      run(async () => {
        const epoch = ++selectionEpoch;
        const response = await request('/event?' + query({ sequence: event.sequence }));
        const fullEvent = await response.json();
        if (epoch === selectionEpoch) select(fullEvent);
      });
    li.append(button);
    list.append(li);
  }
  count();
  $('page').textContent = `Page ${page + 1} of ${Math.max(1, Math.ceil(total / pageSize))}`;
  $('previous').disabled = page === 0;
  $('next').disabled = (page + 1) * pageSize >= total;
}
function count() {
  $('count').textContent =
    `${received + 1} events · ${Math.max(0, received - cursor)} pending display updates`;
}
async function run(action) {
  try {
    await action();
  } catch (error) {
    $('error').textContent = error.message;
  }
}
async function refreshRelated() {
  if (selected === null) return;
  const epoch = ++relatedEpoch;
  const response = await request('/related?' + query({ sequence: selected, page: relatedPage }));
  const batch = await response.json();
  if (epoch !== relatedEpoch) return;
  relatedPage = batch.page;
  $('related').replaceChildren();
  for (const event of batch.events) {
    const button = document.createElement('button');
    button.dataset.relatedSequence = event.sequence;
    button.textContent = `${event.sequence} · ${event.event_name}`;
    button.onclick = () =>
      run(async () => {
        const epoch = ++selectionEpoch;
        const response = await request('/event?' + query({ sequence: event.sequence }));
        const full = await response.json();
        if (epoch !== selectionEpoch) return;
        select(full);
        await refreshPage(true);
      });
    $('related').append(button);
  }
  $('relatedPage').textContent = `${batch.total} related · page ${batch.page + 1}`;
  $('relatedPrevious').disabled = batch.page === 0;
  $('relatedNext').disabled = (batch.page + 1) * 100 >= batch.total;
}
function select(event) {
  relatedEpoch++;
  relatedPage = 0;
  selected = event.sequence;
  $('selected').textContent = `${event.sequence} · ${event.event_name}`;
  $('context').textContent = JSON.stringify(
    { trajectory: event.trajectory_id, recorded_at: event.recorded_at, ...event.context },
    null,
    2
  );
  $('data').textContent = JSON.stringify(event.data, null, 2);
  $('related').replaceChildren();
  run(refreshRelated);
  $('attachments').replaceChildren();
  $('attachmentError').textContent = '';
  for (const ref of [...event.images, ...event.attachments]) {
    const row = document.createElement('div');
    row.className = 'attachment';
    const title = document.createElement('span');
    title.textContent = `${ref.name} · ${ref.size} bytes · ${ref.media_type}`;
    row.append(title);
    const isImage = ['image/png', 'image/jpeg', 'image/gif', 'image/webp'].includes(ref.media_type);
    const isText = ref.media_type === 'application/json' || ref.media_type === 'text/plain';
    if ((isImage && ref.size <= 8 * 1024 ** 2) || isText) {
      const preview = document.createElement('button');
      preview.textContent = 'Preview';
      preview.onclick = () => {
        previewEvent = event;
        previewIndex = event.images.findIndex((image) => image.id === ref.id);
        attachment(ref, false, isImage);
      };
      row.append(preview);
    }
    const download = document.createElement('button');
    download.textContent = 'Download';
    download.onclick = () => attachment(ref, true, false);
    row.append(download);
    $('attachments').append(row);
  }
  render();
}
async function attachment(ref, download, isImage) {
  const epoch = ++previewEpoch;
  try {
    $('attachmentError').textContent = '';
    if (download) {
      const response = await request('/download-ticket?' + new URLSearchParams({ id: ref.id }));
      const { url } = await response.json();
      const link = document.createElement('a');
      link.href = url;
      link.download = ref.name;
      link.click();
      return;
    }
    const response = await request(`/preview/${encodeURIComponent(ref.id)}`);
    const blob = await response.blob();
    if (epoch !== previewEpoch) return;
    if (previewURL) URL.revokeObjectURL(previewURL);
    $('imageNavigation').hidden = !isImage || previewIndex < 0;
    $('imageFrame').hidden = !isImage;
    $('previewTitle').textContent = ref.name;
    $('imagePreview').hidden = !isImage;
    $('textPreview').hidden = isImage;
    if (isImage) {
      previewURL = URL.createObjectURL(new Blob([blob], { type: ref.media_type }));
      $('imagePreview').src = previewURL;
      $('imagePreview').onload = drawOverlays;
    } else
      $('textPreview').textContent =
        (await blob.slice(0, 262144).text()) + (ref.size > 262144 ? '\n[Preview truncated]' : '');
    if (!$('preview').open) $('preview').showModal();
  } catch (error) {
    $('attachmentError').textContent = error.message;
  }
}
function drawOverlays() {
  const svg = $('overlays'),
    image = $('imagePreview');
  svg.replaceChildren();
  svg.setAttribute('viewBox', `0 0 ${image.naturalWidth} ${image.naturalHeight}`);
  if (!$('showOverlays').checked) return;
  for (const box of previewEvent?.snapshot_boxes || []) {
    const rect = document.createElementNS('http://www.w3.org/2000/svg', 'rect');
    for (const key of ['x', 'y', 'width', 'height']) rect.setAttribute(key, box[key]);
    const title = document.createElementNS('http://www.w3.org/2000/svg', 'title');
    title.textContent = box.label;
    rect.append(title);
    svg.append(rect);
  }
}
async function navigateImage(direction) {
  if (!previewEvent) return;
  let index = previewIndex + direction,
    event = previewEvent;
  if (index < 0 || index >= event.images.length) {
    const response = await request(
      '/image-neighbor?' + query({ sequence: event.sequence, direction })
    );
    event = await response.json();
    if (!event) return;
    index = direction > 0 ? 0 : event.images.length - 1;
  }
  previewEvent = event;
  previewIndex = index;
  select(event);
  await refreshPage(true);
  await attachment(event.images[index], false, true);
}
async function poll(force = false) {
  if (polling) {
    refreshRequested ||= force;
    return;
  }
  polling = true;
  try {
    const response = await request(
      '/status' + (generation ? '?' + new URLSearchParams({ generation }) : '')
    );
    const status = await response.json();
    if (generation && generation !== status.generation)
      throw new Error('Trace changed; reopen the viewer');
    generation = status.generation;
    received = status.cursor;
    follow = status.follow;
    if (!paused && (force || cursor !== received || total === 0)) {
      cursor = received;
      await refreshPage(true);
    }
    count();
    completed = status.state === 'completed' && received >= status.final_sequence;
    $('status').textContent = completed
      ? 'Completed'
      : status.state === 'recording'
        ? 'Following · writer not finalized'
        : status.state;
    $('error').textContent = '';
  } catch (error) {
    $('status').textContent = 'Disconnected / unavailable';
    $('error').textContent = error.message;
  } finally {
    polling = false;
    if (refreshRequested) {
      refreshRequested = false;
      poll(true);
    }
  }
}
$('relatedPrevious').onclick = () =>
  run(async () => {
    relatedPage--;
    await refreshRelated();
  });
$('relatedNext').onclick = () =>
  run(async () => {
    relatedPage++;
    await refreshRelated();
  });
$('order').onchange = () => run(() => refreshPage(true));
$('previous').onclick = () =>
  run(async () => {
    page--;
    await refreshPage();
  });
$('next').onclick = () =>
  run(async () => {
    page++;
    await refreshPage();
  });
$('pause').onclick = () => {
  paused = !paused;
  if (paused) {
    pageEpoch++;
    relatedEpoch++;
    cursor = displayedCursor;
    run(refreshRelated);
  }
  $('pause').textContent = paused ? 'Resume updates' : 'Pause updates';
  if (!paused) poll(true);
};
$('refresh').onclick = () => poll(true);
$('closePreview').onclick = () => $('preview').close();
$('showOverlays').onchange = drawOverlays;
$('previousImage').onclick = () => run(() => navigateImage(-1));
$('nextImage').onclick = () => run(() => navigateImage(1));
$('preview').addEventListener('keydown', (event) => {
  if (!$('imageNavigation').hidden && ['ArrowLeft', 'ArrowRight'].includes(event.key)) {
    event.preventDefault();
    run(() => navigateImage(event.key === 'ArrowRight' ? 1 : -1));
  }
});
$('preview').addEventListener('close', () => {
  previewEpoch++;
  if (previewURL) URL.revokeObjectURL(previewURL);
  previewURL = null;
  $('imagePreview').removeAttribute('src');
  $('textPreview').textContent = '';
  $('overlays').replaceChildren();
});
if (!token) {
  $('status').textContent = 'Authorization missing';
  $('error').textContent = 'Reopen the viewer from the CLI to obtain a fresh local session.';
} else {
  poll();
  setInterval(() => {
    if (follow && !completed) poll();
  }, 500);
}
