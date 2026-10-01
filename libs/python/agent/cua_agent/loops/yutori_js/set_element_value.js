(function (elementRef, inputValue) {
  function response(success, payload) {
    var result = {
      success: success,
      action: "set_element_value",
    };
    for (var key in payload) {
      result[key] = payload[key];
    }
    return result;
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

  function isInViewport(el) {
    var rect = el.getBoundingClientRect();
    return rect.top < window.innerHeight && rect.bottom > 0 && rect.left < window.innerWidth && rect.right > 0;
  }

  function ensureVisible(el) {
    if (!isInViewport(el)) {
      var htmlEl = document.documentElement;
      var bodyEl = document.body;
      var prevHtml = htmlEl.style.scrollBehavior;
      var prevBody = bodyEl ? bodyEl.style.scrollBehavior : "";
      try {
        // Neutralize page-level smooth scrolling so form interactions happen against stable layout.
        htmlEl.style.scrollBehavior = "auto";
        if (bodyEl) bodyEl.style.scrollBehavior = "auto";

        el.scrollIntoView({ behavior: "instant", block: "center", inline: "center" });
        el.offsetHeight;
      } finally {
        htmlEl.style.scrollBehavior = prevHtml;
        if (bodyEl) bodyEl.style.scrollBehavior = prevBody;
      }
    }
  }

  // Use the native setter to bypass framework interception (React, Vue, etc.)
  // then dispatch events that frameworks actually listen to.
  function setNativeValue(el, value) {
    var prototype = null;
    if (el instanceof HTMLTextAreaElement) {
      prototype = HTMLTextAreaElement.prototype;
    } else if (el instanceof HTMLInputElement) {
      prototype = HTMLInputElement.prototype;
    }

    var descriptor = prototype ? Object.getOwnPropertyDescriptor(prototype, "value") : null;

    if (descriptor && descriptor.set) {
      descriptor.set.call(el, value);
    } else {
      el.value = value;
    }
  }

  function emitInputEvents(el) {
    // Only focus if not already focused — avoids unnecessary blur on other fields
    if (document.activeElement !== el) {
      el.focus();
    }
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
  }

  function selectOption(element, rawValue) {
    var targetValue = String(rawValue);

    for (var i = 0; i < element.options.length; i++) {
      var option = element.options[i];
      if (option.value === targetValue || option.text === targetValue) {
        element.selectedIndex = i;
        emitInputEvents(element);
        return response(true, {
          message: 'Selected option "' + targetValue + '" in dropdown',
        });
      }
    }

    var optionDescriptions = [];
    for (var j = 0; j < element.options.length; j++) {
      optionDescriptions.push('"' + element.options[j].text + '" (value: "' + element.options[j].value + '")');
    }

    return response(false, {
      message: 'Option "' + targetValue + '" not found. Available options: ' + optionDescriptions.join(", "),
    });
  }

  function updateCheckbox(element, rawValue) {
    if (typeof rawValue !== "boolean" && rawValue !== "true" && rawValue !== "false") {
      return response(false, {
        message: "Checkbox requires a boolean value (true/false)",
      });
    }

    var desiredState = rawValue === true || rawValue === "true";
    // Prefer the native click path for toggles so app frameworks observe the change.
    if (element.checked !== desiredState) {
      element.click();
    }

    if (element.checked !== desiredState) {
      return response(false, {
        message: "Checkbox state did not change as requested",
      });
    }

    return response(true, {
      message: "Checkbox " + (element.checked ? "checked" : "unchecked"),
    });
  }

  function updateRadio(element) {
    // Radios are best driven through their native interaction path.
    if (!element.checked) {
      element.click();
    }

    if (!element.checked) {
      return response(false, {
        message: "Radio button could not be selected",
      });
    }

    return response(true, {
      message: "Radio button selected" + (element.name ? ' in group "' + element.name + '"' : ""),
    });
  }

  function updateNumeric(element, kind, rawValue) {
    var asNumber = Number(rawValue);
    if (isNaN(asNumber) && !(kind === "number" && rawValue === "")) {
      return response(false, {
        message: (kind === "range" ? "Range" : "Number") + " input requires a numeric value",
      });
    }

    setNativeValue(element, kind === "range" ? String(asNumber) : String(rawValue));
    emitInputEvents(element);

    return response(true, {
      message:
        kind === "range"
          ? "Set range to " + element.value + " (min: " + element.min + ", max: " + element.max + ")"
          : "Set number input to " + element.value,
    });
  }

  function updateTextLike(element, elementType, rawValue) {
    setNativeValue(element, String(rawValue));

    if (typeof element.setSelectionRange === "function") {
      try {
        element.setSelectionRange(element.value.length, element.value.length);
      } catch (e) {
        // Some input types (email, number) don't support setSelectionRange
      }
    }

    emitInputEvents(element);

    return response(true, {
      message: "Set " + elementType + ' value to "' + element.value + '"',
    });
  }

  try {
    var element = getTrackedElement(elementRef);
    if (!element) {
      return response(false, {
        message: 'No element found with reference: "' + elementRef + '". The element may have been removed from the page.',
      });
    }

    ensureVisible(element);

    if (element instanceof HTMLSelectElement) {
      return selectOption(element, inputValue);
    }

    if (element instanceof HTMLInputElement) {
      var type = (element.type || "text").toLowerCase();

      if (type === "checkbox") {
        return updateCheckbox(element, inputValue);
      }
      if (type === "radio") {
        return updateRadio(element);
      }
      if (type === "date" || type === "time" || type === "datetime-local" || type === "month" || type === "week") {
        return updateTextLike(element, type, inputValue);
      }
      if (type === "range" || type === "number") {
        return updateNumeric(element, type, inputValue);
      }
      return updateTextLike(element, type || "text", inputValue);
    }

    if (element instanceof HTMLTextAreaElement) {
      return updateTextLike(element, "textarea", inputValue);
    }

    return response(false, {
      message: 'Element type "' + element.tagName + '" is not a supported form input',
    });
  } catch (error) {
    return response(false, {
      message: "Error setting element value: " + (error.message || "Unknown error"),
    });
  }
})
