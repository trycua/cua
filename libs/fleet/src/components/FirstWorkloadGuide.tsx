import Alert from "@cloudscape-design/components/alert"
import Box from "@cloudscape-design/components/box"
import Container from "@cloudscape-design/components/container"
import CopyToClipboard from "@cloudscape-design/components/copy-to-clipboard"
import Header from "@cloudscape-design/components/header"
import Link from "@cloudscape-design/components/link"
import SpaceBetween from "@cloudscape-design/components/space-between"
import type { PoolService } from "../fleet/models"

const PYTHON_PACKAGE = "cua-sandbox==0.7.0"

export function firstWorkloadScript({
  namespace,
  pool,
  claim,
}: {
  namespace: string
  pool: string
  claim: string
}) {
  const reference = JSON.stringify(
    {
      version: 1,
      provider: "fleet",
      namespace,
      pool,
      claim,
      service: "server",
    },
    null,
    4,
  )

  return `import asyncio
from cua_sandbox import Sandbox

reference = ${reference}

async def main():
    async with Sandbox.from_dict(reference) as sandbox:
        result = await sandbox.shell.run("uname -sr", timeout=30)
        if result.returncode != 0:
            raise RuntimeError(result.stderr or "Workload failed")
        output = result.stdout.strip()
        if not output:
            raise RuntimeError("Workload returned no output")
        print(output)

asyncio.run(main())`
}

export function FirstWorkloadGuide({
  namespace,
  pool,
  claim,
  services,
}: {
  namespace: string
  pool: string
  claim: string
  services: PoolService[]
}) {
  const hasServer = services.some(service => service.name === "server")

  if (!hasServer) {
    return (
      <Container
        header={
          <Header
            variant="h2"
            description="Connect to this claim and run a command in its sandbox."
          >
            Run your first workload
          </Header>
        }
      >
        <Alert type="warning" header="This pool does not expose the server service">
          Add a service named <code>server</code> to the pool configuration before
          connecting with the Sandbox SDK. This claim currently exposes: {services.length > 0
            ? services.map(service => service.name).join(", ")
            : "no services"}.
        </Alert>
      </Container>
    )
  }

  const script = firstWorkloadScript({ namespace, pool, claim })

  return (
    <Container
      header={
        <Header
          variant="h2"
          description="Connect to this existing claim and run a real command in its sandbox."
        >
          Run your first workload
        </Header>
      }
    >
      <SpaceBetween size="m">
        {/* Preserve the claim-specific instructions while credentials are created. */}
        <Box>
          1. Create an <Link href="#/user-keys" external externalIconAriaLabel="Opens in a new tab">API key</Link> with access to{" "}
          <code>{namespace}</code>, then set its values as <code>CUA_CLIENT_ID</code>{" "}
          and <code>CUA_CLIENT_SECRET</code> in your environment.
        </Box>
        <Box>
          2. Install the SDK: <code>pip install {PYTHON_PACKAGE}</code>
        </Box>
        <Box>
          3. Save and run this Python code. The claim details are already filled in.
        </Box>
        <Box variant="code" padding="m">
          <pre style={{ margin: 0, whiteSpace: "pre-wrap", overflowWrap: "anywhere" }}>
            {script}
          </pre>
        </Box>
        <CopyToClipboard
          variant="button"
          copyButtonText="Copy Python code"
          copyButtonAriaLabel="Copy first workload Python code"
          textToCopy={script}
          copySuccessText="Python code copied"
          copyErrorText="Failed to copy Python code"
        />
        <Alert type="success">
          When the command prints an operating system name, your first workload has completed.
        </Alert>
      </SpaceBetween>
    </Container>
  )
}
