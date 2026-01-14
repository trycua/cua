import asyncio
import os

from computer import Computer, VMProviderType
from computer.providers.factory import VMProviderFactory


async def main():
    # # Create docker provider
    # provider = VMProviderFactory.create_provider(
    #     provider_type="docker",
    #     image="cua-ubuntu:latest",  # Your Cua Ubuntu image
    #     port=8080,
    #     vnc_port=6901
    # )

    # # Run a container
    # async with provider:
    #     vm_info = await provider.run_vm(
    #         image="cua-ubuntu:latest",
    #         name="my-cua-container",
    #         run_opts={
    #             "memory": "4GB",
    #             "cpu": 2,
    #             "vnc_port": 6901,
    #             "api_port": 8080
    #         }
    #     )
    #     print(vm_info)

    computer = Computer(
        os_type="linux",
        provider_type=VMProviderType.DOCKER,
        name="my-cua-container",
        image="cua-ubuntu:latest",
    )

    await computer.run()

    screenshot = await computer.interface.screenshot()
    with open("screenshot_docker.png", "wb") as f:
        f.write(screenshot)


if __name__ == "__main__":
    asyncio.run(main())
