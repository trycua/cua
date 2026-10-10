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
