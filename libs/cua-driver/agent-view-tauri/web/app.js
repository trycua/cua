const invoke = window.__TAURI__.core.invoke;
const listen = window.__TAURI__.event.listen;

const sessions = document.querySelector("#sessions");
const stage = document.querySelector("#stage");
const empty = document.querySelector("#empty");
const shell = document.querySelector(".shell");
const status = document.querySelector("#status span:last-child");

// Cards are reconciled in place by viewId. Rebuilding the stage with innerHTML
// re-ran the entry animation and re-decoded every image on each snapshot, which
// briefly exposed the host desktop through the translucent shell.
const cards = new Map();
let lastSessionsKey = "";
let latestSnapshot = null;

function escapeHtml(value) {
  const node = document.createElement("span");
  node.textContent = value;
  return node.innerHTML;
}

function renderSessions(snapshot) {
  const key = JSON.stringify([
    snapshot.selectedWorkspaceId,
    snapshot.workspaces.map((w) => [w.workspaceId, w.label, w.targetCount]),
  ]);
  if (key === lastSessionsKey) return;
  lastSessionsKey = key;
  const showTabs = snapshot.workspaces.length > 1;
  sessions.classList.toggle("visible", showTabs);
  sessions.innerHTML = snapshot.workspaces
    .slice(0, 6)
    .map(
      (workspace) => `
        <button
          class="session-tab ${workspace.workspaceId === snapshot.selectedWorkspaceId ? "selected" : ""}"
          data-workspace-id="${escapeHtml(workspace.workspaceId)}"
          title="${escapeHtml(workspace.label)}"
        >${escapeHtml(workspace.label)} · ${workspace.targetCount}</button>`,
    )
    .join("");
}

function createCard() {
  const card = document.createElement("article");
  card.className = "target-card";
  card.innerHTML = `
    <div class="target-slot">
      <img alt="" draggable="false" />
      <i class="synthetic-cursor" hidden></i>
    </div>
    <div class="target-meta">
      <span class="target-kind"></span>
      <span class="target-label"></span>
      <span class="target-action"></span>
    </div>`;
  return {
    el: card,
    img: card.querySelector("img"),
    cursor: card.querySelector(".synthetic-cursor"),
    kind: card.querySelector(".target-kind"),
    label: card.querySelector(".target-label"),
    action: card.querySelector(".target-action"),
    timestampMs: -1,
    pendingUrl: null,
    ar: null,
    ready: false,
  };
}

// Decode off-screen and swap only when ready so the visible image never goes
// blank between frames.
function swapImage(card, url) {
  if (card.img.src === url || card.pendingUrl === url) return;
  card.pendingUrl = url;
  const next = new Image();
  next.src = url;
  const commit = () => {
    if (card.pendingUrl !== url) return;
    card.pendingUrl = null;
    const wasReady = card.ready;
    // Size the preview like a window: keep the screenshot's own aspect ratio,
    // clamped so extreme shapes still read as a card.
    if (next.naturalWidth && next.naturalHeight) {
      const ratio = Math.min(2.2, Math.max(0.6, next.naturalWidth / next.naturalHeight));
      const ar = ratio.toFixed(3);
      if (card.ar !== ar) {
        card.ar = ar;
        card.el.style.setProperty("--ar", ar);
      }
    }
    card.img.src = url;
    card.ready = true;
    // A new card stays outside the layout until its first decoded image and
    // aspect ratio are ready, preventing a blank 4:3 placeholder flash.
    if (!wasReady && latestSnapshot) render(latestSnapshot);
  };
  (next.decode ? next.decode() : Promise.resolve()).then(commit, commit);
}

function updateCard(card, frame, isActive) {
  card.el.dataset.kind = frame.targetKind;
  card.el.classList.toggle("active", isActive);
  if (card.kind.textContent !== frame.targetKind) card.kind.textContent = frame.targetKind;
  if (card.label.textContent !== frame.targetLabel) {
    card.label.textContent = frame.targetLabel;
    // The caption is visually hidden; keep the target name on the card itself.
    card.el.setAttribute("aria-label", frame.targetLabel);
  }
  if (card.action.textContent !== frame.actionLabel) card.action.textContent = frame.actionLabel;
  if (frame.cursorPosition) {
    card.cursor.hidden = false;
    card.cursor.style.left = `${frame.cursorPosition.x * 100}%`;
    card.cursor.style.top = `${frame.cursorPosition.y * 100}%`;
  } else {
    card.cursor.hidden = true;
  }
  if (frame.timestampMs !== card.timestampMs) {
    card.timestampMs = frame.timestampMs;
    swapImage(card, frame.imageUrl);
  }
}

function render(snapshot) {
  if (!snapshot || !Array.isArray(snapshot.workspaces) || !Array.isArray(snapshot.frames)) {
    return;
  }
  latestSnapshot = snapshot;
  renderSessions(snapshot);

  const frames = snapshot.frames;
  const seen = new Set();
  let cursorNode = empty.nextSibling;

  frames.forEach((frame) => {
    seen.add(frame.viewId);
    let card = cards.get(frame.viewId);
    if (!card) {
      card = createCard();
      cards.set(frame.viewId, card);
    }
    updateCard(card, frame, frame.viewId === snapshot.activeViewId);
    if (!card.ready) return;
    // Keep DOM order equal to snapshot order without detaching unchanged nodes.
    if (card.el !== cursorNode) {
      stage.insertBefore(card.el, cursorNode);
    } else {
      cursorNode = cursorNode.nextSibling;
    }
  });

  cards.forEach((card, viewId) => {
    if (!seen.has(viewId)) {
      card.pendingUrl = null;
      card.el.remove();
      cards.delete(viewId);
    }
  });

  const visibleCount = frames.reduce(
    (count, frame) => count + (cards.get(frame.viewId)?.ready ? 1 : 0),
    0,
  );
  empty.hidden = visibleCount > 0;
  // A populated shell rests quietly: the status line is screen-reader only.
  shell.classList.toggle("populated", visibleCount > 0);
  const countClass =
    visibleCount === 1
      ? "count-1"
      : visibleCount === 2
        ? "count-2"
        : visibleCount <= 4
          ? "count-few"
          : "count-many";
  if (!stage.classList.contains(countClass)) {
    stage.classList.remove("count-1", "count-2", "count-few", "count-many");
    stage.classList.add(countClass);
  }
  status.textContent = visibleCount
    ? `${visibleCount} ${visibleCount === 1 ? "target" : "targets"} in this session`
    : "Waiting for agent activity";
}

sessions.addEventListener("click", (event) => {
  const button = event.target.closest("[data-workspace-id]");
  if (button) {
    invoke("select_workspace", { workspaceId: button.dataset.workspaceId });
  }
});

document.querySelectorAll("[data-resize]").forEach((handle) => {
  handle.addEventListener("pointerdown", (event) => {
    event.preventDefault();
    invoke("begin_resize", { direction: handle.dataset.resize });
  });
});

function hydrate() {
  invoke("get_snapshot").then(render).catch(() => {});
}

async function prepareShell() {
  document.documentElement.dataset.wallpaper = "loading";
  try {
    const platform = await invoke("get_platform");
    document.documentElement.dataset.platform = platform;
    let wallpaper = null;
    for (let attempt = 0; attempt < 40 && !wallpaper; attempt += 1) {
      wallpaper = await invoke("get_wallpaper");
      if (!wallpaper) await new Promise((resolve) => setTimeout(resolve, 100));
    }
    if (wallpaper) {
      const image = new Image();
      image.src = wallpaper;
      await new Promise((resolve) => {
        image.onload = resolve;
        image.onerror = resolve;
      });
      if (image.naturalWidth && image.naturalHeight) {
        shell.style.setProperty("--wallpaper-image", `url("${wallpaper}")`);
        document.documentElement.dataset.wallpaper = "ready";
      }
    }
  } finally {
    if (document.documentElement.dataset.wallpaper !== "ready") {
      document.documentElement.dataset.wallpaper = "none";
    }
    invoke("show_agent_view").catch(() => {});
  }
}

// Events are the fast path; periodic hydration also covers a WebView that
// attaches its listener after the first Upsert event has already fired.
listen("agent-view-state", (event) => render(event.payload)).then(hydrate).catch(hydrate);
setInterval(hydrate, 1500);
prepareShell();
