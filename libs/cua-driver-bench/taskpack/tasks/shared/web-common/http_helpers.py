import json
import select
import shutil
import subprocess
import sys
import tempfile
import urllib.error
import urllib.request
from pathlib import Path


class WebTest:
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.workspace = Path(self.tmp.name) / "workspace"
        self.server = None
        self.addCleanup(self.stop)
        self.restart()

    def run_script(self, path, *args, ok=True, cwd=None):
        done = subprocess.run(
            [sys.executable, str(path), *map(str, args)],
            capture_output=True,
            text=True,
            timeout=15,
            cwd=cwd,
        )
        if ok:
            self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        return done

    def stop(self):
        if self.server:
            self.server.terminate()
            try:
                self.server.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.server.kill()
                self.server.wait(timeout=5)
            self.server.stdout.close()
            self.server = None

    def restart(self, seed=0):
        self.stop()
        self.run_script(
            self.bundle / "reset/setup.py",
            "--workspace",
            self.workspace,
            "--seed",
            seed,
            "--port",
            0,
        )
        self.run_script(
            self.bundle / "reset/verify.py", "--workspace", self.workspace, "--seed", seed
        )
        trial_file = self.workspace / ".runtime/trial.json"
        trial = json.loads(trial_file.read_text())
        self.expected = self.variants[seed]
        self.assertEqual(trial["values"], self.expected)
        self.server = subprocess.Popen(
            [
                "node",
                str(self.bundle.parent / "web-common/server.js"),
                "--workspace",
                str(self.workspace),
                "--bundle",
                str(self.bundle),
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        ready, _, _ = select.select([self.server.stdout], [], [], 10)
        self.assertTrue(ready, "server startup timed out")
        line = self.server.stdout.readline().strip()
        self.assertTrue(line.startswith("LISTENING "), line)
        self.base_url = "http://127.0.0.1:" + line.split()[1]
        trial["base_url"] = self.base_url
        trial_file.write_text(json.dumps(trial))
        self.assertEqual(self.request("/healthz")[0], 200)

    def request(self, route, data=None):
        request = urllib.request.Request(
            self.base_url + route,
            data=None if data is None else json.dumps(data).encode(),
            headers={"Content-Type": "application/json"},
        )
        try:
            response = urllib.request.urlopen(request, timeout=5)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            return response.status, json.load(response)

    def client_submit(self, fields):
        done = subprocess.run(
            [
                "node",
                "-e",
                """
const fs = require('node:fs');
const vm = require('node:vm');
const fields = JSON.parse(process.argv[1]);
const errors = { textContent: '' };
let submit;
let payload = null;
const form = {
  dataset: {}, action: '/submit',
  elements: { namedItem: (name) => fields.find((field) => field.name === name) },
  querySelector: (selector) => selector === '[role=alert]' ? errors : {},
  querySelectorAll: () => fields.filter((field) => field.dataset.confirm),
  addEventListener: (_, handler) => { submit = handler; },
};
vm.runInNewContext(fs.readFileSync(process.argv[2], 'utf8'), {
  document: { querySelectorAll: () => [form] },
  FormData: class {
    constructor() { return fields.map(({ name, value }) => [name, value]); }
  },
  fetch: async (_, options) => {
    payload = JSON.parse(options.body);
    return { ok: true, json: async () => ({}) };
  },
});
submit({ preventDefault() {} }).then(() => {
  console.log(JSON.stringify({ payload, error: errors.textContent }));
});
""",
                json.dumps(fields),
                str(self.bundle.parent / "web-common/client.js"),
            ],
            capture_output=True,
            text=True,
            timeout=10,
        )
        self.assertEqual(done.returncode, 0, done.stderr)
        return json.loads(done.stdout)

    def verdict(self, bundle=None, cwd=None):
        output = Path(self.tmp.name) / "result.json"
        self.run_script(
            (bundle or self.bundle) / "evaluator/evaluate.py",
            "--workspace",
            self.workspace,
            "--result",
            output,
            cwd=cwd,
        )
        return json.loads(output.read_text())

    def check_integrity_and_hidden_layout(self):
        self.run_script(self.bundle / "tools/refresh_manifest_digests.py", "--check")
        isolated = Path(self.tmp.name) / self.bundle.name
        shutil.copytree(self.bundle, isolated, ignore=shutil.ignore_patterns("__pycache__"))
        self.assertFalse((isolated.parent / "web-common").exists())
        self.assertFalse(self.verdict(bundle=isolated, cwd=self.tmp.name)["passed"])
        if self.bundle.name == "web-01":
            self.submit()
        else:
            self.checkout()
        self.assertTrue(self.verdict(bundle=isolated, cwd=self.tmp.name)["passed"])
        shared = isolated.parent / "web-common"
        shutil.copytree(self.bundle.parent / "web-common", shared)
        with (shared / "client.js").open("a") as file:
            file.write("\n ")
        done = self.run_script(
            isolated / "reset/verify.py", "--workspace", self.workspace, "--seed", 0, ok=False
        )
        self.assertNotEqual(done.returncode, 0)
        self.assertIn("digest mismatch", done.stderr)
        done = self.run_script(isolated / "tools/refresh_manifest_digests.py", "--check", ok=False)
        self.assertNotEqual(done.returncode, 0)
        self.assertIn("client.js", done.stdout)
