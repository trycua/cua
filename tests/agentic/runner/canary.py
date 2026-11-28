"""Sign the canary test account in, on the staging realm only.

The CLI and app sign in with the RFC 8628 device code flow, as a person
would; this module plays the person: it opens the verification link, signs
in to Keycloak with the canary's username and password, and approves. It
also signs the runner itself in (client cua-cli) for the relay checks.

Credentials come from CUA_E2E_CANARY_USERNAME / CUA_E2E_CANARY_PASSWORD and
are never printed, logged or passed to the agent.
"""

from __future__ import annotations

import html
import http.cookiejar
import json
import os
import re
import time
import urllib.error
import urllib.parse
import urllib.request

STAGING_ISSUER = "https://auth.staging.cua.ai/realms/cua-staging"
DEVICE_URL = re.compile(re.escape(STAGING_ISSUER) + r"/device[^\s\"'<>]*")
USER_CODE = re.compile(r"\b([A-Z0-9]{4}-[A-Z0-9]{4})\b")


class CanaryError(RuntimeError):
    pass


def credentials() -> tuple[str, str]:
    u, p = os.environ.get("CUA_E2E_CANARY_USERNAME", ""), os.environ.get(
        "CUA_E2E_CANARY_PASSWORD", ""
    )
    if not u or not p:
        raise CanaryError("CUA_E2E_CANARY_USERNAME and CUA_E2E_CANARY_PASSWORD are not set")
    return u, p


def secrets() -> list[str]:
    """Values the recorder must redact if they ever reach a turn."""
    return [v for v in (os.environ.get("CUA_E2E_CANARY_PASSWORD"),) if v]


class _Browser:
    def __init__(self):
        self.opener = urllib.request.build_opener(
            urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar())
        )

    def get(self, url):
        with self.opener.open(url, timeout=30) as r:
            return r.geturl(), r.read().decode("utf-8", "replace")

    def post(self, url, form):
        url = urllib.parse.urljoin(STAGING_ISSUER, url)
        req = urllib.request.Request(url, data=urllib.parse.urlencode(form).encode())
        with self.opener.open(req, timeout=30) as r:
            return r.geturl(), r.read().decode("utf-8", "replace")


def _form(page: str, form_id: str | None = None):
    for attrs, body in re.findall(r"<form\b([^>]*)>(.*?)</form>", page, re.S | re.I):
        if form_id and f'id="{form_id}"' not in attrs:
            continue
        action = re.search(r'action="([^"]+)"', attrs)
        if not action:
            continue
        fields = {}
        for inp in re.findall(r"<input\b([^>]*)>", body, re.I):
            name = re.search(r'name="([^"]+)"', inp)
            value = re.search(r'value="([^"]*)"', inp)
            if name and 'type="submit"' not in inp:
                fields[name.group(1)] = html.unescape(value.group(1)) if value else ""
        return html.unescape(action.group(1)), fields
    return None, None


def approve(verification_url: str) -> None:
    """Act as the browser on a staging device code: sign in, then grant."""
    if not verification_url.startswith(STAGING_ISSUER + "/"):
        raise CanaryError("refusing to approve a device code outside the staging realm")
    username, password = credentials()
    b = _Browser()
    url, page = b.get(verification_url)
    for _ in range(8):
        if "/device/status" in url:
            text = re.sub(r"<[^>]+>", " ", page)
            if re.search(r"(denied|expired|invalid|error)", text, re.I) and not re.search(
                r"success", page, re.I
            ):
                raise CanaryError("device approval ended on an error page")
            return
        action, fields = _form(page, "kc-form-login")
        if action is not None:
            two_step = "password" not in fields and "username" in fields
            fields.update({"username": username, "password": password})
            if two_step:
                fields.pop("password")
            url, page = b.post(action, fields)
            if not two_step and "kc-form-login" in page:
                raise CanaryError("staging sign-in rejected the canary credentials")
            continue
        action, fields = _form(page)
        if action is None:
            raise CanaryError("unexpected sign-in page at " + url.split("?")[0])
        if "accept" in page or "kc-login" in page:
            fields.setdefault("accept", "Yes")
        url, page = b.post(action, fields)
    raise CanaryError("device approval did not finish at " + url.split("?")[0])


def _post(url: str, form: dict) -> tuple[int, dict]:
    req = urllib.request.Request(url, data=urllib.parse.urlencode(form).encode())
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return r.status, json.loads(r.read() or b"{}")
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read() or b"{}")


class Session:
    """The runner's own canary session (client cua-cli), for relay checks."""

    def __init__(self):
        oidc = STAGING_ISSUER + "/protocol/openid-connect"
        status, dev = _post(
            oidc + "/auth/device", {"client_id": "cua-cli", "scope": "openid offline_access"}
        )
        if status != 200:
            raise CanaryError(f"device authorization failed: {status} {dev.get('error')}")
        approve(dev["verification_uri_complete"])
        deadline = time.time() + float(dev.get("expires_in", 600))
        interval = float(dev.get("interval", 5))
        while time.time() < deadline:
            status, tok = _post(
                oidc + "/token",
                {
                    "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
                    "client_id": "cua-cli",
                    "device_code": dev["device_code"],
                },
            )
            if status == 200:
                self._tok = tok
                self._at = time.time()
                self._oidc = oidc
                return
            if tok.get("error") == "slow_down":
                interval += 5
            elif tok.get("error") != "authorization_pending":
                raise CanaryError(f"device token failed: {tok.get('error')}")
            time.sleep(interval)
        raise CanaryError("device code expired before approval")

    def token(self) -> str:
        if time.time() - self._at > float(self._tok.get("expires_in", 300)) - 60:
            status, tok = _post(
                self._oidc + "/token",
                {
                    "grant_type": "refresh_token",
                    "client_id": "cua-cli",
                    "refresh_token": self._tok["refresh_token"],
                },
            )
            if status != 200:
                raise CanaryError(f"token refresh failed: {status}")
            self._tok.update(tok)
            self._at = time.time()
        return self._tok["access_token"]

    def logout(self) -> None:
        rt = self._tok.get("refresh_token")
        if rt:
            _post(self._oidc + "/logout", {"client_id": "cua-cli", "refresh_token": rt})


def relay_request(session: Session, base: str, method: str, path: str) -> tuple[int, object]:
    req = urllib.request.Request(
        base.rstrip("/") + path,
        method=method,
        headers={"Authorization": "Bearer " + session.token(), "User-Agent": "cua-agentic-e2e/1"},
    )
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            body = r.read()
            return r.status, json.loads(body) if body else None
    except urllib.error.HTTPError as e:
        body = e.read()
        try:
            return e.code, json.loads(body) if body else None
        except ValueError:
            return e.code, body.decode("utf-8", "replace")[:500]
