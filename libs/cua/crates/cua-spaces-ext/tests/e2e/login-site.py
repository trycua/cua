# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Tiny login site for the site-login e2e: GET / shows the form; POST /login
# with the right creds sets a cookie and shows "Welcome <user>"; GET /status
# reports the last result for the test to verify in the guest.
import http.server, urllib.parse, json, sys
USER, PW = sys.argv[1], sys.argv[2]
STATE = {"signed_in": None, "attempts": 0}
FORM = b"""<!doctype html><html><head><title>Example login</title></head><body>
<h1>Sign in</h1><form method="post" action="/login">
<label>Email <input type="email" name="username" autocomplete="username" id="username"></label>
<label>Password <input type="password" name="password" autocomplete="current-password" id="password"></label>
<button type="submit">Sign in</button></form></body></html>"""
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def send(self, code, body, ctype="text/html", extra=()):
        self.send_response(code); self.send_header("content-type", ctype)
        for k, v in extra: self.send_header(k, v)
        self.end_headers(); self.wfile.write(body)
    def do_GET(self):
        if self.path == "/status":
            return self.send(200, json.dumps(STATE).encode(), "application/json")
        c = self.headers.get("cookie") or ""
        if "session=ok" in c:
            return self.send(200, f"<h1>Welcome {STATE['signed_in']}</h1>".encode())
        self.send(200, FORM)
    def do_POST(self):
        n = int(self.headers.get("content-length") or 0)
        f = urllib.parse.parse_qs(self.rfile.read(n).decode())
        STATE["attempts"] += 1
        u, p = f.get("username", [""])[0], f.get("password", [""])[0]
        if u == USER and p == PW:
            STATE["signed_in"] = u
            return self.send(200, f"<html><head><title>Welcome</title></head><body><h1>Welcome {u}</h1></body></html>".encode(), extra=[("set-cookie", "session=ok; Path=/")])
        self.send(401, b"<h1>Wrong username or password</h1>")
http.server.ThreadingHTTPServer(("127.0.0.1", 8000), H).serve_forever()
