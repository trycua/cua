import os
import sys
from pathlib import Path

# Load environment variables from .env file
project_root = Path(__file__).parent.parent
env_file = project_root / ".env"
print(f"Loading environment from: {env_file}")
from dotenv import load_dotenv

load_dotenv(env_file)

# Add paths to sys.path if needed
pythonpath = os.environ.get("PYTHONPATH", "")
for path in pythonpath.split(":"):
    if path and path not in sys.path:
        sys.path.insert(0, path)  # Insert at beginning to prioritize
        print(f"Added to sys.path: {path}")

import asyncio

from computer.computer import Computer
from computer.helpers import sandboxed


async def main():
    # Initialize the computer in a Cua Container
    computer = Computer()
    await computer.run()

    # Install a package in a virtual environment in the container
    await computer.venv_install("demo_venv", ["requests", "macos-pyxa"])

    # Open Safari
    await computer.interface.run_command("open -a Safari")
    await asyncio.sleep(2)

    # Define a sandboxed function
    # This function will run inside the Cua Container
    @sandboxed("demo_venv")
    def greet_and_print(name):
        # get .html of the current Safari tab
        import PyXA

        safari = PyXA.Application("Safari")
        current_doc = safari.current_document
        html = current_doc.source()
        print(f"Hello from inside the container, {name}!")
        print("Safari HTML length:", len(html))
        return {"greeted": name, "safari_html_length": len(html), "safari_html_snippet": html[:200]}

    # Call with args and kwargs
    result = await greet_and_print("Cua")
    print("Result from sandboxed function:", result)


if __name__ == "__main__":
    asyncio.run(main())
