"""A tiny form page for the browse e2e: GET / is a form, GET /submit echoes it."""

import html
import http.server
import sys
import urllib.parse

FORM = """<!doctype html><html><head><title>Cua form</title></head><body>
<h1>Sign up</h1>
<form action="/submit" method="get">
<label for="name">Name</label> <input id="name" name="name" type="text">
<label><input id="agree" name="agree" type="checkbox" value="yes"> I agree</label>
<button id="go" type="submit">Submit</button>
</form></body></html>"""


class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        u = urllib.parse.urlparse(self.path)
        if u.path == "/submit":
            q = urllib.parse.parse_qs(u.query)
            name = html.escape(q.get("name", [""])[0])
            agree = "yes" if q.get("agree") == ["yes"] else "no"
            body = f"<!doctype html><title>Thanks</title><h1>Thanks, {name}</h1><p id=agree>agree={agree}</p>"
        else:
            body = FORM
        b = body.encode()
        self.send_response(200)
        self.send_header("content-type", "text/html; charset=utf-8")
        self.send_header("content-length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)

    def log_message(self, *a):
        pass


http.server.ThreadingHTTPServer(
    ("127.0.0.1", int(sys.argv[1]) if len(sys.argv) > 1 else 8000), H
).serve_forever()
