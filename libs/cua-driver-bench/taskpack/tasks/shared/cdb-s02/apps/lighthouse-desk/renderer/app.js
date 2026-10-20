"use strict";

const appRoot = document.getElementById("app");
const status = document.getElementById("status");
const productName = document.getElementById("product-name");

function setStatus(message, error) {
  status.textContent = message;
  status.classList.toggle("error", Boolean(error));
}

function element(tag, attributes, children) {
  const node = document.createElement(tag);
  Object.keys(attributes || {}).forEach(function (key) {
    if (key === "className") node.className = attributes[key];
    else node.setAttribute(key, attributes[key]);
  });
  (children || []).forEach(function (child) {
    node.append(child instanceof Node ? child : document.createTextNode(String(child)));
  });
  return node;
}

function summary(record, preferred) {
  for (const key of preferred) {
    if (record[key] !== undefined && record[key] !== "") return String(record[key]);
  }
  return String(record.id || "Untitled record");
}

function recordList(records, preferred, onSelect, protectedItems) {
  const list = element("ul", { className: "items" });
  records.forEach(function (record) {
    const content = [
      element("strong", {}, [summary(record, preferred)]),
      element("span", { className: "meta" }, [String(record.id || record.sender || record.from || "")]),
    ];
    const item = onSelect
      ? element("button", { className: "item" + (record.protected ? " protected" : ""), type: "button" }, content)
      : element("article", { className: "item" }, content);
    if (onSelect) item.addEventListener("click", function () { onSelect(record); });
    if (protectedItems && record.protected) item.setAttribute("aria-label", summary(record, preferred) + " (protected)");
    const li = element("li", {}, [item]);
    list.append(li);
  });
  return list;
}

function panel(title, children) {
  return element("section", { className: "panel" }, [element("h2", {}, [title])].concat(children));
}

function inputField(name, labelText, type, wide) {
  const input = element(type === "textarea" ? "textarea" : "input", { id: name, name: name });
  if (type && type !== "textarea") input.type = type;
  return { input: input, label: element("label", { className: wide ? "wide" : "" }, [labelText, input]) };
}

async function showMail() {
  const messages = await window.lighthouse.mail.list();
  const details = element("pre", { className: "detail", tabindex: "0" }, ["Choose a message to inspect it."]);
  const list = recordList(messages, ["subject", "title"], function (message) {
    details.textContent = JSON.stringify(message, null, 2);
    setStatus("Message opened read-only.");
  });
  appRoot.replaceChildren(panel("Messages", [list]), panel("Message details", [details]));
  setStatus(messages.length + " read-only message" + (messages.length === 1 ? "" : "s") + " loaded.");
}

function blockerForm(refreshBlockers) {
  const form = element("form", { "aria-label": "Record a scheduling blocker" });
  const fields = [
    inputField("conflict_id", "Conflict ID", "text"),
    inputField("affected_session", "Affected session", "text"),
    inputField("conflicting_event", "Conflicting event", "text"),
    inputField("escalation", "Escalation", "text"),
    inputField("reason", "Reason", "textarea", true),
  ];
  fields.forEach(function (field) { field.input.required = true; form.append(field.label); });
  form.append(element("div", { className: "actions" }, [element("button", { type: "submit" }, ["Record blocker"])]));
  form.addEventListener("submit", async function (event) {
    event.preventDefault();
    const values = Object.fromEntries(new FormData(form).entries());
    try {
      const blocker = await window.lighthouse.chat.recordBlocker(values);
      form.reset();
      await refreshBlockers();
      setStatus("Recorded blocker " + blocker.conflict_id + " with an audit entry.");
    } catch (error) {
      setStatus(error.message, true);
    }
  });
  return form;
}

async function showChat() {
  const messages = await window.lighthouse.chat.list();
  const details = element("pre", { className: "detail", tabindex: "0" }, ["Choose a chat message to inspect it."]);
  const list = recordList(messages, ["text", "message", "body"], function (message) {
    details.textContent = JSON.stringify(message, null, 2);
    setStatus("Chat message opened read-only.");
  });
  const blockersPanel = panel("Recorded blockers", []);
  async function refreshBlockers() {
    const blockers = await window.lighthouse.chat.listBlockers();
    const blockerList = element("ul", { className: "items" });
    blockers.forEach(function (blocker) {
      const remove = element("button", {
        type: "button",
        className: "danger",
        "aria-label": "Remove blocker " + blocker.conflict_id,
      }, ["Remove blocker"]);
      remove.addEventListener("click", async function () {
        try {
          await window.lighthouse.chat.removeBlocker(blocker.conflict_id);
          await refreshBlockers();
          setStatus("Removed blocker " + blocker.conflict_id + " with an audit entry.");
        } catch (error) {
          setStatus(error.message, true);
        }
      });
      blockerList.append(element("li", { className: "item" }, [
        element("strong", {}, [blocker.conflict_id]),
        element("span", { className: "meta" }, [blocker.affected_session + " conflicts with " + blocker.conflicting_event]),
        remove,
      ]));
    });
    if (!blockers.length) {
      blockerList.append(element("li", { className: "meta" }, ["No blockers recorded."]));
    }
    blockersPanel.replaceChildren(element("h2", {}, ["Recorded blockers"]), blockerList);
  }
  appRoot.replaceChildren(
    panel("Conversation", [list]),
    panel("Message details", [details]),
    blockersPanel,
    panel("Record blocker", [blockerForm(refreshBlockers)])
  );
  await refreshBlockers();
  setStatus(messages.length + " chat message" + (messages.length === 1 ? "" : "s") + " loaded.");
}

function calendarForm(refresh) {
  const form = element("form", { "aria-label": "Calendar event editor" });
  const definitions = [
    ["id", "ID", "text"], ["title", "Title", "text"], ["date", "Date", "date"],
    ["start", "Start (HH:MM)", "text"], ["end", "End (HH:MM)", "text"], ["room", "Room", "text"],
    ["attendees", "Attendees (CSV)", "text", true], ["status", "Status", "text", true],
  ];
  const fields = {};
  definitions.forEach(function (definition) {
    fields[definition[0]] = inputField(definition[0], definition[1], definition[2], definition[3]);
    if (["id", "title", "date", "start", "end", "status"].includes(definition[0])) fields[definition[0]].input.required = true;
    form.append(fields[definition[0]].label);
  });
  const save = element("button", { type: "submit" }, ["Create event"]);
  const remove = element("button", { type: "button", className: "danger" }, ["Delete event"]);
  const clear = element("button", { type: "button", className: "secondary" }, ["Clear form"]);
  remove.disabled = true;
  form.append(element("div", { className: "actions" }, [save, remove, clear]));
  let originalId = null;

  function reset() {
    originalId = null;
    form.reset();
    Array.from(form.elements).forEach(function (control) { control.disabled = false; });
    remove.disabled = true;
    save.textContent = "Create event";
  }

  function select(event) {
    originalId = event.id;
    definitions.forEach(function (definition) {
      const value = event[definition[0]];
      fields[definition[0]].input.value = Array.isArray(value) ? value.join(", ") : (value || "");
    });
    const immutable = event.protected === true;
    definitions.forEach(function (definition) { fields[definition[0]].input.disabled = immutable; });
    save.disabled = immutable;
    remove.disabled = immutable;
    save.textContent = immutable ? "Protected event" : "Update event";
    setStatus(immutable ? "Protected hold selected. It cannot be changed or deleted." : "Event selected for editing.");
  }

  function values() {
    return Object.fromEntries(new FormData(form).entries());
  }

  form.addEventListener("submit", async function (event) {
    event.preventDefault();
    try {
      if (originalId) await window.lighthouse.calendar.update(originalId, values());
      else await window.lighthouse.calendar.create(values());
      setStatus(originalId ? "Event updated." : "Event created.");
      reset();
      await refresh();
    } catch (error) { setStatus(error.message, true); }
  });
  remove.addEventListener("click", async function () {
    if (!originalId) return;
    try {
      await window.lighthouse.calendar.delete(originalId);
      setStatus("Event deleted.");
      reset();
      await refresh();
    } catch (error) { setStatus(error.message, true); }
  });
  clear.addEventListener("click", function () { reset(); setStatus("Editor cleared."); });
  return { form: form, select: select };
}

async function showCalendar() {
  let listPanel = panel("Events", []);
  const editor = calendarForm(refresh);
  async function refresh() {
    const events = await window.lighthouse.calendar.list();
    const list = recordList(events, ["title"], editor.select, true);
    listPanel.replaceChildren(element("h2", {}, ["Events"]), list);
    return events;
  }
  appRoot.replaceChildren(listPanel, panel("Event editor", [editor.form]));
  const events = await refresh();
  setStatus(events.length + " calendar event" + (events.length === 1 ? "" : "s") + " loaded. Protected holds are immutable.");
}

async function start() {
  try {
    const info = await window.lighthouse.info();
    document.title = info.name;
    productName.textContent = info.name;
    document.body.dataset.surface = info.surface;
    if (info.surface === "mail") await showMail();
    else if (info.surface === "chat") await showChat();
    else if (info.surface === "calendar") await showCalendar();
    else throw new Error("Unknown surface " + info.surface);
  } catch (error) {
    appRoot.replaceChildren(panel("Unable to load workspace", [element("p", {}, [error.message])]));
    setStatus(error.message, true);
  } finally {
    appRoot.setAttribute("aria-busy", "false");
  }
}

start();
