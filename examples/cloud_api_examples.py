import asyncio
import os

from utils import load_dotenv_files

load_dotenv_files()

from computer.providers.cloud.provider import CloudProvider


async def main() -> None:
    api_key = os.getenv("CUA_API_KEY")
    if not api_key:
        raise RuntimeError("CUA_API_KEY environment variable is not set")
    api_base = os.getenv("CUA_API_BASE")
    if api_base:
        print(f"Using API base: {api_base}")

    provider = CloudProvider(api_key=api_key, verbose=True)
    async with provider:

        # List all VMs
        vms = await provider.list_vms()
        print(f"Found {len(vms)} VM(s)")
        for vm in vms:
            print(
                f"name: {vm['name']}\n",
                f"status: {vm['status']}\n",  # pending, running, stopped, terminated, failed
                f"api_url: {vm.get('api_url')}\n",
                f"vnc_url: {vm.get('vnc_url')}\n",
            )

        # # --- Additional operations (commented out) ---
        # # To stop a VM by name:
        # name = "m-linux-96lcxd2c2k"
        # resp = await provider.stop_vm(name)
        # print(
        #     "stop_vm response:\n",
        #     f"name: {resp['name']}\n",
        #     f"status: {resp['status']}\n", # stopping
        # )

        # # To start a VM by name:
        # name = "m-linux-96lcxd2c2k"
        # resp = await provider.run_vm(name)
        # print(
        #     "run_vm response:\n",
        #     f"name: {resp['name']}\n",
        #     f"status: {resp['status']}\n", # starting
        # )

        # # To restart a VM by name:
        # name = "m-linux-96lcxd2c2k"
        # resp = await provider.restart_vm(name)
        # print(
        #     "restart_vm response:\n",
        #     f"name: {resp['name']}\n",
        #     f"status: {resp['status']}\n", # restarting
        # )

        # # To probe a VM's status via its public hostname (if you know the name):
        # name = "m-linux-96lcxd2c2k"
        # info = await provider.get_vm(name)
        # print("get_vm info:\n",
        #     f"name: {info['name']}\n",
        #     f"status: {info['status']}\n", # running
        #     f"api_url: {info.get('api_url')}\n",
        #     f"os_type: {info.get('os_type')}\n",
        # )


if __name__ == "__main__":
    asyncio.run(main())
