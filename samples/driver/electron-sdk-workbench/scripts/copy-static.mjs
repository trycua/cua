import { copyFile, mkdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const appDirectory = path.resolve(scriptDirectory, '..');
const rendererSource = path.join(appDirectory, 'src', 'renderer');
const rendererOutput = path.join(appDirectory, 'dist', 'renderer');

await mkdir(rendererOutput, { recursive: true });
await Promise.all([
  copyFile(path.join(rendererSource, 'index.html'), path.join(rendererOutput, 'index.html')),
  copyFile(path.join(rendererSource, 'styles.css'), path.join(rendererOutput, 'styles.css')),
]);
