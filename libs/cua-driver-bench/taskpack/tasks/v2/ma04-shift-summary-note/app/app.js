"use strict";
const PER_PAGE = 6;
const DAYS = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const ROLES = ["Delivery driver", "Front desk", "Packing", "Warehouse sorting"];
let state = { shifts: [] };
let view = { day: "all", role: "all", page: 0 };
let assigning = null;
const $ = (id) => document.getElementById(id);
const dayLabel = (s) => `${s.day} ${Number(s.date.slice(8))} October`;

function visible() {
  return state.shifts
    .filter((s) => (view.day === "all" || s.day === view.day) && (view.role === "all" || s.role === view.role))
    .sort((a, b) => a.role.localeCompare(b.role) || a.date.localeCompare(b.date) || a.start.localeCompare(b.start));
}

function volunteerCell(s) {
  if (s.cancelled) return v2.el("td", { class: "cancelled", text: `Cancelled: ${s.cancel_reason}` });
  if (!s.volunteer) return v2.el("td", { class: "needed", text: "Volunteer needed" });
  return v2.el("td", { text: s.volunteer });
}

function render() {
  const rows = visible();
  const pages = Math.max(1, Math.ceil(rows.length / PER_PAGE));
  view.page = Math.max(0, Math.min(view.page, pages - 1));
  const body = $("rows");
  body.replaceChildren();
  rows.slice(view.page * PER_PAGE, (view.page + 1) * PER_PAGE).forEach((s) => {
    const action = !s.cancelled && !s.volunteer
      ? v2.el("button", { type: "button", text: "Assign", "data-id": s.id, "aria-label": `Assign a volunteer to ${s.role} on ${s.day} ${s.start}`, onclick: () => ask(s.id) })
      : v2.el("span");
    body.append(v2.el("tr", { "data-id": s.id }, [
      v2.el("td", { text: s.role }), v2.el("td", { text: dayLabel(s) }),
      v2.el("td", { text: `${s.start}–${s.end}` }), volunteerCell(s), v2.el("td", {}, [action]),
    ]));
  });
  if (!rows.length) body.append(v2.el("tr", {}, [v2.el("td", { colspan: "5", text: "No shifts match these filters." })]));
  $("count").textContent = `${rows.length} shift${rows.length === 1 ? "" : "s"}`;
  $("page-label").textContent = `Page ${view.page + 1} of ${pages}`;
  $("prev").disabled = view.page === 0;
  $("next").disabled = view.page >= pages - 1;
}

function ask(id) {
  const s = state.shifts.find((x) => x.id === id);
  assigning = id;
  $("assign-what").textContent = `${s.role}, ${dayLabel(s)}, ${s.start}–${s.end}`;
  $("assign-name").value = "";
  $("assign-error").hidden = true;
  $("backdrop").hidden = false;
  $("assign-name").focus();
}
function closeDialog() { assigning = null; $("backdrop").hidden = true; }

async function assign(ev) {
  ev.preventDefault();
  const name = $("assign-name").value.replace(/\s+/g, " ").trim();
  if (!name) { $("assign-error").hidden = false; return; }
  const s = state.shifts.find((x) => x.id === assigning);
  closeDialog();
  if (!s) return;
  s.volunteer = name;
  await v2.save(state);
  await v2.event("shift_assigned", { id: s.id, volunteer: name, trusted: ev.isTrusted });
  $("flash").textContent = `${name} is now on ${s.role}, ${dayLabel(s)}.`;
  $("flash").hidden = false;
  render();
}

async function init() {
  state = await v2.state();
  DAYS.forEach((d) => $("day").append(v2.el("option", { value: d, text: d })));
  ROLES.forEach((r) => $("role").append(v2.el("option", { value: r, text: r })));
  $("day").addEventListener("change", (e) => { view.day = e.target.value; view.page = 0; render(); });
  $("role").addEventListener("change", (e) => { view.role = e.target.value; view.page = 0; render(); });
  $("prev").addEventListener("click", () => { view.page -= 1; render(); });
  $("next").addEventListener("click", () => { view.page += 1; render(); });
  $("assign-cancel").addEventListener("click", closeDialog);
  $("assign").addEventListener("submit", assign);
  document.addEventListener("keydown", (e) => { if (e.key === "Escape" && !$("backdrop").hidden) closeDialog(); });
  render();
}
init();
