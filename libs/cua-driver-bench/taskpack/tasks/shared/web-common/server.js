const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const args = process.argv.slice(2);
const workspace = args[args.indexOf('--workspace') + 1];
const bundle = args[args.indexOf('--bundle') + 1];
const trial = JSON.parse(fs.readFileSync(path.join(workspace, '.runtime/trial.json')));
const state = {
  schema_version: 1,
  task: trial.task,
  seed: trial.seed,
  variant_id: trial.variant_id,
};
const app = require(path.resolve(bundle, 'fixture/app.js'))(state, trial.values);
const send = (res, status, data, type = 'application/json') => {
  res.writeHead(status, { 'Content-Type': type, 'Cache-Control': 'no-store' });
  res.end(type === 'application/json' ? JSON.stringify(data) : data);
};
const server = http.createServer(async (req, res) => {
  if (req.method === 'GET' && req.url === '/healthz') return send(res, 200, { ok: true });
  if (req.method === 'GET' && req.url === '/__state') return send(res, 200, state);
  if (req.method === 'GET' && req.url === '/client.js') {
    return send(res, 200, fs.readFileSync(path.join(__dirname, 'client.js')), 'text/javascript');
  }
  if (req.method === 'GET' && req.url === '/') return send(res, 200, app.html, 'text/html');
  if (req.method !== 'POST') return send(res, 404, { error: 'Not found' });
  try {
    let text = '';
    for await (const chunk of req) {
      text += chunk;
      if (text.length > 16384) return send(res, 413, { error: 'Request too large' });
    }
    const response = app.submit(req.url, JSON.parse(text));
    send(res, response.errors ? 422 : 200, response);
  } catch {
    send(res, 400, { errors: { request: 'Send a valid form object' } });
  }
});
server.listen(Number(new URL(trial.base_url).port), '127.0.0.1', () => {
  console.log(`LISTENING ${server.address().port}`);
});
