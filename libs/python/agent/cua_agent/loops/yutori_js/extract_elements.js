(function (filterType) {
  var MAX_DEPTH = 15;
  var SKIPPED_TAGS = {
    script: true,
    style: true,
    meta: true,
    link: true,
    title: true,
    noscript: true,
  };
  var INTERACTIVE_TAGS = {
    a: true,
    button: true,
    input: true,
    select: true,
    textarea: true,
    details: true,
    summary: true,
  };
  var SEMANTIC_TAGS = {
    h1: true,
    h2: true,
    h3: true,
    h4: true,
    h5: true,
    h6: true,
    nav: true,
    main: true,
    header: true,
    footer: true,
    section: true,
    article: true,
    aside: true,
  };
  var FUNCTIONAL_KEYWORDS = ["search", "dropdown", "menu", "modal", "dialog", "popup", "toolbar", "sidebar", "content", "text"];

  // Keep a stable ref for each live element so later actions can target the same node.
  function ensureStore() {
    if (!window.__yutoriElementRefs) {
      window.__yutoriElementRefs = {};
    }
    if (!window.__yutoriElementIds) {
      window.__yutoriElementIds = new WeakMap();
    }
    if (!window.__yutoriRefCounter) {
      window.__yutoriRefCounter = 0;
    }
  }

  function compactWhitespace(value) {
    return value ? value.replace(/\s+/g, " ").trim() : "";
  }

  function readDirectText(element) {
    var chunks = [];
    for (var i = 0; i < element.childNodes.length; i++) {
      var node = element.childNodes[i];
      if (node.nodeType === Node.TEXT_NODE && node.textContent) {
        chunks.push(node.textContent);
      }
    }
    return compactWhitespace(chunks.join(" "));
  }

  function getRole(element) {
    var explicitRole = element.getAttribute("role");
    if (explicitRole) {
      return explicitRole;
    }

    var tag = element.tagName.toLowerCase();
    if (tag === "input") {
      var type = (element.getAttribute("type") || "").toLowerCase();
      if (type === "submit" || type === "button" || type === "file") {
        return "button";
      }
      if (type === "checkbox") {
        return "checkbox";
      }
      if (type === "radio") {
        return "radio";
      }
      return "textbox";
    }

    var tagRoles = {
      a: "link",
      button: "button",
      select: "combobox",
      textarea: "textbox",
      h1: "heading",
      h2: "heading",
      h3: "heading",
      h4: "heading",
      h5: "heading",
      h6: "heading",
      img: "image",
      nav: "navigation",
      main: "main",
      header: "banner",
      footer: "contentinfo",
      section: "region",
      article: "article",
      aside: "complementary",
      form: "form",
      table: "table",
      ul: "list",
      ol: "list",
      li: "listitem",
      label: "label",
    };

    return tagRoles[tag] || "generic";
  }

  function getName(element) {
    var tag = element.tagName.toLowerCase();
    var candidate = "";

    if (tag === "select") {
      var selectedOption = element.options[element.selectedIndex] || element.querySelector("option[selected]");
      if (selectedOption && selectedOption.textContent) {
        candidate = compactWhitespace(selectedOption.textContent);
        if (candidate) {
          return candidate;
        }
      }
    }

    var attributeNames = ["aria-label", "placeholder", "title", "alt"];
    for (var i = 0; i < attributeNames.length; i++) {
      candidate = compactWhitespace(element.getAttribute(attributeNames[i]) || "");
      if (candidate) {
        return candidate;
      }
    }

    if (element.id) {
      var label = document.querySelector('label[for="' + element.id.replace(/"/g, '\\"') + '"]');
      candidate = label && label.textContent ? compactWhitespace(label.textContent) : "";
      if (candidate) {
        return candidate;
      }
    }

    if (tag === "input") {
      var type = (element.getAttribute("type") || "").toLowerCase();
      var rawValue = compactWhitespace(element.getAttribute("value") || "");
      if (type === "submit" && rawValue) {
        return rawValue;
      }

      candidate = compactWhitespace(element.value || "");
      if (candidate && candidate.length < 50) {
        return candidate;
      }
    }

    if (tag === "button" || tag === "a" || tag === "summary") {
      candidate = readDirectText(element);
      if (candidate) {
        return candidate;
      }
    }

    if (/^h[1-6]$/.test(tag)) {
      candidate = compactWhitespace(element.textContent || "");
      if (candidate) {
        return candidate.slice(0, 100);
      }
    }

    if (tag === "img") {
      var src = element.getAttribute("src") || "";
      if (src) {
        var filename = src.split("/").pop() || "";
        filename = filename.split("?")[0];
        return "Image: " + filename;
      }
    }

    candidate = readDirectText(element);
    if (candidate && candidate.length >= 3) {
      return candidate.length > 50 ? candidate.slice(0, 50) + "..." : candidate;
    }

    return "";
  }

  function isVisible(element) {
    var style = window.getComputedStyle(element);
    return (
      style.display !== "none" &&
      style.visibility !== "hidden" &&
      style.opacity !== "0" &&
      element.offsetWidth > 0 &&
      element.offsetHeight > 0
    );
  }

  function isInViewport(element) {
    var rect = element.getBoundingClientRect();
    return rect.top < window.innerHeight && rect.bottom > 0 && rect.left < window.innerWidth && rect.right > 0;
  }

  function isInteractive(element) {
    var role = element.getAttribute("role");
    var tag = element.tagName.toLowerCase();
    return (
      INTERACTIVE_TAGS[tag] === true ||
      element.getAttribute("onclick") !== null ||
      element.getAttribute("tabindex") !== null ||
      role === "button" ||
      role === "link" ||
      element.getAttribute("contenteditable") === "true"
    );
  }

  function isSemantic(element) {
    return SEMANTIC_TAGS[element.tagName.toLowerCase()] === true || element.getAttribute("role") !== null;
  }

  function isContainer(element) {
    var role = element.getAttribute("role") || "";
    var tag = element.tagName.toLowerCase();
    var id = (element.id || "").toLowerCase();
    var className = compactWhitespace(typeof element.className === "string" ? element.className : "").toLowerCase();

    if (
      role === "search" ||
      role === "form" ||
      role === "group" ||
      role === "toolbar" ||
      role === "navigation" ||
      tag === "form" ||
      tag === "fieldset" ||
      tag === "nav"
    ) {
      return true;
    }

    for (var i = 0; i < FUNCTIONAL_KEYWORDS.length; i++) {
      var keyword = FUNCTIONAL_KEYWORDS[i];
      if (id.indexOf(keyword) !== -1 || className.indexOf(keyword) !== -1) {
        return true;
      }
    }

    return false;
  }

  function shouldInclude(element) {
    var tag = element.tagName.toLowerCase();
    if (SKIPPED_TAGS[tag] || element.getAttribute("aria-hidden") === "true") {
      return false;
    }
    if (!isVisible(element)) {
      return false;
    }
    if (filterType !== "all" && !isInViewport(element)) {
      return false;
    }
    if (filterType === "interactive") {
      return isInteractive(element);
    }
    if (isInteractive(element) || isSemantic(element)) {
      return true;
    }

    var cleanName = getName(element);
    if (cleanName) {
      return true;
    }

    if (getRole(element) === "generic" && (tag === "div" || tag === "span")) {
      return isContainer(element);
    }

    return isContainer(element);
  }

  function getOrCreateRef(element) {
    var existingRef = window.__yutoriElementIds.get(element);
    if (existingRef && window.__yutoriElementRefs[existingRef] && window.__yutoriElementRefs[existingRef].deref() === element) {
      return existingRef;
    }

    var ref = "ref_" + ++window.__yutoriRefCounter;
    window.__yutoriElementIds.set(element, ref);
    window.__yutoriElementRefs[ref] = new WeakRef(element);
    return ref;
  }

  function quoteAttribute(value) {
    return String(value)
      .replace(/\\/g, "\\\\")
      .replace(/\r/g, " ")
      .replace(/\n/g, " ")
      .replace(/\t/g, " ")
      .replace(/"/g, '\\"');
  }

  function formatLine(element, depth) {
    var role = getRole(element);
    var name = getName(element);
    var line = new Array(depth + 1).join("  ") + "- " + role;

    if (name) {
      line += ' "' + quoteAttribute(compactWhitespace(name).slice(0, 100)) + '"';
    }

    line += " [ref=" + getOrCreateRef(element) + "]";

    if (element.id) {
      line += ' id="' + quoteAttribute(element.id) + '"';
    }
    if (element.getAttribute("href")) {
      line += ' href="' + quoteAttribute(element.getAttribute("href")) + '"';
    }
    if (element.getAttribute("type")) {
      line += ' type="' + quoteAttribute(element.getAttribute("type")) + '"';
    }
    if (element.getAttribute("placeholder")) {
      line += ' placeholder="' + quoteAttribute(element.getAttribute("placeholder")) + '"';
    }

    return line;
  }

  function walk(element, depth, output) {
    if (!element || !element.tagName || depth > MAX_DEPTH) {
      return;
    }

    var includeHere = depth === 0 || shouldInclude(element);
    if (includeHere) {
      output.push(formatLine(element, depth));
    }

    if (!element.children || depth >= MAX_DEPTH) {
      return;
    }

    // Preserve the current indentation level when skipping a purely structural node.
    var childDepth = includeHere ? depth + 1 : depth;
    for (var i = 0; i < element.children.length; i++) {
      walk(element.children[i], childDepth, output);
    }
  }

  function pruneDeadRefs() {
    // WeakRefs may outlive detached nodes in the map until we sweep them explicitly.
    for (var ref in window.__yutoriElementRefs) {
      if (!window.__yutoriElementRefs[ref].deref()) {
        delete window.__yutoriElementRefs[ref];
      }
    }
  }

  ensureStore();

  var lines = [];
  if (document.body) {
    walk(document.body, 0, lines);
  }
  pruneDeadRefs();

  var filteredLines = lines.filter(function (line) {
    return !/^\s*- generic \[ref=ref_\d+\]$/.test(line);
  });

  return {
    success: true,
    pageContent: filteredLines.join("\n"),
    totalLines: filteredLines.length,
  };
})
