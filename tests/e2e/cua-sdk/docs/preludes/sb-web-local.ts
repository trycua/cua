// Hidden docs prelude `sb-web-local`: the sandbox `sb` a page's earlier section
// started, serving HTTP on the service `web` (port 8000), on this machine (container lane).
import {
  embedded as __cuaDocsEmbedded,
  http as __cuaDocsHttp,
  SandboxCreateOptions as __cuaDocsOptions,
} from '@trycua/cua';

const sb = await __cuaDocsEmbedded().sandboxes().create(
  __cuaDocsOptions.create({
    on: "local",
    image: 'python:3.12-slim',
    command: ['python', '-m', 'http.server', '8000'],
    services: new Map([['web', 8000]]),
    waitFor: [__cuaDocsHttp('web', '/')],
  })
);
