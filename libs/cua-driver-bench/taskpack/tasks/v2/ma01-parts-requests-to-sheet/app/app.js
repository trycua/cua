"use strict";
const PER_PAGE = 5;
const MONTHS = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
let state = { requests: [] };
let view = { status: "all", sort: "newest", page: 0, open: new Set() };
let pendingId = null;
const $ = (id) => document.getElementById(id);
const nice = (s) => { const [y, m, d] = s.split("-").map(Number); return `${d} ${MONTHS[m - 1]} ${y}`; };

function visible() {
  const rows = state.requests.filter((r) => view.status === "all" || r.status === view.status);
  rows.sort((a, b) => (a.submitted < b.submitted ? -1 : a.submitted > b.submitted ? 1 : a.id < b.id ? -1 : 1));
  if (view.sort === "newest") rows.reverse();
  return rows;
}

function detailList(r) {
  const items = [
    ["Requested quantity", String(r.requested_qty)],
    ["Approved quantity", r.approved_qty === null ? "–" : String(r.approved_qty)],
    ["Requested by", r.requester],
    ["Purchase log", r.status !== "Approved" ? "Not applicable" : r.logged ? "Logged" : "Not logged yet"],
  ];
  if (r.note) items.push(["Committee note", r.note]);
  const dl = v2.el("dl", { class: "details", id: `details-${r.id}` });
  items.forEach(([k, v]) => dl.append(v2.el("dt", { text: k }), v2.el("dd", { text: v })));
  return dl;
}

function render() {
  const rows = visible();
  const pages = Math.max(1, Math.ceil(rows.length / PER_PAGE));
  view.page = Math.min(view.page, pages - 1);
  const list = $("list");
  list.replaceChildren();
  rows.slice(view.page * PER_PAGE, view.page * PER_PAGE + PER_PAGE).forEach((r) => {
    const open = view.open.has(r.id);
    const toggle = v2.el("button", {
      type: "button", text: open ? "Hide details" : "Details", "aria-expanded": String(open),
      "aria-controls": `details-${r.id}`, "aria-label": `${open ? "Hide details" : "Details"} for ${r.id}`,
      onclick: () => { open ? view.open.delete(r.id) : view.open.add(r.id); render(); },
    });
    let action = v2.el("span");
    if (r.status === "Approved" && r.logged) action = v2.el("span", { class: "logged", text: "Logged" });
    else if (r.status === "Approved") {
      action = v2.el("button", { type: "button", class: "mark", "data-id": r.id, text: "Mark as logged",
        "aria-label": `Mark ${r.id} as logged`, onclick: () => ask(r.id) });
    }
    const row = v2.el("div", { class: "row" }, [
      v2.el("span", { class: "rid", text: r.id }),
      v2.el("span", { text: r.part }),
      v2.el("span", { class: `badge ${r.status}`, text: r.status }),
      v2.el("span", { class: "date", text: nice(r.submitted) }),
      toggle, action,
    ]);
    const li = v2.el("li", { "data-id": r.id }, [row]);
    if (open) li.append(detailList(r));
    list.append(li);
  });
  $("count").textContent = `${rows.length} request${rows.length === 1 ? "" : "s"}`;
  $("page-label").textContent = `Page ${view.page + 1} of ${pages}`;
  $("prev").disabled = view.page === 0;
  $("next").disabled = view.page >= pages - 1;
}

function ask(id) {
  pendingId = id;
  const r = state.requests.find((x) => x.id === id);
  $("dialog-text").textContent = `Mark ${id} (${r.part}) as logged in the purchase log?`;
  $("backdrop").hidden = false;
  $("dialog-ok").focus();
}
function closeDialog() { pendingId = null; $("backdrop").hidden = true; }

async function confirmLogged(ev) {
  const id = pendingId;
  closeDialog();
  const r = state.requests.find((x) => x.id === id);
  if (!r || r.status !== "Approved" || r.logged) return;
  r.logged = true;
  await v2.save(state);
  await v2.event("request_logged", { id, trusted: ev.isTrusted });
  $("flash").textContent = `${id} is marked as logged.`;
  $("flash").hidden = false;
  render();
}

async function init() {
  state = await v2.state();
  $("status-filter").addEventListener("change", (e) => { view.status = e.target.value; view.page = 0; render(); });
  $("sort").addEventListener("change", (e) => { view.sort = e.target.value; view.page = 0; render(); });
  $("prev").addEventListener("click", () => { view.page -= 1; render(); });
  $("next").addEventListener("click", () => { view.page += 1; render(); });
  $("dialog-cancel").addEventListener("click", closeDialog);
  $("dialog-ok").addEventListener("click", confirmLogged);
  document.addEventListener("keydown", (e) => { if (e.key === "Escape" && !$("backdrop").hidden) closeDialog(); });
  render();
}
init();
