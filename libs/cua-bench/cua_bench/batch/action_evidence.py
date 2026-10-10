"""Common desktop action evidence boundary for benchmark agents.

Wraps the session supplied to any agent without changing the concrete provider.
"""
import hashlib
import json
from datetime import datetime, timezone


class EvidenceSession:
    def __init__(self, session, output_dir, execution_id):
        self._session = session
        self._path = output_dir / "executed-actions.jsonl"
        self._execution_id = execution_id
        self._sequence = 0

    def __getattr__(self, name):
        return getattr(self._session, name)

    async def _observation(self):
        try:
            data = await self._session.screenshot()
            return {"sha256": hashlib.sha256(data).hexdigest(), "size_bytes": len(data)}
        except Exception as exc:
            return {"error_type": type(exc).__name__}

    async def execute_action(self, action):
        from cua_bench.actions import action_to_dict

        self._sequence += 1
        before = await self._observation()
        payload = action_to_dict(action)
        if "text" in payload:
            payload["text_length"] = len(payload.pop("text"))
            payload["text_redacted"] = True
        error = None
        try:
            return await self._session.execute_action(action)
        except Exception as exc:
            error = type(exc).__name__
            raise
        finally:
            after = await self._observation()
            event = {
                "schema_version": "cua-bench-executed-action/v1",
                "execution_id": self._execution_id,
                "sequence": self._sequence,
                "timestamp": datetime.now(timezone.utc).isoformat(),
                "action": payload,
                "before": before,
                "after": after,
                "execution_status": "failed" if error else "returned",
                "error_type": error,
            }
            with self._path.open("a", encoding="utf-8") as stream:
                stream.write(json.dumps(event, sort_keys=True) + "\n")

    # High-level DesktopSession shortcuts bypass execute_action on the wrapped
    # session; route them through this adapter to avoid silently missing actions.
    async def step(self, action):
        return await self.execute_action(action)

    async def click(self, x, y):
        from cua_bench.types import ClickAction
        return await self.execute_action(ClickAction(x=x, y=y))

    async def right_click(self, x, y):
        from cua_bench.types import RightClickAction
        return await self.execute_action(RightClickAction(x=x, y=y))

    async def double_click(self, x, y):
        from cua_bench.types import DoubleClickAction
        return await self.execute_action(DoubleClickAction(x=x, y=y))

    async def type(self, text):
        from cua_bench.types import TypeAction
        return await self.execute_action(TypeAction(text=text))

    async def key(self, key):
        from cua_bench.types import KeyAction
        return await self.execute_action(KeyAction(key=key))

    async def hotkey(self, keys):
        from cua_bench.types import HotkeyAction
        return await self.execute_action(HotkeyAction(keys=keys))

    async def scroll(self, direction="down", amount=300):
        from cua_bench.types import ScrollAction
        return await self.execute_action(ScrollAction(direction=direction, amount=amount))

    async def move_to(self, x, y):
        from cua_bench.types import MoveToAction
        return await self.execute_action(MoveToAction(x=x, y=y))

    async def drag(self, from_x, from_y, to_x, to_y):
        from cua_bench.types import DragAction
        return await self.execute_action(DragAction(
            from_x=from_x, from_y=from_y, to_x=to_x, to_y=to_y
        ))

    async def _record_direct(self, method, *args):
        # Element-based interactions do not map to pixel Actions, but they
        # still mutate the real desktop and must be included in the evidence.
        self._sequence += 1
        before = await self._observation()
        error = None
        try:
            return await getattr(self._session, method)(*args)
        except Exception as exc:
            error = type(exc).__name__
            raise
        finally:
            after = await self._observation()
            event = {
                "schema_version": "cua-bench-executed-action/v1",
                "execution_id": self._execution_id,
                "sequence": self._sequence,
                "timestamp": datetime.now(timezone.utc).isoformat(),
                "action": {"type": method, "selector_sha256": hashlib.sha256(
                    str(args[1]).encode("utf-8")
                ).hexdigest()},
                "before": before,
                "after": after,
                "execution_status": "failed" if error else "returned",
                "error_type": error,
            }
            with self._path.open("a", encoding="utf-8") as stream:
                stream.write(json.dumps(event, sort_keys=True) + "\n")

    async def click_element(self, pid, selector):
        return await self._record_direct("click_element", pid, selector)

    async def right_click_element(self, pid, selector):
        return await self._record_direct("right_click_element", pid, selector)
