from computer import Computer
import asyncio


async def main():
    """
    Main async function that handles the computer automation.
    This function:
    1. Reads the MIDI file created from your humming
    2. Connects to a cloud computer environment  
    3. Uploads the MIDI file to the remote computer
    """
    # Read the MIDI file that was generated from your humming
    midi_file = open("./hum_basic_pitch.mid", "rb")
    content = midi_file.read()
    midi_file.close()
    
    print("📁 MIDI file loaded successfully")
    print(f"📊 File size: {len(content)} bytes")
    
    # Connect to the cloud computer environment
    async with Computer(
        os_type="linux",                    # Using Linux environment
        provider_type="cloud",              # Cloud-based computer
        name="l-linux-x318jdeu72",         # Specific instance name
        api_key="sk_cua-api01_36f300ae69521b32bc04505961205c3f1ca6900773344e019e93da12b5be62d0"
    ) as computer:
        print("🖥️ Connected to cloud computer")
        
        # Start the computer interface
        await computer.run()
        print("▶️ Computer interface started")
        
        # Upload the MIDI file to the remote computer
        await computer.interface.write_bytes("~/Downloads/midi-file-name.midi", content)
        print("✅ MIDI file uploaded to ~/Downloads/midi-file-name.midi")


# This ensures the main function runs when the script is executed
# It properly handles the async/await syntax
if __name__ == "__main__":
    asyncio.run(main())