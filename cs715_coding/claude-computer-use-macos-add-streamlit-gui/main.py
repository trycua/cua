import asyncio
import os
import sys
import json
import base64
import streamlit as st

from computer_use_demo.loop import sampling_loop, APIProvider
from computer_use_demo.tools import ToolResult
from anthropic.types.beta import BetaMessage, BetaMessageParam
from anthropic import APIResponse


async def main():
    # Set up your Anthropic API key and model
    api_key = os.getenv("ANTHROPIC_API_KEY", "YOUR_API_KEY_HERE")
    if api_key == "YOUR_API_KEY_HERE":
        raise ValueError(
            "Please first set your API key in the ANTHROPIC_API_KEY environment variable"
        )
    provider = APIProvider.ANTHROPIC

    # Check if the instruction is provided via command line arguments
    if len(sys.argv) > 1:
        instruction = " ".join(sys.argv[1:])
    else:
        instruction = "Save an image of a cat to the desktop."

    print(
        f"Starting Claude 'Computer Use'.\nPress ctrl+c to stop.\nInstructions provided: '{instruction}'"
    )

    # Set up the initial messages
    messages: list[BetaMessageParam] = [
        {
            "role": "user",
            "content": instruction,
        }
    ]

    # Define callbacks (you can customize these)
    def output_callback(content_block):
        if isinstance(content_block, dict) and content_block.get("type") == "text":
            st.write("Assistant:", content_block.get("text"))

    def tool_output_callback(result: ToolResult, tool_use_id: str):
        if result.output:
            st.write(f"> Tool Output [{tool_use_id}]:", result.output)
        if result.error:
            st.write(f"!!! Tool Error [{tool_use_id}]:", result.error)
        if result.base64_image:
            # Save the image to a file if needed
            os.makedirs("screenshots", exist_ok=True)
            image_data = result.base64_image
            with open(f"screenshots/screenshot_{tool_use_id}.png", "wb") as f:
                f.write(base64.b64decode(image_data))
            st.image(f"screenshots/screenshot_{tool_use_id}.png", caption=f"Screenshot {tool_use_id}")

    def api_response_callback(response: APIResponse[BetaMessage]):
        st.write(
            "\n---------------\nAPI Response:\n",
            json.dumps(json.loads(response.text)["content"], indent=4),  # type: ignore
            "\n",
        )

    # Run the sampling loop
    messages = await sampling_loop(
        model="claude-3-5-sonnet-20241022",
        provider=provider,
        system_prompt_suffix="",
        messages=messages,
        output_callback=output_callback,
        tool_output_callback=tool_output_callback,
        api_response_callback=api_response_callback,
        api_key=api_key,
        only_n_most_recent_images=10,
        max_tokens=4096,
    )


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except Exception as e:
        st.write(f"Encountered Error:\n{e}")
