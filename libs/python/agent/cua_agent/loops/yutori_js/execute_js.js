(async function (source) {
  try {
    var AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;

    // Try expression-style first (e.g. "document.title", "2 + 2") by wrapping
    // in "return (...)".  If that fails to parse, fall back to body-style
    // (e.g. multi-statement code with its own return).
    var fn;
    try {
      fn = new AsyncFunction("return (" + source + ")");
    } catch (e) {
      fn = new AsyncFunction(source);
    }

    var value = await fn();

    if (value === undefined) {
      return {
        success: true,
        hasResult: false,
        result: null,
      };
    }

    if (typeof value === "string") {
      return {
        success: true,
        hasResult: true,
        result: value,
      };
    }

    try {
      return {
        success: true,
        hasResult: true,
        result: JSON.stringify(value),
      };
    } catch (error) {
      return {
        success: true,
        hasResult: true,
        result: String(value),
      };
    }
  } catch (error) {
    return {
      success: false,
      message: "Error executing JavaScript: " + (error.message || "Unknown error"),
    };
  }
})
