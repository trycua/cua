"use strict";
const COUNTRIES = [
  { code: "CA", name: "Canada", regions: ["Alberta", "British Columbia", "Manitoba", "Nova Scotia", "Ontario", "Quebec"],
    re: /^[A-Z]\d[A-Z] ?\d[A-Z]\d$/, fmt: (p) => p.replace(/^(...) ?(...)$/, "$1 $2"), hint: "A1A 1A1" },
  { code: "DE", name: "Germany", regions: ["Bavaria", "Hesse", "Lower Saxony", "Saxony", "Schleswig-Holstein"],
    re: /^\d{5}$/, fmt: (p) => p, hint: "12345" },
  { code: "IE", name: "Ireland", regions: ["Cork", "Dublin", "Galway", "Kerry", "Mayo"],
    re: /^[A-Z]\d[\dW] ?[A-Z\d]{4}$/, fmt: (p) => p.replace(/^(...) ?(....)$/, "$1 $2"), hint: "A65 F4E2" },
  { code: "NL", name: "Netherlands", regions: ["Drenthe", "Friesland", "Gelderland", "Groningen", "Overijssel", "Utrecht", "Zeeland"],
    re: /^\d{4} ?[A-Z]{2}$/, fmt: (p) => p.replace(/^(\d{4}) ?([A-Z]{2})$/, "$1 $2"), hint: "1234 AB" },
  { code: "NZ", name: "New Zealand", regions: ["Canterbury", "Nelson", "Otago", "Waikato", "Wellington"],
    re: /^\d{4}$/, fmt: (p) => p, hint: "1234" },
  { code: "GB", name: "United Kingdom", regions: ["England", "Northern Ireland", "Scotland", "Wales"],
    re: /^[A-Z]{1,2}\d[A-Z\d]? ?\d[A-Z]{2}$/, fmt: (p) => p.replace(/^(.+?) ?(\d[A-Z]{2})$/, "$1 $2"), hint: "AB1 2CD" },
];
const OPTIONS = [
  ["parcel", "Tracked parcel", "3 to 5 working days, up to 10 kg"],
  ["pallet", "Pallet freight", "Booked delivery slot, for bulk orders"],
  ["hub", "Collect from a co-op hub", "Pick up at your nearest member hub"],
];
const FIELDS = ["name", "line1", "line2", "town", "region", "postcode", "country"];
let state = {};
const blocks = {};
const $ = (id) => document.getElementById(id);
const byCode = (c) => COUNTRIES.find((x) => x.code === c);
const clean = (s) => s.trim().replace(/\s+/g, " ");

function combobox(prefix, onPick) {
  const id = prefix + "-country";
  const input = v2.el("input", { id, type: "text", role: "combobox", autocomplete: "off", "aria-autocomplete": "list", "aria-expanded": "false", "aria-controls": id + "-list" });
  const list = v2.el("ul", { id: id + "-list", role: "listbox", "aria-label": "Countries", hidden: true });
  const box = { value: "", shown: [], active: -1, input };
  const highlight = () => {
    [...list.children].forEach((li, i) => li.classList.toggle("active", i === box.active));
    if (box.active >= 0) input.setAttribute("aria-activedescendant", list.children[box.active].id);
    else input.removeAttribute("aria-activedescendant");
  };
  const close = () => { list.hidden = true; input.setAttribute("aria-expanded", "false"); box.active = -1; highlight(); };
  const open = () => {
    const q = input.value.trim().toLowerCase();
    const all = COUNTRIES.filter((c) => !q || box.value || c.name.toLowerCase().includes(q));
    box.shown = all.sort((a, b) => (b.name.toLowerCase().startsWith(q) - a.name.toLowerCase().startsWith(q)) || a.name.localeCompare(b.name));
    list.replaceChildren(...box.shown.map((c) => {
      const li = v2.el("li", { role: "option", id: `${id}-opt-${c.code}`, "aria-selected": String(c.code === box.value), text: c.name });
      li.addEventListener("mousedown", (e) => e.preventDefault());
      li.addEventListener("click", () => pick(c.code));
      return li;
    }));
    if (!box.shown.length) list.append(v2.el("li", { class: "none", text: "No matching country" }));
    list.hidden = false;
    input.setAttribute("aria-expanded", "true");
    highlight();
  };
  const pick = (code, quiet) => {
    const changed = code !== box.value;
    box.value = code;
    input.value = code ? byCode(code).name : "";
    close();
    if (changed || quiet) onPick(code, quiet);
  };
  box.set = (code) => pick(code, true);
  input.addEventListener("input", () => {
    const exact = COUNTRIES.find((c) => c.name.toLowerCase() === input.value.trim().toLowerCase());
    if (box.value && (!exact || exact.code !== box.value)) { box.value = ""; onPick(""); }
    open();
  });
  input.addEventListener("click", () => { if (list.hidden) open(); });
  input.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (list.hidden) open();
      const n = box.shown.length;
      if (!n) return;
      box.active = e.key === "ArrowDown" ? (box.active + 1) % n : (box.active <= 0 ? n - 1 : box.active - 1);
      highlight();
    } else if (e.key === "Enter") {
      if (!list.hidden && box.active >= 0) { e.preventDefault(); pick(box.shown[box.active].code); }
      else if (!list.hidden && box.shown.length === 1) { e.preventDefault(); pick(box.shown[0].code); }
    } else if (e.key === "Escape") close();
  });
  input.addEventListener("blur", () => {
    const exact = COUNTRIES.find((c) => c.name.toLowerCase() === input.value.trim().toLowerCase());
    if (exact) pick(exact.code); else close();
  });
  return { box, el: v2.el("div", { class: "combo" }, [input, list]) };
}

function addressBlock(prefix, fieldset) {
  const f = (key, label, input, cls, extra) => {
    const err = v2.el("p", { class: "error", id: `${prefix}-${key}-error`, hidden: true });
    fieldset.append(v2.el("div", { class: "f " + (cls || "") }, [v2.el("label", { for: input.id || `${prefix}-${key}`, text: label }), input, ...(extra || []), err]));
  };
  const text = (key) => v2.el("input", { id: `${prefix}-${key}`, type: "text", autocomplete: "off" });
  const block = { prefix, inputs: {} };
  const region = v2.el("select", { id: `${prefix}-region`, disabled: true });
  const hint = v2.el("span", { class: "hint", id: `${prefix}-postcode-hint` });
  const combo = combobox(prefix, (code) => {
    const c = byCode(code);
    region.replaceChildren(v2.el("option", { value: "", text: c ? "Choose a region" : "Choose a country first" }), ...(c ? c.regions.map((r) => v2.el("option", { value: r, text: r })) : []));
    region.disabled = !c;
    hint.textContent = c ? `Format: ${c.hint}` : "";
  });
  block.combo = combo.box;
  ["name", "line1", "line2", "town", "postcode"].forEach((k) => { block.inputs[k] = text(k); });
  block.inputs.region = region;
  block.inputs.postcode.setAttribute("aria-describedby", hint.id);
  f("name", "Full name", block.inputs.name, "wide");
  fieldset.append(v2.el("div", { class: "f wide" }, [v2.el("label", { for: `${prefix}-country`, text: "Country" }), combo.el, v2.el("p", { class: "error", id: `${prefix}-country-error`, hidden: true })]));
  f("line1", "Address line 1", block.inputs.line1, "wide");
  f("line2", "Address line 2 (optional)", block.inputs.line2, "wide");
  f("town", "Town or city", block.inputs.town);
  f("region", "Region", region);
  f("postcode", "Postal code", block.inputs.postcode, "", [hint]);
  combo.box.set("");
  blocks[prefix] = block;
}

function fill(prefix, data) {
  const b = blocks[prefix];
  b.combo.set(data.country || "");
  ["name", "line1", "line2", "town", "postcode"].forEach((k) => { b.inputs[k].value = data[k] || ""; });
  b.inputs.region.value = data.region || "";
}

function read(prefix) {
  const b = blocks[prefix];
  const out = {};
  const errors = [];
  const err = (k, msg) => { const e = $(`${prefix}-${k}-error`); e.textContent = msg; e.hidden = !msg; if (msg) errors.push(msg); };
  ["name", "line1", "line2", "town"].forEach((k) => { out[k] = clean(b.inputs[k].value); });
  out.country = b.combo.value;
  out.region = b.inputs.region.value;
  const c = byCode(out.country);
  const pc = clean(b.inputs.postcode.value).toUpperCase();
  err("name", out.name ? "" : "Enter a full name.");
  err("country", c ? "" : "Choose a country from the list.");
  err("line1", out.line1 ? "" : "Enter the first address line.");
  err("line2", "");
  err("town", out.town ? "" : "Enter a town or city.");
  err("region", !c || out.region ? "" : "Choose a region.");
  err("postcode", !c ? "" : c.re.test(pc) ? "" : `Postal codes in ${c.name} look like ${c.hint}.`);
  out.postcode = c && c.re.test(pc) ? c.fmt(pc) : pc;
  if (c && c.re.test(pc)) b.inputs.postcode.value = out.postcode;
  return { out, errors };
}

async function submit(ev) {
  ev.preventDefault();
  const ship = read("ship");
  const same = $("billing-same").checked;
  const bill = same ? { out: null, errors: [] } : read("bill");
  const option = (document.querySelector("input[name=option]:checked") || {}).value;
  const problems = [...ship.errors.map((e) => "Delivery address: " + e), ...bill.errors.map((e) => "Billing address: " + e)];
  if (!option) problems.push("Choose a delivery option.");
  const box = $("form-error");
  box.textContent = problems.length ? `Please fix ${problems.length} problem${problems.length > 1 ? "s" : ""}: ${problems.join(" ")}` : "";
  box.hidden = !problems.length;
  if (problems.length) { $("confirmation").hidden = true; return; }
  state.delivery = { status: "saved", shipping: ship.out, billing_same: same, billing: bill.out, option, notes: $("notes").value.trim() };
  await v2.save(state);
  await v2.event("delivery_saved", { delivery: state.delivery, trusted: ev.isTrusted });
  $("confirmation").textContent = "Delivery details saved. Payment opens on 2 November 2026; we will email you a link.";
  $("confirmation").hidden = false;
}

function renderSummary() {
  const o = state.order;
  $("summary").replaceChildren(v2.el("h2", { text: "Your order" }), v2.el("dl", {}, [
    v2.el("dt", { text: "Order" }), v2.el("dd", { text: o.id }),
    v2.el("dt", { text: "Member" }), v2.el("dd", { text: `${o.member} (${o.member_no})` }),
    v2.el("dt", { text: "Season" }), v2.el("dd", { text: o.season }),
    v2.el("dt", { text: "Packets" }), v2.el("dd", { text: String(o.packets) }),
    v2.el("dt", { text: "Weight" }), v2.el("dd", { text: `${o.weight_kg} kg` }),
  ]));
}

async function init() {
  state = await v2.state();
  renderSummary();
  addressBlock("ship", $("ship"));
  addressBlock("bill", $("bill"));
  const d = state.delivery;
  fill("ship", d.shipping);
  if (d.billing) fill("bill", d.billing);
  $("billing-same").checked = d.billing_same;
  $("bill").hidden = d.billing_same;
  $("billing-same").addEventListener("change", () => { $("bill").hidden = $("billing-same").checked; });
  OPTIONS.forEach(([v, t, h]) => $("options").append(v2.el("label", {}, [
    v2.el("input", { type: "radio", name: "option", value: v, id: "opt-" + v, checked: d.option === v }), ` ${t} `, v2.el("span", { class: "hint", text: "· " + h }),
  ])));
  $("notes").value = d.notes;
  $("form").addEventListener("submit", submit);
}
init();
