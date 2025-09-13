from computer import Computer
import asyncio
import logging
from pathlib import Path
from agent import ComputerAgent
import os


async def computer_use_agent(midi_file='./hum_basic_pitch.mid', instrument='guitar'):
    """
    Main async function that handles the computer automation.
    This function:
    1. Reads the MIDI file created from your humming
    2. Connects to a cloud computer environment  
    3. Uploads the MIDI file to the remote computer
    """
    # Read the MIDI file that was generated from your humming
    midi_file = open(midi_file, "rb")
    content = midi_file.read()
    midi_file.close()
    
    print("📁 MIDI file loaded successfully")
    print(f"📊 File size: {len(content)} bytes")
    
    # Connect to the cloud computer environment
    async with Computer(
        os_type="linux",                    # Using Linux environment
        provider_type="cloud",              # Cloud-based computer
        name="l-linux-x318jdeu72",         # Specific instance name
        api_key=os.getenv('CUA_API_KEY')
    ) as computer:
        print("🖥️ Connected to cloud computer")
        
        # Start the computer interface
        await computer.run()
        print("▶️ Computer interface started")
        
        # Upload the MIDI file to the remote computer
        await computer.interface.write_bytes("~/Downloads/midi-file-name.midi", content)
        print("✅ MIDI file uploaded to ~/Downloads/midi-file-name.midi")
        midi_name = "midi-file-name.midi"


        agent = ComputerAgent(
            model="anthropic/claude-opus-4-20250514",
            tools=[computer],
            max_trajectory_budget=5.0
        )
        tasks = [f"""
You are inside BandLab Studio in Firefox on Linux. p

Goal: Import the MIDI file and play it with the chosen instrument.

FILE TO IMPORT: "~/Downloads/midi-file-name.midi"
FILE NAME ONLY: 
INSTRUMENT: {instrument}

Do the following step by step:
   
1. Click the dashed box in the timeline that says “Drop a loop or an audio/MIDI/video file”.
   - This should open a file upload dialog.

2. In the file dialog:
   - Click “Downloads” in the sidebar.
   - Find and double-click “{midi_name}”.
   - If it’s not visible, type “{midi_name}” into the filename field and press Enter.
   - Wait until the MIDI region appears on the timeline.
   - Take a screenshot.
   
3. Click the "Instrument" button in the bottom left corner. It is the button with the text "Instrument" and an icon of a piano.

4. Click the grey button with text "Grand Piano". Wait for a pop up called Browse Instruments to show up. 

5. In the search bar, type in the {instrument} name. Select the first clickable option in the list in the pop up. Click the "Instrument" button in the bottom left corner.


6. Click the long white cursor and drag it to the beginning.

Rules:
- Always interact inside BandLab, not the browser’s URL bar.
- Use precise clicks; scroll if needed.
"""]

        for i, task in enumerate(tasks):
            print(f"\nExecuting task {i}/{len(tasks)}: {task}")
            async for result in agent.run(task):
                print(result)
            print(f"\n✅ Task {i+1}/{len(tasks)} completed: {task}")





# This ensures the main function runs when the script is executed
# It properly handles the async/await syntax
if __name__ == "__main__":
    asyncio.run(computer_use_agent())