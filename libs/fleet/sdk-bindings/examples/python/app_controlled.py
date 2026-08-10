import asyncio
from contract_fixture import ScriptedHttpClient, expected_lifecycle, run_lifecycle
async def main():
    pool, claim, sandbox, service = await run_lifecycle(ScriptedHttpClient(expected_lifecycle()))
    print(f'pool={pool.metadata.name} claim={claim.metadata.name} sandbox={sandbox.name} service_status={service.status}')
asyncio.run(main())
