(function (rawNeedle) {
  var MAX_MATCHES = 20;
  var SKIPPED_TAGS = {
    html: true,
    body: true,
    head: true,
    script: true,
    style: true,
    meta: true,
    link: true,
    title: true,
    noscript: true,
  };

  function failure(message) {
    return {
      success: false,
      message: message,
      totalMatches: 0,
      matches: [],
      pageContent: "",
    };
  }

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

  function getRole(element) {
    var explicitRole = element.getAttribute("role");
    if (explicitRole) {
      return explicitRole;
    }

    var tag = element.tagName.toLowerCase();
    var tagRoles = {
      a: "link",
      button: "button",
      input: "textbox",
      select: "combobox",
      textarea: "textbox",
      h1: "heading",
      h2: "heading",
      h3: "heading",
      h4: "heading",
      h5: "heading",
      h6: "heading",
    };

    return tagRoles[tag] || tag;
  }

  function getName(element) {
    var candidates = [
      element.getAttribute("aria-label"),
      element.getAttribute("placeholder"),
      element.getAttribute("title"),
      element.value,
      element.textContent,
    ];
    for (var i = 0; i < candidates.length; i += 1) {
      var candidate = compactWhitespace(candidates[i] || "");
      if (candidate) {
        return candidate;
      }
    }
    return "";
  }

  function describe(element) {
    var role = getRole(element);
    var name = getName(element);
    var ref = getOrCreateRef(element);
    return "- " + role + (name ? ' "' + name.slice(0, 120) + '"' : "") + " [ref=" + ref + "]";
  }

  function shouldInspect(element) {
    var tag = element.tagName.toLowerCase();
    return !SKIPPED_TAGS[tag] && isVisible(element);
  }

  function scrollIntoView(element) {
    var htmlElement = document.documentElement;
    var bodyElement = document.body;
    var previousHtmlScrollBehavior = htmlElement.style.scrollBehavior;
    var previousBodyScrollBehavior = bodyElement ? bodyElement.style.scrollBehavior : "";
    try {
      htmlElement.style.scrollBehavior = "auto";
      if (bodyElement) bodyElement.style.scrollBehavior = "auto";
      element.scrollIntoView({ behavior: "instant", block: "center", inline: "center" });
      element.offsetHeight;
    } finally {
      htmlElement.style.scrollBehavior = previousHtmlScrollBehavior;
      if (bodyElement) bodyElement.style.scrollBehavior = previousBodyScrollBehavior;
    }
  }

  var needle = compactWhitespace(String(rawNeedle || "")).toLowerCase();
  if (!needle) {
    return failure("find requires non-empty text");
  }

  ensureStore();

  var matches = [];
  var firstMatch = null;
  var nodes = document.querySelectorAll("*");
  for (var i = 0; i < nodes.length; i += 1) {
    var element = nodes[i];
    if (!shouldInspect(element)) {
      continue;
    }

    var haystack = compactWhitespace(
      [
        element.textContent || "",
        element.getAttribute("aria-label") || "",
        element.getAttribute("placeholder") || "",
        element.getAttribute("title") || "",
        element.value || "",
      ].join(" "),
    ).toLowerCase();

    if (!haystack.includes(needle)) {
      continue;
    }

    if (!firstMatch) {
      firstMatch = element;
    }
    matches.push(describe(element));
    if (matches.length >= MAX_MATCHES) {
      break;
    }
  }

  if (!matches.length) {
    return {
      success: true,
      totalMatches: 0,
      matches: [],
      message: 'No visible matches found for "' + rawNeedle + '".',
      pageContent: "",
    };
  }

  if (firstMatch) {
    scrollIntoView(firstMatch);
  }

  return {
    success: true,
    totalMatches: matches.length,
    matches: matches,
    message: 'Found ' + matches.length + ' visible match' + (matches.length === 1 ? "" : "es") + ' for "' + rawNeedle + '".',
    pageContent: matches.join("\n"),
  };
})
