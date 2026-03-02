import type { Argv } from 'yargs';
import { createHash } from 'crypto';
import { ensureApiKeyInteractive } from '../auth';
import { http } from '../http';
import { clearApiKey } from '../storage';

// Types for image API responses
type ImageInfo = {
  image_id: number;
  name: string;
  description: string;
  image_type: string;
  created_at: string;
  versions: ImageVersionInfo[];
};

type ImageVersionInfo = {
  version_id: number;
  tag: string;
  size_bytes: number;
  checksum_sha256: string;
  status: string;
  created_at: string;
};

type UploadSession = {
  upload_id: string;
  version_id: number;
  part_size: number;
  total_parts: number;
  expires_at: string;
};

type PartInfo = {
  part_number: number;
  etag: string;
};

// Default part size: 100MB
const DEFAULT_PART_SIZE = 100 * 1024 * 1024;

// Helper to format bytes
function formatBytes(bytes: number): string {
  if (bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
}

// Helper to calculate SHA256 hash of a file
async function calculateFileHash(filePath: string): Promise<string> {
  const file = Bun.file(filePath);
  const buffer = await file.arrayBuffer();
  const hash = createHash('sha256');
  hash.update(Buffer.from(buffer));
  return hash.digest('hex');
}

// Helper for printing tables
function printTable(headers: string[], rows: string[][]) {
  const allRows = [headers, ...rows];
  const widths: number[] = new Array(headers.length).fill(0);

  for (const r of allRows)
    for (let i = 0; i < headers.length; i++)
      widths[i] = Math.max(widths[i] ?? 0, (r[i] ?? '').length);

  for (const r of allRows)
    console.log(r.map((c, i) => (c ?? '').padEnd(widths[i] ?? 0)).join('  '));
}

// Command handlers
const listHandler = async (argv: Record<string, unknown>) => {
  const token = await ensureApiKeyInteractive();
  const res = await http('/v1/images', { token });

  if (res.status === 401) {
    clearApiKey();
    console.error("Unauthorized. Try 'cua login' again.");
    process.exit(1);
  }
  if (!res.ok) {
    const body = await res.json().catch(() => ({}));
    console.error(`Request failed: ${res.status}`, (body as any).error || '');
    process.exit(1);
  }

  const images = (await res.json()) as ImageInfo[];

  if (argv.json) {
    console.log(JSON.stringify(images, null, 2));
    return;
  }

  if (images.length === 0) {
    console.log('No images found');
    return;
  }

  const headers = ['NAME', 'TYPE', 'TAG', 'SIZE', 'STATUS', 'CREATED'];
  const rows: string[][] = [];

  for (const img of images) {
    if (img.versions && img.versions.length > 0) {
      for (const ver of img.versions) {
        rows.push([
          img.name,
          img.image_type,
          ver.tag,
          formatBytes(ver.size_bytes),
          ver.status,
          new Date(ver.created_at).toLocaleDateString(),
        ]);
      }
    } else {
      rows.push([
        img.name,
        img.image_type,
        '-',
        '-',
        '-',
        new Date(img.created_at).toLocaleDateString(),
      ]);
    }
  }

  printTable(headers, rows);
};

const pushHandler = async (argv: Record<string, unknown>) => {
  const token = await ensureApiKeyInteractive();
  const name = String(argv.name);
  const tag = String(argv.tag || 'latest');
  const imageType = String(argv.type || 'qcow2');

  // Determine file path - use --file if provided, otherwise look in cua-bench images directory
  let filePath: string;
  if (argv.file) {
    filePath = String(argv.file);
  } else {
    // Look for image in cua-bench local storage
    const home = process.env.HOME || process.env.USERPROFILE || '';
    const cuaBenchPath = `${home}/.local/share/cua-bench/images/${name}/data.img`;
    filePath = cuaBenchPath;
  }

  // Check if file exists and get size
  const file = Bun.file(filePath);
  const exists = await file.exists();
  if (!exists) {
    if (argv.file) {
      console.error(`File not found: ${filePath}`);
    } else {
      console.error(`Image not found: ${name}`);
      console.error(`Looked in: ${filePath}`);
      console.error(`Use --file to specify a custom path`);
    }
    process.exit(1);
  }

  const sizeBytes = file.size;
  console.log(`Pushing ${filePath} (${formatBytes(sizeBytes)})`);

  // Calculate checksum
  console.log('Calculating checksum...');
  const checksum = await calculateFileHash(filePath);
  console.log(`Checksum: ${checksum}`);

  // Initiate upload
  console.log('Initiating upload...');
  const initRes = await http(`/v1/images/${encodeURIComponent(name)}/upload`, {
    token,
    method: 'POST',
    body: {
      tag,
      image_type: imageType,
      size_bytes: sizeBytes,
      checksum_sha256: checksum,
    },
  });

  if (initRes.status === 401) {
    clearApiKey();
    console.error("Unauthorized. Try 'cua login' again.");
    process.exit(1);
  }
  if (initRes.status === 409) {
    console.error(`Image version already exists: ${name}:${tag}`);
    process.exit(1);
  }
  if (!initRes.ok) {
    const body = await initRes.json().catch(() => ({}));
    console.error(
      `Failed to initiate upload: ${initRes.status}`,
      (body as any).error || ''
    );
    process.exit(1);
  }

  const session = (await initRes.json()) as UploadSession;
  console.log(`Upload session: ${session.upload_id}`);
  console.log(
    `Parts: ${session.total_parts} x ${formatBytes(session.part_size)}`
  );

  // Upload parts
  const completedParts: PartInfo[] = [];
  const fileBuffer = await file.arrayBuffer();

  for (let partNum = 1; partNum <= session.total_parts; partNum++) {
    const start = (partNum - 1) * session.part_size;
    const end = Math.min(start + session.part_size, sizeBytes);
    const partData = fileBuffer.slice(start, end);

    // Get signed URL for this part
    const urlRes = await http(
      `/v1/images/${encodeURIComponent(name)}/upload/${session.upload_id}/part/${partNum}`,
      { token }
    );

    if (!urlRes.ok) {
      console.error(`Failed to get upload URL for part ${partNum}`);
      // Abort upload
      await http(
        `/v1/images/${encodeURIComponent(name)}/upload/${session.upload_id}`,
        {
          token,
          method: 'DELETE',
        }
      );
      process.exit(1);
    }

    const urlData = (await urlRes.json()) as {
      upload_url: string;
      expires_in: number;
    };

    // Upload the part
    process.stdout.write(
      `\rUploading part ${partNum}/${session.total_parts}...`
    );

    const uploadRes = await fetch(urlData.upload_url, {
      method: 'PUT',
      body: partData,
      headers: {
        'Content-Type': 'application/octet-stream',
      },
    });

    if (!uploadRes.ok) {
      console.error(`\nFailed to upload part ${partNum}: ${uploadRes.status}`);
      // Abort upload
      await http(
        `/v1/images/${encodeURIComponent(name)}/upload/${session.upload_id}`,
        {
          token,
          method: 'DELETE',
        }
      );
      process.exit(1);
    }

    // Get ETag from response
    const etag = uploadRes.headers.get('ETag') || '';
    completedParts.push({ part_number: partNum, etag });
  }

  console.log('\nCompleting upload...');

  // Complete the upload
  const completeRes = await http(
    `/v1/images/${encodeURIComponent(name)}/upload/${session.upload_id}/complete`,
    {
      token,
      method: 'POST',
      body: { parts: completedParts },
    }
  );

  if (!completeRes.ok) {
    const body = await completeRes.json().catch(() => ({}));
    console.error(
      `Failed to complete upload: ${completeRes.status}`,
      (body as any).error || ''
    );
    process.exit(1);
  }

  const result = (await completeRes.json()) as ImageVersionInfo;
  console.log(`Push complete: ${name}:${tag}`);
  console.log(`Version ID: ${result.version_id}`);
  console.log(`Status: ${result.status}`);
};

const pullHandler = async (argv: Record<string, unknown>) => {
  const token = await ensureApiKeyInteractive();
  const name = String(argv.name);
  const tag = String(argv.tag || 'latest');
  const outputPath = String(argv.output || `${name}-${tag}.qcow2`);

  console.log(`Pulling ${name}:${tag}...`);

  // Get download URL
  const urlRes = await http(
    `/v1/images/${encodeURIComponent(name)}/download?tag=${encodeURIComponent(tag)}`,
    {
      token,
    }
  );

  if (urlRes.status === 401) {
    clearApiKey();
    console.error("Unauthorized. Try 'cua login' again.");
    process.exit(1);
  }
  if (urlRes.status === 404) {
    console.error(`Image not found: ${name}:${tag}`);
    process.exit(1);
  }
  if (!urlRes.ok) {
    const body = await urlRes.json().catch(() => ({}));
    console.error(
      `Failed to get download URL: ${urlRes.status}`,
      (body as any).error || ''
    );
    process.exit(1);
  }

  const urlData = (await urlRes.json()) as {
    download_url: string;
    size_bytes: number;
    checksum_sha256: string;
  };
  console.log(`Size: ${formatBytes(urlData.size_bytes)}`);

  // Download the file
  console.log(`Downloading to ${outputPath}...`);
  const downloadRes = await fetch(urlData.download_url);

  if (!downloadRes.ok) {
    console.error(`Download failed: ${downloadRes.status}`);
    process.exit(1);
  }

  const buffer = await downloadRes.arrayBuffer();
  await Bun.write(outputPath, buffer);

  // Verify checksum
  console.log('Verifying checksum...');
  const downloadedChecksum = await calculateFileHash(outputPath);

  if (downloadedChecksum !== urlData.checksum_sha256) {
    console.error('Checksum mismatch! Download may be corrupted.');
    console.error(`Expected: ${urlData.checksum_sha256}`);
    console.error(`Got: ${downloadedChecksum}`);
    process.exit(1);
  }

  console.log(`Pull complete: ${outputPath}`);
  console.log(`Checksum verified: ${downloadedChecksum}`);
};

const deleteHandler = async (argv: Record<string, unknown>) => {
  const token = await ensureApiKeyInteractive();
  const name = String(argv.name);
  const tag = String(argv.tag || 'latest');

  if (!argv.force) {
    console.log(`This will delete ${name}:${tag}. Use --force to confirm.`);
    process.exit(1);
  }

  console.log(`Deleting ${name}:${tag}...`);

  const res = await http(
    `/v1/images/${encodeURIComponent(name)}?tag=${encodeURIComponent(tag)}`,
    {
      token,
      method: 'DELETE',
    }
  );

  if (res.status === 401) {
    clearApiKey();
    console.error("Unauthorized. Try 'cua login' again.");
    process.exit(1);
  }
  if (res.status === 404) {
    console.error(`Image not found: ${name}:${tag}`);
    process.exit(1);
  }
  if (!res.ok) {
    const body = await res.json().catch(() => ({}));
    console.error(`Delete failed: ${res.status}`, (body as any).error || '');
    process.exit(1);
  }

  console.log(`Deleted: ${name}:${tag}`);
};

// Register commands
export function registerImageCommands(y: Argv) {
  y.command(
    ['image', 'img'],
    'Manage VM images (push, pull, delete)',
    (y) => {
      return y
        .command(
          ['list', 'ls'],
          'List all images in your workspace',
          (y) =>
            y.option('json', {
              type: 'boolean',
              default: false,
              describe: 'Output in JSON format',
            }),
          listHandler
        )
        .command(
          'push <name>',
          'Push a VM image to cloud storage',
          (y) =>
            y
              .positional('name', {
                type: 'string',
                describe: 'Image name',
                demandOption: true,
              })
              .option('file', {
                alias: 'f',
                type: 'string',
                describe:
                  'Path to image file (defaults to ~/.local/share/cua-bench/images/<name>/data.img)',
              })
              .option('tag', {
                type: 'string',
                default: 'latest',
                describe: 'Image tag/version',
              })
              .option('type', {
                type: 'string',
                default: 'qcow2',
                choices: ['qcow2', 'raw', 'vmdk'],
                describe: 'Image type',
              }),
          pushHandler
        )
        .command(
          'pull <name>',
          'Pull a VM image from cloud storage',
          (y) =>
            y
              .positional('name', {
                type: 'string',
                describe: 'Image name',
                demandOption: true,
              })
              .option('tag', {
                type: 'string',
                default: 'latest',
                describe: 'Image tag/version',
              })
              .option('output', {
                alias: 'o',
                type: 'string',
                describe: 'Output file path',
              }),
          pullHandler
        )
        .command(
          'delete <name>',
          'Delete an image version from cloud storage',
          (y) =>
            y
              .positional('name', {
                type: 'string',
                describe: 'Image name',
                demandOption: true,
              })
              .option('tag', {
                type: 'string',
                default: 'latest',
                describe: 'Image tag/version',
              })
              .option('force', {
                alias: 'f',
                type: 'boolean',
                default: false,
                describe: 'Skip confirmation',
              }),
          deleteHandler
        )
        .demandCommand(1, 'You must provide an image command');
    },
    () => {}
  );

  return y;
}
