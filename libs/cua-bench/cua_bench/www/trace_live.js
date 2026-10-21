'use strict';
// Event identities never depend on display order. No captured HTML is executed.
const token = location.hash.slice(1);
history.replaceState(null, '', location.pathname);
const $ = (id) => document.getElementById(id);
const events = new Map();
let cursor = -1,
  generation = null,
  selected = null,
  page = 0,
  paused = false;
let pending = 0,
  polling = false,
  follow = true,
  completed = false,
  previewURL = null;
const pageSize = 100;
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
function ordered() {
  const result = Array.from(events.values());
  if ($('order').value === 'observed')
    result.sort((a, b) => {
      const at = Date.parse(a.context.aligned_at),
        bt = Date.parse(b.context.aligned_at);
      if (Number.isFinite(at) !== Number.isFinite(bt)) return Number.isFinite(at) ? -1 : 1;
      return (Number.isFinite(at) ? at - bt : 0) || a.sequence - b.sequence;
    });
  return result;
}
function render(keepSelection = false) {
  const rows = ordered();
  if (keepSelection && selected !== null)
    page = Math.floor(rows.findIndex((e) => e.sequence === selected) / pageSize);
  page = Math.max(0, Math.min(page, Math.ceil(rows.length / pageSize) - 1));
  const list = $('events');
  list.replaceChildren();
  for (const event of rows.slice(page * pageSize, (page + 1) * pageSize)) {
    const li = document.createElement('li');
    const button = document.createElement('button');
    button.dataset.sequence = event.sequence;
    button.setAttribute('aria-current', String(event.sequence === selected));
    button.textContent = `${event.sequence} · ${event.event_name}`;
    const detail = document.createElement('small');
    detail.textContent =
      $('order').value === 'observed'
        ? event.context.aligned_at || 'Unaligned · arrival order'
        : event.recorded_at;
    button.append(detail);
    button.onclick = () => select(event);
    li.append(button);
    list.append(li);
  }
  $('count').textContent = `${rows.length} events · ${pending} pending display updates`;
  $('page').textContent = `Page ${page + 1} of ${Math.max(1, Math.ceil(rows.length / pageSize))}`;
  $('previous').disabled = page === 0;
  $('next').disabled = (page + 1) * pageSize >= rows.length;
}
function select(event) {
  selected = event.sequence;
  $('selected').textContent = `${event.sequence} · ${event.event_name}`;
  $('context').textContent = JSON.stringify(
    { trajectory: event.trajectory_id, recorded_at: event.recorded_at, ...event.context },
    null,
    2
  );
  $('data').textContent = JSON.stringify(event.data, null, 2);
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
    if (isImage || isText) {
      const preview = document.createElement('button');
      preview.textContent = 'Preview';
      preview.onclick = () => attachment(ref, false, isImage);
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
  try {
    $('attachmentError').textContent = '';
    const response = await request(`/attachment/${encodeURIComponent(ref.id)}`);
    const blob = await response.blob();
    if (download) {
      const url = URL.createObjectURL(blob);
      const link = document.createElement('a');
      link.href = url;
      link.download = ref.name;
      link.click();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
      return;
    }
    if (previewURL) URL.revokeObjectURL(previewURL);
    $('previewTitle').textContent = ref.name;
    $('imagePreview').hidden = !isImage;
    $('textPreview').hidden = isImage;
    if (isImage) {
      previewURL = URL.createObjectURL(new Blob([blob], { type: ref.media_type }));
      $('imagePreview').src = previewURL;
    } else
      $('textPreview').textContent =
        (await blob.slice(0, 262144).text()) + (blob.size > 262144 ? '\n[Preview truncated]' : '');
    if (!$('preview').open) $('preview').showModal();
  } catch (error) {
    $('attachmentError').textContent = error.message;
  }
}
async function poll() {
  if (polling) return;
  polling = true;
  try {
    let batch;
    do {
      const response = await request(
        `/events?after=${cursor}` +
          (generation ? `&generation=${encodeURIComponent(generation)}` : '')
      );
      batch = await response.json();
      if (generation && generation !== batch.generation)
        throw new Error('Trace changed; reopen the viewer');
      generation = batch.generation;
      for (const event of batch.events) {
        if (!events.has(event.sequence)) pending++;
        events.set(event.sequence, event);
      }
      cursor = batch.cursor;
      follow = batch.follow;
      if (!paused) {
        pending = 0;
        render(true);
      } else $('count').textContent = `${events.size} events · ${pending} pending display updates`;
      completed = batch.state === 'completed' && cursor >= batch.final_sequence;
      $('status').textContent = completed
        ? 'Completed'
        : batch.state === 'recording'
          ? 'Following · writer not finalized'
          : batch.state;
      await new Promise((resolve) => setTimeout(resolve, 0));
    } while (batch.more);
    $('error').textContent = '';
  } catch (error) {
    $('status').textContent = 'Disconnected / unavailable';
    $('error').textContent = error.message;
  } finally {
    polling = false;
  }
}
$('order').onchange = () => render(true);
$('previous').onclick = () => {
  page--;
  render();
};
$('next').onclick = () => {
  page++;
  render();
};
$('pause').onclick = () => {
  paused = !paused;
  $('pause').textContent = paused ? 'Resume updates' : 'Pause updates';
  if (!paused) {
    pending = 0;
    render(true);
  }
};
$('refresh').onclick = poll;
$('closePreview').onclick = () => $('preview').close();
$('preview').addEventListener('close', () => {
  if (previewURL) URL.revokeObjectURL(previewURL);
  previewURL = null;
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
