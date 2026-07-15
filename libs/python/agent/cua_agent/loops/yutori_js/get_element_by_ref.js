(function (elementRef) {
  function failure(message) {
    return {
      success: false,
      action: "get_element_by_ref",
      message: message,
    };
  }

  function getTrackedElement(ref) {
    if (!window.__yutoriElementRefs || !window.__yutoriElementRefs[ref]) {
      return null;
    }

    var weakRef = window.__yutoriElementRefs[ref];
    var element = weakRef.deref();
    if (!element || !document.contains(element)) {
      delete window.__yutoriElementRefs[ref];
      return null;
    }

    return element;
  }

  function isViewportVisible(rect) {
    var viewportWidth = window.innerWidth || document.documentElement.clientWidth;
    var viewportHeight = window.innerHeight || document.documentElement.clientHeight;
    return (
      rect.top < viewportHeight &&
      rect.bottom > 0 &&
      rect.left < viewportWidth &&
      rect.right > 0 &&
      rect.width > 0 &&
      rect.height > 0
    );
  }

  function isPointInViewport(x, y) {
    var viewportWidth = window.innerWidth || document.documentElement.clientWidth;
    var viewportHeight = window.innerHeight || document.documentElement.clientHeight;
    return x >= 0 && x <= viewportWidth && y >= 0 && y <= viewportHeight;
  }

  try {
    var element = getTrackedElement(elementRef);
    if (!element) {
      return failure('No element found with reference: "' + elementRef + '". The element may have been removed from the page.');
    }

    var beforeScrollRect = element.getBoundingClientRect();
    var wasVisibleBeforeScroll = isViewportVisible(beforeScrollRect);
    var centerX = beforeScrollRect.left + beforeScrollRect.width / 2;
    var centerY = beforeScrollRect.top + beforeScrollRect.height / 2;

    var centerInViewport = isPointInViewport(centerX, centerY);

    // Downstream actions click the element center, so partial visibility is not enough.
    if (!wasVisibleBeforeScroll || !centerInViewport) {
      var htmlEl = document.documentElement;
      var bodyEl = document.body;
      var prevHtml = htmlEl.style.scrollBehavior;
      var prevBody = bodyEl ? bodyEl.style.scrollBehavior : "";
      try {
        // Some sites force smooth scrolling globally; temporarily disable it so coordinates settle immediately.
        htmlEl.style.scrollBehavior = "auto";
        if (bodyEl) bodyEl.style.scrollBehavior = "auto";

        element.scrollIntoView({ behavior: "instant", block: "center", inline: "center" });

        // Force layout so getBoundingClientRect reflects the post-scroll position.
        element.offsetHeight;
      } finally {
        htmlEl.style.scrollBehavior = prevHtml;
        if (bodyEl) bodyEl.style.scrollBehavior = prevBody;
      }
    }

    var rect = element.getBoundingClientRect();
    return {
      success: true,
      coordinates: [rect.left + rect.width / 2, rect.top + rect.height / 2],
    };
  } catch (error) {
    return failure("Error finding element by reference: " + (error.message || "Unknown error"));
  }
})
