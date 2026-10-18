// docs: test="container"
import { Image, SandboxCreateOptions, embedded, http } from '@trycua/cua';

const cua = embedded();
const sb = await cua.sandboxes().create(
  SandboxCreateOptions.create({
    on: 'local', // or 'cloud'
    image: Image.linux(),
    command: ['python3', '-m', 'http.server', '8000'],
    services: new Map([['web', 8000]]),
    waitFor: [http('web', '/')],
  })
);
const res = await sb.service('web').request('GET', '/', undefined, undefined, undefined);
console.log(res.status);
await sb.delete_();
