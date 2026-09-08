import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { Agent } from '@mastra/core/agent';
import { Workspace } from '@mastra/core/workspace';
import {
  CyclopsClient,
  CyclopsCredentials,
  uniffiInitAsync,
  type Sandbox,
} from '@trycua/fleet/node';
import { CuaFleetSandbox } from '../src/index.js';
import { FleetHttpClient, decodeGuestResponse } from '../src/fleet-session.js';

const output = new URL('../output/', import.meta.url);
const recordFile = new URL('run.json', output);
const image =
  'public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04@sha256:c1e601dbb748fdc467c663136f7592e308a91a3c19c309b75261544432826a57';
const delay = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
type RecordFile = { name: string; createdAt?: string; cleanupConfirmed?: boolean };
let stage = 'configuration';
let failedStage: string | undefined;

function env(name: string) {
  const value = process.env[name];
  if (!value) throw new Error(`Missing ${name}`);
  return value;
}

async function main() {
  const mode = process.argv[2] ?? 'model';
  assert.ok(['model', 'scripted', 'cleanup'].includes(mode));
  const modelId = mode === 'model' ? env('MASTRA_MODEL') : undefined;
  const clientId = env('CUA_CLIENT_ID'),
    clientSecret = env('CUA_CLIENT_SECRET');
  await uniffiInitAsync();
  const credentials = new CyclopsCredentials(clientId, clientSecret);
  const client = CyclopsClient.connect(
    {
      baseUrl: 'https://run.cua.ai',
      tokenUrl: 'https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token',
      credentials,
      poolPollIntervalMs: 2_000n,
      poolPollLimit: 450,
      claimPollIntervalMs: 2_000n,
      claimPollLimit: 450,
    },
    new FleetHttpClient(60_000)
  );

  async function cleanup(record: RecordFile) {
    assert.match(record.name, /^mastra-test-[a-f0-9]{20}$/);
    assert.ok(record.createdAt);
    const current = (await client.listNamespaces()).find((item) => item.name === record.name);
    if (current) {
      assert.equal(current.createdAt, record.createdAt, 'Refusing changed namespace identity');
      await client.deleteNamespace(record.name);
    }
    for (let i = 0; i < 90; i++) {
      if (!(await client.listNamespaces()).some((item) => item.name === record.name)) {
        record.cleanupConfirmed = true;
        await writeFile(recordFile, JSON.stringify(record, null, 2) + '\n');
        console.log('PASS: temporary namespace absent from account inventory');
        return;
      }
      await delay(2_000);
    }
    throw new Error('Cleanup pending');
  }

  async function shell(sandbox: Sandbox, command: string) {
    const response = await client.serviceRequest(sandbox, 'server', '/cmd', {
      method: 'POST',
      url: 'https://ignored.invalid/cmd',
      headers: [{ name: 'content-type', value: 'application/json' }],
      body: new TextEncoder().encode(
        JSON.stringify({ command: 'run_command', params: { command, timeout: 30 } })
      ).buffer,
      timeoutSecs: 60n,
    });
    assert.equal(response.status, 200);
    const result = decodeGuestResponse(response.body);
    if (result.success !== true || result.return_code !== 0) {
      console.error('Guest fixture command failed', {
        success: result.success,
        exitCode: result.return_code,
      });
    }
    assert.equal(result.success, true);
    assert.equal(result.return_code, 0);
    assert.equal(typeof result.stdout, 'string');
    return result.stdout as string;
  }

  try {
    if (mode === 'cleanup') {
      await cleanup(JSON.parse(await readFile(recordFile, 'utf8')));
      return;
    }
    await mkdir(output, { recursive: true });
    const record: RecordFile = {
      name: `mastra-test-${randomUUID().replaceAll('-', '').slice(0, 20)}`,
    };
    await writeFile(recordFile, JSON.stringify(record, null, 2), { flag: 'wx' });
    stage = 'namespace reservation';
    const ns = await client.createNamespace(record.name);
    record.createdAt = ns.createdAt;
    assert.ok(record.createdAt);
    try {
      await writeFile(recordFile, JSON.stringify(record, null, 2));
      stage = 'pool provisioning';
      console.log('Provisioning a temporary Linux Fleet pool (4 CPU / 4 GiB; one-hour TTL)...');
      const templateName = `${record.name}-template`;
      await client.createTemplate({
        namespace: record.name,
        name: templateName,
        spec: {
          vmTemplate: {
            containerDiskImage: image,
            cpuCores: 4,
            memory: '4Gi',
            services: [{ name: 'server', targetPort: 8000 }],
          },
        },
      });
      await client.createPool({
        namespace: record.name,
        spec: {
          replicas: 1,
          sandboxTemplateRef: { name: templateName },
          ttlSecondsAfterCreated: 3600,
        },
      });
      const sandbox = new CuaFleetSandbox({
        id: randomUUID(),
        poolName: record.name,
        clientId,
        clientSecret,
      });
      const workspace = new Workspace({ sandbox });
      try {
        stage = 'provider startup';
        await sandbox.start();
        const claims = await client.listClaims(record.name);
        assert.equal(claims.length, 1);
        const guest = await client.waitClaim(claims[0]!);
        stage = 'fixture setup';
        const fixture = (await readFile(new URL('./fixture.py', import.meta.url))).toString(
          'base64'
        );
        const bootstrap = Buffer.from(
          `import base64, pathlib, subprocess, shutil, time, urllib.request
pathlib.Path('/tmp/mastra-fleet-fixture.py').write_bytes(base64.b64decode('${fixture}'))
detached = dict(stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
subprocess.Popen(['python3', '/tmp/mastra-fleet-fixture.py'], **detached)
for attempt in range(30):
    try:
        urllib.request.urlopen('http://127.0.0.1:8765', timeout=1).close()
        break
    except Exception:
        time.sleep(0.2)
else:
    raise RuntimeError('FIXTURE_SERVER_UNAVAILABLE')
browser = next((shutil.which(name) for name in ['chromium', 'chromium-browser', 'google-chrome', 'google-chrome-stable'] if shutil.which(name)), None)
if not browser:
    raise RuntimeError('CHROMIUM_BROWSER_MISSING')
subprocess.Popen([browser, '--no-sandbox', '--disable-dev-shm-usage', '--no-first-run', '--user-data-dir=/tmp/mastra-fleet-browser', '--kiosk', 'http://127.0.0.1:8765'], **detached)
print('FIXTURE_STARTED')
`
        ).toString('base64');
        const setup = await shell(
          guest,
          `python3 -c "import base64; exec(base64.b64decode('${bootstrap}'))"`
        );
        assert.match(setup, /FIXTURE_STARTED/);
        await delay(5_000);
        stage = 'initial fixture screenshot';
        await writeFile(new URL('before.png', output), (await sandbox.computer.screenshot()).data);
        const marker = `mastra-${randomUUID().slice(0, 8)}`;
        const usage = { inputTokens: 1, outputTokens: 1, totalTokens: 2 };
        const tool = (name: string, input: object, index: number) => ({
          content: [
            {
              type: 'tool-call' as const,
              toolCallId: `fixture-${index}`,
              toolName: `mastra_workspace_computer_${name}`,
              input: JSON.stringify(input),
            },
          ],
          finishReason: 'tool-calls' as const,
          usage,
          warnings: [],
        });
        // Scripted mode tests the real Mastra tool loop, not autonomous model choice.
        const responses = [
          tool('screenshot', {}, 0),
          tool('click', { x: 200, y: 205 }, 1),
          tool('type', { text: marker }, 2),
          tool('click', { x: 150, y: 280 }, 3),
          {
            content: [{ type: 'text' as const, text: 'Fixture submitted.' }],
            finishReason: 'stop' as const,
            usage,
            warnings: [],
          },
        ];
        let responseIndex = 0;
        const model = modelId ?? {
          specificationVersion: 'v2' as const,
          provider: 'fixture',
          modelId: 'scripted',
          supportedUrls: {},
          doGenerate: async () => {
            const response = responses[responseIndex++];
            if (!response) throw new Error('Scripted model exhausted');
            return response;
          },
          doStream: async (): Promise<never> => {
            throw new Error('Streaming is not used by this fixture');
          },
        };
        const agent = new Agent({
          id: 'fleet-fixture-agent',
          name: 'Fleet Fixture Agent',
          model,
          workspace,
          instructions:
            'Use only computer tools to interact with the visible test page. Take a screenshot first. Type the requested value into Verification value and click Submit value. Confirm the displayed Submitted result. Do not use the terminal, address bar, developer tools, or other applications.',
        });
        stage = `${mode} Mastra agent`;
        console.log(`Running ${mode} Mastra agent on the guest desktop...`);
        const result = await agent.generate(`Submit exactly ${marker} using the test page.`, {
          maxSteps: 15,
        });
        stage = 'independent fixture assertion';
        const observed = JSON.parse(await shell(guest, 'cat /tmp/mastra-fleet-fixture-state.json'));
        assert.equal(observed.value, marker);
        await writeFile(new URL('after.png', output), (await sandbox.computer.screenshot()).data);
        await writeFile(
          new URL('result.json', output),
          JSON.stringify(
            {
              mode,
              model: modelId ?? 'scripted LanguageModelV2 fixture',
              timestamp: new Date().toISOString(),
              providerVersion: '0.1.0',
              mastraVersion: '1.62.0',
              fleetVersion: '0.1.1',
              fixtureVerified: true,
              steps: result.steps.length,
              image,
            },
            null,
            2
          ) + '\n'
        );
        console.log('PASS: guest fixture independently recorded the exact submitted value');
      } catch (error) {
        failedStage = stage;
        throw error;
      } finally {
        stage = 'claim cleanup';
        await sandbox.destroy();
        assert.equal((await client.listClaims(record.name)).length, 0);
        console.log('PASS: provider released its claim and left the caller-owned pool intact');
        assert.equal((await client.getPool(record.name)).metadata.name, record.name);
      }
    } catch (error) {
      failedStage ??= stage;
      throw error;
    } finally {
      stage = 'pool cleanup';
      await cleanup(record);
    }
  } finally {
    if (client instanceof CyclopsClient) client.uniffiDestroy();
    credentials.uniffiDestroy();
  }
}

main().catch(() => {
  console.error(
    `Example failed during ${failedStage ?? stage}. Raw SDK/model errors are withheld. Preserve output/run.json and run the cleanup command with the same account.`
  );
  process.exitCode = 1;
});
