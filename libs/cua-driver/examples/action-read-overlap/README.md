# Experimental action and read overlap

This caller helper overlaps up to two advisory semantic reads with one already-bound action. It retains the action's full window-change observation and focus-protection period, waits for the action to finish, then takes a mandatory fresh read. Early observations never establish completion or authorize another input. Nothing changes in Driver's tools, defaults, or action timing.

On one synthetic AppKit field, twelve alternating pairs on released Cua Driver 0.34.0 measured 1.2258 s sequential versus 1.2010 s with overlap, about 2% lower latency. The action itself stayed about 1.14 s; the final read fell from 76.9 ms to 61.0 ms. This is a small read-warming experiment for a final field check, not a replacement for the one-second observation horizon. Complete multi-action task speed has not been measured. It uses three observations instead of one and may cost more than it saves on a larger application. The first two pairs were excluded as warm-up. Sanitized measurements are in `evidence/native-2026-10-06.json`; this is macOS evidence, not cross-platform certification.

The tempting alternative of using the early read as final verification failed. An owned AppKit fixture rejected the value 450 ms after the write without opening a window or changing focus. The early read matched in both trials, but the independently observed value was rejected when the action returned. The helper's final read detects that rejection. Quiet samples likewise do not prove that an application has no pending timer; they cannot safely shorten the default observation horizon.

## Use

Create two supported Driver client connections with separate session labels. The actor retains its exact snapshot-bound action arguments. The observer only reads the same explicit PID and window. A named session cannot be shared across transports; Driver correctly refuses that with `session_ended`. The `Driver` wrapper in `../jev-use/python/run.py` provides the usual structured-error handling for MCP clients.

```python
from overlap import action_with_read_warming

# actor and observer are independent Driver clients with distinct sessions.
# bound_arguments came from the actor's fresh observation and are unchanged.
action_result, final_observation = await action_with_read_warming(
    lambda: actor.call("set_value", bound_arguments),
    lambda: observer.call("get_window_state", {
        "pid": pid,
        "window_id": window_id,
        "include_screenshot": False,
    }),
    matches_expected_field,
)
```

Apply the application's ordinary verification policy to both returned results. The helper preserves the complete action result, including delayed-window reports and refusals; it never upgrades an effect verdict. An observer token belongs to the observer session and must not be used for another actor mutation. Observe again through the actor before constructing any subsequent action. A read failure joins the in-flight action and propagates the failure without retrying input. After interruption or process loss, effects remain uncertain and require fresh observation before recovery.

## Offline safety checks

```sh
python3 -m unittest discover -s libs/cua-driver/examples/action-read-overlap -p 'test_*.py' -v
```

The owner test forces an early matching value followed by deferred rejection, preserves a late action report, and requires the final read after the action finishes. Additional checks cover observation failure without mutation retry, cancellation after an early match, and bounded advisory reads. The exact helper also passed three live stable-field cases and three deferred-rejection cases through the public MCP interface; final-read and cancellation-join mutations make the owner checks fail. The helper is not enabled by default in any recipe.
