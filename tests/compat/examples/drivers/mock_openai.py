"""A tiny OpenAI-compatible endpoint (Chat Completions and Responses) for
examples that only need an LLM to take a scripted turn. Written by us.

Behaviour, per request:
  - the request offers tools and no tool result is in the conversation yet:
    answer with ONE call of the first tool, arguments filled from its schema
    (strings named cmd/command get "ls -la", others "x");
  - a tool result is already in the conversation: answer with the final text
    "mock-llm: done." and log the tool result to $MOCK_TRACE (default
    mock-trace.log) as `TOOL_OUTPUT: <text>` so the harness can assert that
    the tool really ran in the sandbox;
  - no tools offered: the final text.
Also /v1/models. Usage: python mock_openai.py PORT"""

import json
import os
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

REPLY = "mock-llm: done."
TRACE = os.environ.get("MOCK_TRACE", "mock-trace.log")


def trace(line):
    with open(TRACE, "a") as f:
        f.write(line.replace("\n", "\\n")[:2000] + "\n")


def fill(schema):
    props = (schema or {}).get("properties", {})
    req = (schema or {}).get("required", list(props))
    args = {}
    for k in req:
        t = props.get(k, {}).get("type", "string")
        if t == "string":
            args[k] = "ls -la" if k in ("cmd", "command") else "x"
        elif t in ("integer", "number"):
            args[k] = 1
        elif t == "boolean":
            args[k] = False
        elif t == "array":
            args[k] = []
        elif t == "object":
            args[k] = {}
    return args


def shell_tool(req):
    return any(t.get("type") == "shell" for t in req.get("tools") or [])


def first_tool(req):
    for t in req.get("tools") or []:
        if t.get("type") == "function":
            fn = t.get("function", t)  # chat nests under "function", responses is flat
            return fn.get("name"), fn.get("parameters")
    return None, None


def tool_results(req):
    out = []
    for it in req.get("input") or []:
        if isinstance(it, dict) and it.get("type") == "function_call_output":
            out.append(str(it.get("output")))
        if isinstance(it, dict) and it.get("type") in (
            "shell_call_output",
            "local_shell_call_output",
        ):
            out.append(json.dumps(it.get("output")))
    for m in req.get("messages") or []:
        if m.get("role") == "tool":
            out.append(str(m.get("content")))
    return out


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _send(self, code, body, ctype="application/json"):
        data = body if isinstance(body, bytes) else json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path.rstrip("/").endswith("/models"):
            self._send(200, {"object": "list", "data": [{"id": "mock", "object": "model"}]})
        else:
            self._send(200, {"ok": True})

    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        try:
            req = json.loads(self.rfile.read(n) or b"{}")
        except Exception:
            req = {}
        model = req.get("model", "mock")
        now = int(time.time())
        trace(
            "REQ "
            + self.path
            + " tools="
            + json.dumps(
                [
                    (t.get("type"), (t.get("function") or t).get("name"))
                    for t in req.get("tools") or []
                ]
            )
            + " tool_choice="
            + json.dumps(req.get("tool_choice"))
        )
        results = tool_results(req)
        name, params = first_tool(req)
        call = bool(name) and not results
        for r in results:
            trace("TOOL_OUTPUT: " + r)
        if self.path.rstrip("/").endswith("/responses"):
            if shell_tool(req) and not results:
                out = [
                    {
                        "type": "shell_call",
                        "id": "sc_mock",
                        "call_id": "call_mock",
                        "status": "completed",
                        "action": {
                            "commands": ["ls -la"],
                            "timeout_ms": 30000,
                            "max_output_length": 10000,
                        },
                    }
                ]
            elif call:
                out = [
                    {
                        "type": "function_call",
                        "id": "fc_mock",
                        "call_id": "call_mock",
                        "name": name,
                        "arguments": json.dumps(fill(params)),
                        "status": "completed",
                    }
                ]
            else:
                out = [
                    {
                        "type": "message",
                        "id": "msg_mock",
                        "role": "assistant",
                        "status": "completed",
                        "content": [{"type": "output_text", "text": REPLY, "annotations": []}],
                    }
                ]
            body = {
                "id": "resp_mock",
                "object": "response",
                "created_at": now,
                "model": model,
                "status": "completed",
                "output": out,
                "parallel_tool_calls": False,
                "tool_choice": "auto",
                "tools": [],
                "usage": {
                    "input_tokens": 1,
                    "output_tokens": 1,
                    "total_tokens": 2,
                    "input_tokens_details": {"cached_tokens": 0},
                    "output_tokens_details": {"reasoning_tokens": 0},
                },
            }
            if req.get("stream"):
                ev = [
                    {
                        "type": "response.created",
                        "response": {**body, "status": "in_progress", "output": []},
                        "sequence_number": 0,
                    }
                ]
                for i, item in enumerate(out):
                    ev.append(
                        {
                            "type": "response.output_item.added",
                            "output_index": i,
                            "item": item,
                            "sequence_number": len(ev),
                        }
                    )
                    if item["type"] == "message":
                        part = item["content"][0]
                        ev.append(
                            {
                                "type": "response.content_part.added",
                                "item_id": item["id"],
                                "output_index": i,
                                "content_index": 0,
                                "part": {**part, "text": ""},
                                "sequence_number": len(ev),
                            }
                        )
                        ev.append(
                            {
                                "type": "response.output_text.delta",
                                "item_id": item["id"],
                                "output_index": i,
                                "content_index": 0,
                                "delta": part["text"],
                                "sequence_number": len(ev),
                                "logprobs": [],
                            }
                        )
                        ev.append(
                            {
                                "type": "response.output_text.done",
                                "item_id": item["id"],
                                "output_index": i,
                                "content_index": 0,
                                "text": part["text"],
                                "sequence_number": len(ev),
                                "logprobs": [],
                            }
                        )
                        ev.append(
                            {
                                "type": "response.content_part.done",
                                "item_id": item["id"],
                                "output_index": i,
                                "content_index": 0,
                                "part": part,
                                "sequence_number": len(ev),
                            }
                        )
                    else:
                        ev.append(
                            {
                                "type": "response.function_call_arguments.delta",
                                "item_id": item["id"],
                                "output_index": i,
                                "delta": item["arguments"],
                                "sequence_number": len(ev),
                            }
                        )
                        ev.append(
                            {
                                "type": "response.function_call_arguments.done",
                                "item_id": item["id"],
                                "output_index": i,
                                "name": item["name"],
                                "arguments": item["arguments"],
                                "sequence_number": len(ev),
                            }
                        )
                    ev.append(
                        {
                            "type": "response.output_item.done",
                            "output_index": i,
                            "item": item,
                            "sequence_number": len(ev),
                        }
                    )
                ev.append(
                    {"type": "response.completed", "response": body, "sequence_number": len(ev)}
                )
                data = "".join(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in ev)
                self._send(200, data.encode(), "text/event-stream")
            else:
                self._send(200, body)
            return
        # chat completions
        if call:
            msg = {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    {
                        "id": "call_mock",
                        "type": "function",
                        "function": {"name": name, "arguments": json.dumps(fill(params))},
                    }
                ],
            }
            fin = "tool_calls"
        else:
            msg, fin = {"role": "assistant", "content": REPLY}, "stop"
        if req.get("stream"):
            delta = dict(msg)
            if "tool_calls" in delta:
                delta["tool_calls"] = [{"index": 0, **delta["tool_calls"][0]}]
            chunks = [
                {
                    "id": "c1",
                    "object": "chat.completion.chunk",
                    "created": now,
                    "model": model,
                    "choices": [{"index": 0, "delta": delta, "finish_reason": None}],
                },
                {
                    "id": "c1",
                    "object": "chat.completion.chunk",
                    "created": now,
                    "model": model,
                    "choices": [{"index": 0, "delta": {}, "finish_reason": fin}],
                },
            ]
            self._send(
                200,
                (
                    "".join(f"data: {json.dumps(c)}\n\n" for c in chunks) + "data: [DONE]\n\n"
                ).encode(),
                "text/event-stream",
            )
            return
        self._send(
            200,
            {
                "id": "c1",
                "object": "chat.completion",
                "created": now,
                "model": model,
                "choices": [{"index": 0, "message": msg, "finish_reason": fin}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
            },
        )


if __name__ == "__main__":
    ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
