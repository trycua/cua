"use strict";
const COURSES = [
  ["laser", "Laser cutter basics"],
  ["bandsaw", "Bandsaw safety"],
  ["pottery", "Pottery wheel intro"],
  ["solder", "Soldering 101"],
];
const SLOTS = ["10:00", "14:00", "18:30"];
const FULL = { "2026-11-12": ["10:00"], "2026-11-10": ["14:00", "18:30"], "2026-10-22": ["18:30"] };
const TODAY = new Date(2026, 9, 11);
const MONTHS = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const DAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
let state = { bookings: [] };
let view = new Date(2026, 9, 1);
let pick = { date: null, slot: null, count: 1 };
const $ = (id) => document.getElementById(id);
const iso = (d) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
const longDate = (s) => { const [y, m, d] = s.split("-").map(Number); const dt = new Date(y, m - 1, d); return `${DAYS[dt.getDay()]} ${d} ${MONTHS[m - 1]} ${y}`; };
const courseName = (id) => (COURSES.find((c) => c[0] === id) || [id, id])[1];

function renderBookings() {
  const list = $("booking-list");
  list.replaceChildren();
  state.bookings.forEach((b) => {
    const cancel = v2.el("button", { type: "button", text: "Cancel", "aria-label": `Cancel ${courseName(b.course)} on ${longDate(b.date)}`, onclick: () => cancelBooking(b.id) });
    list.append(v2.el("li", {}, [`${courseName(b.course)} · ${longDate(b.date)} · ${b.slot} · ${b.participants} ${b.participants === 1 ? "person" : "people"} `, cancel]));
  });
  if (!state.bookings.length) list.append(v2.el("li", { text: "No bookings yet." }));
}
async function cancelBooking(id) {
  if (!window.confirm("Cancel this booking?")) return;
  state.bookings = state.bookings.filter((b) => b.id !== id);
  await v2.save(state);
  await v2.event("booking_cancelled", { id });
  renderBookings();
}
function renderCourses() {
  const box = $("courses");
  COURSES.forEach(([id, name]) => {
    box.append(v2.el("label", {}, [v2.el("input", { type: "radio", name: "course", value: id }), name]));
  });
}
function renderCalendar() {
  $("month-label").textContent = `${MONTHS[view.getMonth()]} ${view.getFullYear()}`;
  const grid = $("cal-grid");
  grid.replaceChildren();
  ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"].forEach((d) => grid.append(v2.el("span", { class: "dow", text: d, "aria-hidden": "true" })));
  const first = new Date(view.getFullYear(), view.getMonth(), 1);
  for (let i = 0; i < first.getDay(); i++) grid.append(v2.el("span"));
  const days = new Date(view.getFullYear(), view.getMonth() + 1, 0).getDate();
  for (let d = 1; d <= days; d++) {
    const dt = new Date(view.getFullYear(), view.getMonth(), d);
    const closed = dt.getDay() === 0 || dt.getDay() === 1 || dt < TODAY;
    grid.append(v2.el("button", {
      type: "button", text: String(d), disabled: closed,
      "aria-label": `${DAYS[dt.getDay()]} ${d} ${MONTHS[dt.getMonth()]} ${dt.getFullYear()}${closed ? " (closed)" : ""}`,
      onclick: () => chooseDate(iso(dt)),
    }));
  }
}
function chooseDate(value) {
  pick.date = value;
  pick.slot = null;
  $("date").value = longDate(value);
  $("calendar").hidden = true;
  $("open-cal").setAttribute("aria-expanded", "false");
  renderSlots();
}
function renderSlots() {
  const box = $("slots");
  box.replaceChildren();
  if (!pick.date) return;
  SLOTS.forEach((s) => {
    const full = (FULL[pick.date] || []).includes(s);
    box.append(v2.el("button", {
      type: "button", role: "radio", "aria-checked": String(pick.slot === s), disabled: full,
      text: full ? `${s} (full)` : s,
      onclick: () => { pick.slot = s; renderSlots(); },
    }));
  });
}
function setCount(n) {
  pick.count = Math.max(1, Math.min(6, n));
  $("count").textContent = String(pick.count);
}
function fail(msg) { const e = $("form-error"); e.textContent = msg; e.hidden = false; }

async function submit(ev) {
  ev.preventDefault();
  $("form-error").hidden = true;
  const course = (document.querySelector('input[name="course"]:checked') || {}).value;
  const member = $("member").value.trim().toUpperCase();
  const memberOk = /^FH-\d{5}$/.test(member);
  $("member-error").hidden = memberOk;
  if (!course) return fail("Choose a course.");
  if (!pick.date) return fail("Choose a date.");
  if (!pick.slot) return fail("Choose a session time.");
  if (!memberOk) return fail("Check the member number.");
  const access = $("access").checked;
  const note = access ? $("access-note").value.trim() : "";
  if (access && !note) return fail("Tell us what you need, or untick Accessibility needs.");
  const booking = {
    id: "B-" + String(1000 + state.bookings.length * 7 + member.charCodeAt(4)).slice(-4),
    course, date: pick.date, slot: pick.slot, participants: pick.count, member, access, access_note: note,
  };
  state.bookings.push(booking);
  await v2.save(state);
  await v2.event("booking_submitted", { booking, trusted: ev.isTrusted });
  renderBookings();
  $("confirmation").textContent = `Booked: ${courseName(course)} on ${longDate(pick.date)} at ${pick.slot} (reference ${booking.id}).`;
  $("confirmation").hidden = false;
  $("book").reset();
  pick = { date: null, slot: null, count: 1 };
  setCount(1);
  $("access-box").hidden = true;
  renderSlots();
}

async function init() {
  state = await v2.state();
  renderBookings();
  renderCourses();
  renderCalendar();
  $("open-cal").addEventListener("click", () => {
    const cal = $("calendar");
    cal.hidden = !cal.hidden;
    $("open-cal").setAttribute("aria-expanded", String(!cal.hidden));
  });
  $("prev-month").addEventListener("click", () => { view = new Date(view.getFullYear(), view.getMonth() - 1, 1); renderCalendar(); });
  $("next-month").addEventListener("click", () => { view = new Date(view.getFullYear(), view.getMonth() + 1, 1); renderCalendar(); });
  $("fewer").addEventListener("click", () => setCount(pick.count - 1));
  $("more").addEventListener("click", () => setCount(pick.count + 1));
  $("access").addEventListener("change", () => { $("access-box").hidden = !$("access").checked; });
  $("book").addEventListener("submit", submit);
}
init();
