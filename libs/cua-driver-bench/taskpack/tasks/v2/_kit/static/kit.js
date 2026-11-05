// Shared page helpers for the v2 web tasks: load and save the task state, log events.
(function () {
  "use strict";
  async function post(path, body) {
    const res = await fetch(path, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    if (!res.ok) throw new Error("save failed: " + res.status);
    return res.json();
  }
  window.v2 = {
    state: async function () {
      const res = await fetch("/api/state", { cache: "no-store" });
      return res.json();
    },
    save: function (state) { return post("/api/state", state); },
    event: function (type, data) { return post("/api/event", Object.assign({}, data || {}, { type: type })); },
    beacon: function (type, data) {
      const body = JSON.stringify(Object.assign({}, data || {}, { type: type }));
      return navigator.sendBeacon("/api/event", new Blob([body], { type: "text/plain" }));
    },
    el: function (tag, attrs, children) {
      const node = document.createElement(tag);
      Object.entries(attrs || {}).forEach(function ([k, v]) {
        if (k === "text") node.textContent = v;
        else if (k.startsWith("on")) node.addEventListener(k.slice(2), v);
        else if (v === true) node.setAttribute(k, "");
        else if (v !== false && v !== null && v !== undefined) node.setAttribute(k, v);
      });
      (children || []).forEach(function (c) { node.append(c); });
      return node;
    },
  };
})();
