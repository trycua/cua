"use strict";
// Members' area of the Coppergate Cycling Club: two pages (profile, preferences), each with its own form.
const PAGE = document.body.dataset.page;
const PACES = [
  ["social", "Social (under 20 km/h)"], ["steady", "Steady (22 to 25 km/h)"],
  ["brisk", "Brisk (26 to 29 km/h)"], ["race", "Race pace (30 km/h and over)"],
];
const GROUPS = [
  ["thursday-evening", "Thursday evening spin"], ["saturday-cafe", "Saturday café ride"],
  ["sunday-hills", "Sunday hill loop"], ["gravel", "Gravel explorers"],
];
const PROFILE_FIELDS = ["display_name", "phone", "bike", "emergency_name", "emergency_relation", "emergency_phone"];
const $ = (id) => document.getElementById(id);
let saved = null;
let groups = [];
let guarded = false;

function values() {
  if (PAGE === "profile") {
    const out = {};
    PROFILE_FIELDS.forEach((k) => { out[k] = $(k).value; });
    return out;
  }
  return {
    pace: (document.querySelector('input[name="pace"]:checked') || {}).value || "",
    groups: groups.slice().sort(),
    newsletter: $("newsletter").value,
    leaderboard: $("leaderboard").checked,
  };
}
function changedKeys() {
  const now = values();
  return Object.keys(now).filter((k) => JSON.stringify(now[k]) !== JSON.stringify(saved[k]));
}

// Unsaved edits: ask the browser to confirm before the page is left, and report edits that were lost.
function guard(ev) {
  // Chrome shows the leave dialog only after a user gesture on the page; log whether one happened.
  const activated = navigator.userActivation ? navigator.userActivation.hasBeenActive : null;
  v2.beacon("leave_guard", { page: PAGE, changed: changedKeys(), activated });
  ev.preventDefault();
  ev.returnValue = "";
  return "";
}
function updateGuard() {
  const dirty = saved !== null && changedKeys().length > 0;
  if (dirty && !guarded) window.addEventListener("beforeunload", guard);
  if (!dirty && guarded) window.removeEventListener("beforeunload", guard);
  guarded = dirty;
  if (dirty) $("status").textContent = "Unsaved changes";
  else if ($("status").textContent === "Unsaved changes") $("status").textContent = "";
}
window.addEventListener("pagehide", () => {
  if (saved === null) return;
  const changed = changedKeys();
  if (changed.length) v2.beacon("left_unsaved", { page: PAGE, changed });
});

function renderProfile(p) {
  $("member-no").textContent = p.member_no;
  PROFILE_FIELDS.forEach((k) => { $(k).value = p[k]; });
  $("ec-summary").textContent = `${p.emergency_name} (${p.emergency_relation}), ${p.emergency_phone}`;
  $("ec-toggle").addEventListener("click", () => {
    const box = $("ec-box");
    box.hidden = !box.hidden;
    $("ec-toggle").setAttribute("aria-expanded", String(!box.hidden));
  });
}
function renderGroups() {
  const box = $("groups");
  box.replaceChildren();
  GROUPS.forEach(([id, name]) => {
    const on = groups.includes(id);
    box.append(v2.el("button", {
      type: "button", role: "switch", "aria-checked": String(on), "aria-label": name, text: name,
      onclick: () => { groups = on ? groups.filter((g) => g !== id) : groups.concat(id); renderGroups(); updateGuard(); },
    }));
  });
}
function renderPreferences(p) {
  PACES.forEach(([id, name]) => $("paces").append(
    v2.el("label", {}, [v2.el("input", { type: "radio", name: "pace", value: id, checked: p.pace === id }), name])));
  groups = p.groups.slice();
  renderGroups();
  $("newsletter").value = p.newsletter;
  $("leaderboard").checked = p.leaderboard;
}

const PHONE = /^0\d{4} ?\d{6}$/;
function validate(v) {
  if (PAGE === "profile") {
    if (!v.display_name.trim()) return "Enter a display name.";
    if (!PHONE.test(v.phone.trim())) return "Mobile numbers have 11 digits and start with 0, like 07700 900000.";
    if (v.emergency_name.trim().split(/\s+/).length < 2) return "Enter your emergency contact's full name.";
    if (!v.emergency_relation.trim()) return "Say how your emergency contact is related to you.";
    if (!PHONE.test(v.emergency_phone.trim())) return "Check the emergency contact's phone number.";
    return "";
  }
  if (!v.pace) return "Choose a usual pace.";
  return "";
}
async function save(ev) {
  ev.preventDefault();
  const v = values();
  const problem = validate(v);
  $("form-error").textContent = problem;
  $("form-error").hidden = !problem;
  if (problem) return;
  if (PAGE === "profile") Object.keys(v).forEach((k) => { v[k] = v[k].trim().replace(/\s+/g, " "); });
  const state = await v2.state();
  state[PAGE] = Object.assign({}, state[PAGE], v);
  await v2.save(state);
  await v2.event(PAGE + "_saved", { values: state[PAGE], trusted: ev.isTrusted });
  saved = values();
  if (PAGE === "profile") $("ec-summary").textContent = `${v.emergency_name} (${v.emergency_relation}), ${v.emergency_phone}`;
  updateGuard();
  $("status").textContent = "Saved";
}

async function init() {
  const state = await v2.state();
  if (PAGE === "profile") renderProfile(state.profile);
  else renderPreferences(state.preferences);
  saved = values();
  const form = $("form");
  form.addEventListener("input", updateGuard);
  form.addEventListener("change", updateGuard);
  form.addEventListener("submit", save);
  v2.event("page_loaded", { page: PAGE });
}
init();
