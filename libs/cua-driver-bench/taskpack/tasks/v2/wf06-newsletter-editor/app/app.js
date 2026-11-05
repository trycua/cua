"use strict";
const CATEGORIES = { events: "Events", notices: "Notices", volunteering: "Volunteering", "lost-found": "Lost and found", planning: "Planning applications" };
const $ = (id) => document.getElementById(id);
let state = { posts: [] };
let current = null; // id of the post in the editor, or null for a new post
let savedRange = null;

// ------------------------------------------------------------ HTML normalisation
// Keep only p, strong, em, ul/li and a[href]; b -> strong, i -> em, div -> p; drop everything else.
const esc = (s) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
const BLOCKS = new Set(["P", "DIV", "UL", "OL", "LI", "H1", "H2", "H3", "H4", "BLOCKQUOTE"]);

function inline(node) {
  if (node.nodeType === Node.TEXT_NODE) return esc(node.textContent.replace(/ /g, " "));
  if (node.nodeType !== Node.ELEMENT_NODE) return "";
  const inner = Array.from(node.childNodes).map(inline).join("");
  const tag = node.tagName;
  if (tag === "BR") return " ";
  if (!inner) return "";
  if (tag === "B" || tag === "STRONG") return `<strong>${inner}</strong>`;
  if (tag === "I" || tag === "EM") return `<em>${inner}</em>`;
  if (tag === "A" && node.getAttribute("href")) return `<a href="${esc(node.getAttribute("href").trim())}">${inner}</a>`;
  if (tag === "SPAN" && /font-weight:\s*(bold|[6-9]00)/.test(node.getAttribute("style") || "")) return `<strong>${inner}</strong>`;
  return inner;
}
function blocks(container, out) {
  let run = [];
  const flush = () => {
    const html = run.join("").replace(/\s+/g, " ").trim();
    if (html && html.replace(/<[^>]*>/g, "").trim()) out.push(`<p>${html}</p>`);
    run = [];
  };
  Array.from(container.childNodes).forEach((child) => {
    if (child.nodeType === Node.ELEMENT_NODE && (child.tagName === "UL" || child.tagName === "OL")) {
      flush();
      const items = Array.from(child.querySelectorAll("li")).filter((li) => !li.querySelector("li"))
        .map((li) => inline(li).replace(/\s+/g, " ").trim()).filter((t) => t.replace(/<[^>]*>/g, "").trim());
      if (items.length) out.push(`<ul>${items.map((t) => `<li>${t}</li>`).join("")}</ul>`);
    } else if (child.nodeType === Node.ELEMENT_NODE && BLOCKS.has(child.tagName)) {
      flush();
      blocks(child, out);
    } else {
      run.push(inline(child));
    }
  });
  flush();
}
function normalise(root) {
  const out = [];
  blocks(root, out);
  return out.join("");
}

// ------------------------------------------------------------ rendering
function renderList() {
  const list = $("post-list");
  list.replaceChildren();
  state.posts.forEach((p) => {
    const label = p.status === "published" ? `Published ${p.published_on}` : "Draft";
    const btn = v2.el("button", { type: "button", "aria-current": String(p.id === current), onclick: () => openPost(p.id) },
      [p.title, v2.el("span", { class: "badge", text: `${CATEGORIES[p.category] || p.category} · ${label}` })]);
    list.append(v2.el("li", {}, [btn]));
  });
}
function openPost(id) {
  const post = state.posts.find((p) => p.id === id);
  current = id;
  $("editor-h").textContent = post ? `Editing: ${post.title}` : "New post";
  $("editing-note").textContent = post && post.status === "published" ? "This post is already published. Saving a draft will take it down until it is published again." : "";
  $("title").value = post ? post.title : "";
  $("category").value = post ? post.category : "";
  $("body").innerHTML = post ? post.body_html : "<p><br></p>";
  $("form-error").hidden = true;
  $("status").textContent = "";
  renderList();
}
function newPost() { openPost(null); }

// ------------------------------------------------------------ editing
function insideBody(node) { return node && $("body").contains(node.nodeType === Node.TEXT_NODE ? node.parentNode : node); }
document.addEventListener("selectionchange", () => {
  const sel = window.getSelection();
  if (sel.rangeCount && insideBody(sel.anchorNode)) savedRange = sel.getRangeAt(0).cloneRange();
  syncPressed();
});
function syncPressed() {
  const inBody = insideBody(window.getSelection().anchorNode);
  [["bold", "bold"], ["italic", "italic"]].forEach(([cmd, q]) => {
    const b = document.querySelector(`.toolbar button[data-cmd="${cmd}"]`);
    b.setAttribute("aria-pressed", String(inBody && document.queryCommandState(q)));
  });
}
function restoreSelection() {
  const body = $("body");
  body.focus();
  const sel = window.getSelection();
  if (savedRange && insideBody(savedRange.startContainer)) {
    sel.removeAllRanges();
    sel.addRange(savedRange);
  } else {
    const r = document.createRange();
    r.selectNodeContents(body);
    r.collapse(false);
    sel.removeAllRanges();
    sel.addRange(r);
  }
}
function command(cmd) {
  restoreSelection();
  if (cmd === "bold") document.execCommand("bold");
  else if (cmd === "italic") document.execCommand("italic");
  else if (cmd === "list") document.execCommand("insertUnorderedList");
  else if (cmd === "link") return openLinkModal();
  syncPressed();
  v2.event("format_applied", { command: cmd });
}
function openLinkModal() {
  const sel = window.getSelection();
  $("link-text").value = sel.rangeCount ? sel.toString() : "";
  $("link-href").value = "";
  $("link-error").hidden = true;
  $("link-modal").hidden = false;
  ($("link-text").value ? $("link-href") : $("link-text")).focus();
}
function closeLinkModal() { $("link-modal").hidden = true; }
function insertLink() {
  const text = $("link-text").value.trim();
  const href = $("link-href").value.trim();
  const err = $("link-error");
  if (!text) { err.textContent = "Enter the text to show for the link."; err.hidden = false; return; }
  if (!/^(https?:\/\/|mailto:)\S+$/i.test(href)) { err.textContent = "Enter a full address starting with https://"; err.hidden = false; return; }
  closeLinkModal();
  restoreSelection();
  document.execCommand("insertHTML", false, `<a href="${esc(href)}">${esc(text)}</a>`);
  v2.event("link_inserted", { text, href });
}

// ------------------------------------------------------------ saving
function fail(msg) { const e = $("form-error"); e.textContent = msg; e.hidden = false; }
async function save(publish, ev) {
  $("form-error").hidden = true;
  const title = $("title").value.trim();
  const category = $("category").value;
  const html = normalise($("body"));
  if (!title) return fail("Give the post a title.");
  if (!category) return fail("Choose a category.");
  if (!html) return fail("Write something in the body.");
  let post = state.posts.find((p) => p.id === current);
  if (!post) {
    const next = Math.max(0, ...state.posts.map((p) => Number(p.id.slice(3)))) + 1;
    post = { id: "QL-" + String(next).padStart(4, "0"), title: "", category: "", status: "draft", published_on: "", body_html: "" };
    state.posts.push(post);
    current = post.id;
  }
  Object.assign(post, { title, category, body_html: html });
  if (publish) { post.status = "published"; post.published_on = "2026-10-11"; }
  else { post.status = "draft"; post.published_on = ""; }
  await v2.save(state);
  await v2.event(publish ? "post_published" : "draft_saved", { id: post.id, title, category, body_html: html, trusted: ev.isTrusted });
  $("body").innerHTML = html || "<p><br></p>";
  $("editor-h").textContent = `Editing: ${post.title}`;
  $("editing-note").textContent = "";
  $("status").textContent = publish ? `Published “${title}”.` : `Draft “${title}” saved.`;
  renderList();
}

async function init() {
  state = await v2.state();
  document.execCommand("defaultParagraphSeparator", false, "p");
  newPost();
  $("new-post").addEventListener("click", newPost);
  document.querySelectorAll(".toolbar button").forEach((b) => {
    b.addEventListener("mousedown", (ev) => ev.preventDefault());
    b.addEventListener("click", () => command(b.dataset.cmd));
  });
  $("link-cancel").addEventListener("click", () => { closeLinkModal(); restoreSelection(); });
  $("link-insert").addEventListener("click", insertLink);
  $("link-modal").addEventListener("keydown", (ev) => {
    if (ev.key === "Enter") { ev.preventDefault(); insertLink(); }
    if (ev.key === "Escape") closeLinkModal();
  });
  $("save-draft").addEventListener("click", (ev) => save(false, ev));
  $("publish").addEventListener("click", (ev) => save(true, ev));
}
init();
