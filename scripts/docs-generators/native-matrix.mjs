import fs from 'node:fs';
import { fileURLToPath } from 'node:url';

const config = JSON.parse(fs.readFileSync(new URL('./config.json', import.meta.url), 'utf8'));
const rows = [
  { library: 'cua-driver', platform: 'linux', os: 'ubuntu-latest' },
  { library: 'cua-driver', platform: 'macos', os: 'macos-latest' },
  { library: 'cua-driver', platform: 'windows', os: 'windows-latest' },
  { library: 'lume', platform: 'macos', os: 'macos-latest' },
];
const hosts = { linux: 'linux', darwin: 'macos', win32: 'windows' };

export function selectNativeMatrix(files) {
  const selected = new Set();
  const select = (library, platform) => {
    for (const row of rows) {
      if ((!library || row.library === library) && (!platform || row.platform === platform)) {
        selected.add(row);
      }
    }
  };
  for (const file of files) {
    if (
      [
        '.github/workflows/ci-check-docs.yml',
        '.gitattributes',
        'package.json',
        'pnpm-workspace.yaml',
        'docs/package.json',
        'docs/pnpm-lock.yaml',
      ].includes(file)
    ) {
      select();
    } else if (file.startsWith('scripts/docs-generators/')) {
      if (/\/cua-driver(?:[.-])/.test(file)) select('cua-driver');
      else if (/\/lume(?:[.-])/.test(file)) select('lume');
      else select();
    } else if (
      file.startsWith('libs/cua-driver/rust/') ||
      file.startsWith('.cargo/') ||
      /^rust-toolchain(?:\.toml)?$/.test(file)
    ) {
      const platform = file.match(
        /^libs\/cua-driver\/rust\/crates\/platform-(linux|macos|windows)\//
      )?.[1];
      select('cua-driver', platform);
    } else if (file.startsWith('libs/lume/')) {
      select('lume');
    } else {
      for (const library of ['cua-driver', 'lume']) {
        const generator = config.generators[library];
        for (const output of generator.outputs) {
          if (file !== `${generator.docsOutputPath}/${output.outputFile}`) continue;
          const platform = output.platform ? hosts[output.platform.host] : undefined;
          if (output.platform && !platform)
            throw new Error(`Unknown output host: ${output.platform.host}`);
          select(library, platform);
        }
      }
    }
  }
  return { include: rows.filter((row) => selected.has(row)) };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const files = fs.readFileSync(process.argv[2], 'utf8').split('\0').filter(Boolean);
  const matrix = selectNativeMatrix(files);
  process.stdout.write(`matrix=${JSON.stringify(matrix)}\nhas_work=${matrix.include.length > 0}\n`);
}
