#!/usr/bin/env python3
"""Invoke one Cua Driver tool through the release-matched Python SDK."""

from __future__ import annotations

import argparse
import asyncio
import json

from cua_driver import CuaDriver


async def invoke(socket_path: str, name: str, arguments_json: str) -> dict:
    driver = CuaDriver.connect(socket_path)
    result = await driver.call_tool(name, arguments_json)
    return {
        "text": result.text,
        "images": [
            {
                "mime_type": image.mime_type,
                "data_base64": image.data_base64,
            }
            for image in result.images
        ],
        "structured_json": result.structured_json,
        "is_error": result.is_error,
        "error_code": result.error_code,
        "verified": result.verified,
        "degraded": result.degraded,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--socket", required=True)
    parser.add_argument("name")
    parser.add_argument("arguments_json")
    args = parser.parse_args()
    print(
        json.dumps(
            asyncio.run(invoke(args.socket, args.name, args.arguments_json)),
            separators=(",", ":"),
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
