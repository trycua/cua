"""Synthetic guest-local GUI fixture. No external network requests."""
import json
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

STATE = Path('/tmp/mastra-fleet-fixture-state.json')
HTML = b'''<!doctype html><html><head><title>Fleet Mastra Test</title>
<style>body{font:24px sans-serif;margin:0;background:#f3f5f6;color:#18232d}
h1{position:absolute;left:48px;top:24px}label{position:absolute;left:48px;top:125px}
input,button{font:24px sans-serif;padding:16px;position:absolute;box-sizing:border-box}
input{left:48px;top:170px;width:480px;height:64px}
button{left:48px;top:260px;height:64px;background:#135f4b;color:white;border:0}
#result{position:absolute;left:48px;top:350px;font-weight:bold}</style></head><body><h1>Fleet Mastra Test</h1>
<label for="value">Verification value</label><input id="value" autofocus>
<button id="submit">Submit value</button><p id="result">Waiting for submission</p>
<script>document.querySelector('#submit').onclick=async()=>{
const value=document.querySelector('#value').value;
const r=await fetch('/submit',{method:'POST',body:JSON.stringify({value})});
if(r.ok)document.querySelector('#result').textContent='Submitted: '+value;
};</script></body></html>'''

class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header('Content-Type', 'text/html')
        self.end_headers()
        self.wfile.write(HTML)

    def do_POST(self):
        data = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        STATE.write_text(json.dumps(data))
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b'ok')

    def log_message(self, *args):
        pass

HTTPServer(('127.0.0.1', 8765), Handler).serve_forever()
