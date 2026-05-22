#!/usr/bin/env python3
"""
Headless variant of test_slack.py that injects a 'Hello!' outbound message
so the test can run non-interactively.
"""

import time
from cua_bench_ui import execute_javascript, launch_window

def test_slack_message_api_headless():
    url = "http://localhost:8080"
    # if bench_ui supports headless, pass headless=True; otherwise omit arg
    try:
        pid = launch_window(url=url, title="Slack (headless)", headless=True)
    except TypeError:
        # fallback if launch_window doesn't accept headless
        pid = launch_window(url=url, title="Slack (headless)")

    # Wait for page / API to initialize
    time.sleep(2)

    # Confirm API exists
    api_exists = execute_javascript(pid, "typeof window.__slack_shell !== 'undefined'")
    assert api_exists, "window.__slack_shell API not found (is server running?)"
    print("API found: window.__slack_shell")

    # Inject a synthetic outbound message so the test isn't interactive.
    js_inject = r"""
(function(){
  // Prefer an internal helper if available
  try {
    if (typeof window.__slack_shell._addOutboundMessage === 'function') {
      window.__slack_shell._addOutboundMessage({text: "Hello!", timestamp: Date.now()});
      return true;
    }
  } catch(e) {}

  // Common internal array names used by this UI
  try {
    window.__slack_shell._outboundMessages = window.__slack_shell._outboundMessages || [];
    window.__slack_shell._outboundMessages.push({text: "Hello!", timestamp: Date.now()});
  } catch(e) {}

  // Legacy fallback
  try {
    window.__slack_shell._outbound = window.__slack_shell._outbound || [];
    window.__slack_shell._outbound.push({text: "Hello!", timestamp: Date.now()});
  } catch(e) {}

  return true;
})()
"""

    ok = execute_javascript(pid, js_inject)
    assert ok, "Failed to inject synthetic message"

    # Give the UI a moment to process injected message
    time.sleep(1.0)

    # Robust reader: prefer public getter, then known internals, then frames
    getter_js = (
        "window.__slack_shell && window.__slack_shell.getOutboundMessages ? "
        "window.__slack_shell.getOutboundMessages() : "
        "(window.__slack_shell && (window.__slack_shell._outboundMessages || window.__slack_shell._outbound)) || "
        "Array.from(window.frames).map(f=>f.__slack_shell && (f.__slack_shell._outboundMessages || f.__slack_shell._outbound)).flat().filter(Boolean)"
    )
    msgs = execute_javascript(pid, getter_js) or []

    print(f"Retrieved {len(msgs)} outbound message(s)")

    assert any("Hello!" in str(m.get("text", "")) for m in msgs), f"No Hello! message in {msgs}"
    print("PASSED: Found injected 'Hello!' message")
    return True

if __name__ == '__main__':
    import sys
    res = test_slack_message_api_headless()
    sys.exit(0 if res else 1)
