const invoke = window.__TAURI__.core.invoke;
const listen = window.__TAURI__.event.listen;

const sessions = document.querySelector("#sessions");
const stage = document.querySelector("#stage");
const status = document.querySelector("#status span:last-child");
let lastRenderKey = "";

function escapeHtml(value) {
  const node = document.createElement("span");
  node.textContent = value;
  return node.innerHTML;
}

function render(snapshot) {
  if (!snapshot || !Array.isArray(snapshot.workspaces) || !Array.isArray(snapshot.frames)) {
    return;
  }
  const renderKey = JSON.stringify({
    selectedWorkspaceId: snapshot.selectedWorkspaceId,
    activeViewId: snapshot.activeViewId,
    workspaces: snapshot.workspaces.map((workspace) => [
      workspace.workspaceId,
      workspace.targetCount,
      workspace.updatedMs,
    ]),
    frames: snapshot.frames.map((frame) => [frame.viewId, frame.timestampMs]),
  });
  if (renderKey === lastRenderKey) return;
  lastRenderKey = renderKey;
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

  if (!snapshot.frames.length) {
    stage.innerHTML = `
      <div class="empty-state">
        <div class="empty-orbit"><span></span></div>
        <strong>Agent View</strong>
        <p>Used windows and tabs will appear here.</p>
      </div>`;
    status.textContent = "Waiting for agent activity";
    return;
  }

  stage.innerHTML = snapshot.frames
    .map((frame) => {
      const cursor = frame.cursorPosition
        ? `<i class="synthetic-cursor" style="left:${frame.cursorPosition.x * 100}%;top:${frame.cursorPosition.y * 100}%"></i>`
        : "";
      return `
        <article class="target-card ${frame.viewId === snapshot.activeViewId ? "active" : ""}">
          <img src="${frame.imageUrl}" alt="" draggable="false" />
          ${cursor}
          <div class="target-meta">
            <span class="target-kind">${frame.targetKind}</span>
            <span class="target-label">${escapeHtml(frame.targetLabel)}</span>
            <span>${escapeHtml(frame.actionLabel)}</span>
          </div>
        </article>`;
    })
    .join("");
  status.textContent = `${snapshot.frames.length} ${snapshot.frames.length === 1 ? "target" : "targets"} in this session`;
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

// Events are the fast path; periodic hydration also covers a WebView that
// attaches its listener after the first Upsert event has already fired.
listen("agent-view-state", (event) => render(event.payload)).then(hydrate).catch(hydrate);
setInterval(hydrate, 1500);
