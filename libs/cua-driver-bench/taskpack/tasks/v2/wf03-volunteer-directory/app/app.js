"use strict";
const TEAMS = [
  ["riverbank", "Riverbank Clean-up"],
  ["pantry", "Food Pantry"],
  ["reading", "Reading Buddies"],
  ["tools", "Tool Library"],
  ["events", "Events Crew"],
];
const PER_PAGE = 15;
const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
let state = { volunteers: [] };
let page = 1;
let openMenu = null;
let target = null;
const $ = (id) => document.getElementById(id);
const teamName = (id) => (TEAMS.find((t) => t[0] === id) || [id, id])[1];
const shortDate = (s) => { const [y, m, d] = s.split("-").map(Number); return `${d} ${MONTHS[m - 1]} ${y}`; };

function filtered() {
  const q = $("search").value.trim().toLowerCase();
  const team = $("team-filter").value;
  return state.volunteers.filter((v) =>
    (!q || v.name.toLowerCase().includes(q) || v.email.toLowerCase().includes(q)) && (!team || v.team === team));
}

function closeMenu() {
  if (!openMenu) return;
  openMenu.menu.remove();
  openMenu.button.setAttribute("aria-expanded", "false");
  openMenu = null;
}

function showMenu(button, v) {
  const again = openMenu && openMenu.button === button;
  closeMenu();
  if (again) return;
  const item = (text, fn) => v2.el("button", { type: "button", role: "menuitem", text, onclick: () => { closeMenu(); fn(); } });
  const menu = v2.el("div", { role: "menu", "aria-label": `Actions for ${v.name}` }, [
    item("Change team…", () => openTeam(v)),
    v.status === "active" ? item("Deactivate", () => openDeactivate(v)) : item("Reactivate", () => reactivate(v)),
  ]);
  button.parentElement.append(menu);
  button.setAttribute("aria-expanded", "true");
  openMenu = { button, menu };
  menu.querySelector("[role=menuitem]").focus();
}

function render() {
  closeMenu();
  const list = filtered();
  const pages = Math.max(1, Math.ceil(list.length / PER_PAGE));
  page = Math.min(Math.max(1, page), pages);
  const rows = $("rows");
  rows.replaceChildren();
  list.slice((page - 1) * PER_PAGE, page * PER_PAGE).forEach((v) => {
    const btn = v2.el("button", { type: "button", class: "menu-btn", text: "⋯", "aria-label": `Actions for ${v.name}`, "aria-haspopup": "menu", "aria-expanded": "false" });
    btn.addEventListener("click", (e) => { e.stopPropagation(); showMenu(btn, v); });
    rows.append(v2.el("tr", { class: v.status === "active" ? "" : "off", "data-id": v.id }, [
      v2.el("td", { text: v.name }),
      v2.el("td", { text: v.email }),
      v2.el("td", { text: teamName(v.team) }),
      v2.el("td", { class: v.status, text: v.status === "active" ? "Active" : "Inactive" }),
      v2.el("td", { text: shortDate(v.joined) }),
      v2.el("td", { class: "cell-actions" }, [btn]),
    ]));
  });
  if (!list.length) rows.append(v2.el("tr", {}, [v2.el("td", { colspan: "6", text: "No volunteers match." })]));
  $("count").textContent = `${list.length} volunteer${list.length === 1 ? "" : "s"}`;
  const nums = $("pages");
  nums.replaceChildren();
  for (let p = 1; p <= pages; p++) {
    nums.append(v2.el("button", { type: "button", text: String(p), "aria-label": `Page ${p}`, "aria-current": p === page ? "page" : false, onclick: () => { page = p; render(); } }));
  }
  $("prev").disabled = page === 1;
  $("next").disabled = page === pages;
  $("page-label").textContent = `Page ${page} of ${pages}`;
}

function showDialog(id) {
  $("backdrop").hidden = false;
  $(id).hidden = false;
}
function hideDialogs() {
  $("backdrop").hidden = true;
  $("team-dialog").hidden = true;
  $("deact-dialog").hidden = true;
  target = null;
}

function openTeam(v) {
  target = v;
  $("team-who").textContent = `${v.name} (${v.email}) is in ${teamName(v.team)}.`;
  $("team-select").value = v.team;
  $("team-error").hidden = true;
  showDialog("team-dialog");
  $("team-select").focus();
}

async function saveTeam() {
  const v = target;
  const to = $("team-select").value;
  if (!v) return;
  if (to === v.team) {
    $("team-error").textContent = "Choose a different team, or Cancel.";
    $("team-error").hidden = false;
    return;
  }
  const from = v.team;
  v.team = to;
  hideDialogs();
  await v2.save(state);
  await v2.event("team_changed", { id: v.id, from, to });
  $("notice").textContent = `${v.name} moved to ${teamName(to)}.`;
  render();
}

function openDeactivate(v) {
  target = v;
  $("deact-who").textContent = `Deactivate ${v.name} (${v.email})?`;
  showDialog("deact-dialog");
  $("deact-ok").focus();
}

async function deactivate() {
  const v = target;
  if (!v) return;
  v.status = "inactive";
  hideDialogs();
  await v2.save(state);
  await v2.event("volunteer_deactivated", { id: v.id });
  $("notice").textContent = `${v.name} deactivated.`;
  render();
}

async function reactivate(v) {
  v.status = "active";
  await v2.save(state);
  await v2.event("volunteer_reactivated", { id: v.id });
  $("notice").textContent = `${v.name} reactivated.`;
  render();
}

async function init() {
  state = await v2.state();
  $("team-filter").append(v2.el("option", { value: "", text: "All teams" }), ...TEAMS.map(([v, t]) => v2.el("option", { value: v, text: t })));
  $("team-select").append(...TEAMS.map(([v, t]) => v2.el("option", { value: v, text: t })));
  $("search").addEventListener("input", () => { page = 1; render(); });
  $("team-filter").addEventListener("change", () => { page = 1; render(); });
  $("prev").addEventListener("click", () => { page -= 1; render(); });
  $("next").addEventListener("click", () => { page += 1; render(); });
  $("team-cancel").addEventListener("click", hideDialogs);
  $("team-save").addEventListener("click", saveTeam);
  $("deact-cancel").addEventListener("click", hideDialogs);
  $("deact-ok").addEventListener("click", deactivate);
  document.addEventListener("click", (e) => { if (openMenu && !openMenu.menu.contains(e.target)) closeMenu(); });
  document.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    if (openMenu) { const b = openMenu.button; closeMenu(); b.focus(); } else if (target) hideDialogs();
  });
  render();
}
init();
