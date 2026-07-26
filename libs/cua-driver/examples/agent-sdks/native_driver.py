"""Perceive, act, and externally verify with the same-process Python SDK."""

from __future__ import annotations

import argparse
import asyncio
import json
from urllib.request import Request, urlopen
from uuid import uuid4

from cua_driver import (
    CaptureScope,
    CuaDriver,
    DesktopScope,
    EndSessionInput,
    GetDesktopStateInput,
    PressKeyInput,
    StartSessionInput,
    TypeTextInput,
)


def fixture_request(url: str, path: str, *, method: str = "GET") -> dict[str, str | None]:
    with urlopen(Request(f"{url.rstrip('/')}{path}", method=method), timeout=2) as response:
        if response.status == 204:
            return {}
        return json.loads(response.read())


async def wait_for_submission(url: str, token: str, timeout: float) -> None:
    deadline = asyncio.get_running_loop().time() + timeout
    while asyncio.get_running_loop().time() < deadline:
        state = await asyncio.to_thread(fixture_request, url, "/state")
        if state.get("submitted") == token:
            return
        await asyncio.sleep(0.1)
    raise TimeoutError("the fixture did not observe the submitted value")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture", default="http://127.0.0.1:8765")
    parser.add_argument("--timeout", type=float, default=10.0)
    return parser.parse_args()


async def main() -> None:
    args = parse_args()
    token = f"cua-{uuid4().hex[:10]}"
    session = f"native-python-{uuid4().hex[:10]}"
    await asyncio.to_thread(fixture_request, args.fixture, "/reset", method="POST")

    driver = CuaDriver.create()
    started = False
    mutation_outcome_unknown = False
    try:
        await asyncio.wait_for(
            driver.start_session(
                StartSessionInput(session=session, capture_scope=CaptureScope.DESKTOP)
            ),
            args.timeout,
        )
        started = True

        before = await asyncio.wait_for(
            driver.get_desktop_state(
                GetDesktopStateInput(session=session, screenshot_out_file=None)
            ),
            args.timeout,
        )
        if before.is_error or not before.images:
            raise RuntimeError(before.text or "desktop capture returned no image")
        print(f"perceived desktop: {before.images[0].mime_type}")

        try:
            typed = await asyncio.wait_for(
                driver.type_text(
                    TypeTextInput(
                        text=token,
                        scope=DesktopScope.DESKTOP,
                        session=session,
                    )
                ),
                args.timeout,
            )
            if typed.is_error:
                raise RuntimeError(typed.text)
            submitted = await asyncio.wait_for(
                driver.press_key(
                    PressKeyInput(
                        key="ENTER",
                        modifiers=None,
                        scope=DesktopScope.DESKTOP,
                        session=session,
                    )
                ),
                args.timeout,
            )
            if submitted.is_error:
                raise RuntimeError(submitted.text)
        except (TimeoutError, ConnectionError, RuntimeError):
            mutation_outcome_unknown = True

        # Never blindly replay a mutation after a timeout or disconnect. Query
        # the independent fixture first; it may have consumed the action.
        await wait_for_submission(args.fixture, token, args.timeout)
        after = await asyncio.wait_for(
            driver.get_desktop_state(
                GetDesktopStateInput(session=session, screenshot_out_file=None)
            ),
            args.timeout,
        )
        if after.is_error or not after.images:
            raise RuntimeError(after.text or "post-action capture returned no image")
        print(f"verified submitted value: {token}")
        if mutation_outcome_unknown:
            print("the driver response was uncertain; the external postcondition resolved it")
    finally:
        try:
            if started:
                await driver.end_session(EndSessionInput(session=session))
        finally:
            await driver.shutdown()


if __name__ == "__main__":
    asyncio.run(main())
