"use strict";
(function () {
  var list = document.getElementById("case-list");
  var detail = document.getElementById("detail");
  var selected = null;
  function show(value) { return value || "—"; }
  function renderList() {
    return window.rillwatch.list().then(function (cases) {
      list.innerHTML = "";
      cases.forEach(function (item) {
        var row = document.createElement("li"); row.tabIndex = 0; row.setAttribute("role", "button");
        row.setAttribute("aria-selected", item.id === selected ? "true" : "false");
        row.setAttribute("aria-label", item.id + ": " + item.title + " (" + item.status + ")");
        row.innerHTML = "<span class=case-code></span><span class=status></span><div class=title></div>";
        row.querySelector(".case-code").textContent = item.id; row.querySelector(".status").textContent = item.status; row.querySelector(".title").textContent = item.title;
        function activate() { openCase(item.id); } row.addEventListener("click", activate); row.addEventListener("keydown", function (event) { if (event.key === "Enter" || event.key === " ") { event.preventDefault(); activate(); } }); list.appendChild(row);
      });
    });
  }
  function openCase(id) {
    selected = id;
    return window.rillwatch.get(id).then(function (item) {
      var node = document.getElementById("case-template").content.cloneNode(true);
      node.querySelector(".case-id").textContent = item.id; node.querySelector(".case-title").textContent = item.title;
      node.querySelector(".fact-status").textContent = item.status; node.querySelector(".fact-component").textContent = item.component; node.querySelector(".fact-reported").textContent = item.reported_at; node.querySelector(".fact-duplicate").textContent = show(item.duplicate_of); node.querySelector(".case-body").textContent = item.body;
      var form = node.querySelector("form"); var editable = ["RWC-193", "RWC-201", "RWC-206", "RWC-212"].includes(item.id);
      form.elements.status.value = item.status; form.elements.duplicate_of.value = item.duplicate_of || "";
      ["wiki_ref", "repro_url", "log_ref"].forEach(function (key) { form.elements[key].value = item.evidence ? item.evidence[key] : ""; });
      if (!editable) { form.hidden = true; node.querySelector(".readonly-note").textContent = "This case is read-only."; }
      form.addEventListener("submit", function (event) { event.preventDefault(); var status = node.querySelector(".save-status"); window.rillwatch.update(item.id, { status: form.elements.status.value, duplicate_of: form.elements.duplicate_of.value || null, evidence: { wiki_ref: form.elements.wiki_ref.value.trim(), repro_url: form.elements.repro_url.value.trim(), log_ref: form.elements.log_ref.value.trim() } }).then(function () { status.textContent = "Saved"; return renderList().then(function () { return openCase(item.id); }); }).catch(function (error) { status.textContent = "Not saved: " + error.message; }); });
      detail.innerHTML = ""; detail.appendChild(node); return renderList();
    });
  }
  window.rillwatch.workspace().then(function (value) { document.getElementById("workspace").textContent = value; }); renderList();
})();
